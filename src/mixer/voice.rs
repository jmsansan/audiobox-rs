use std::sync::Arc;

use super::{Loop, Stream, VoiceId};
use crate::math::sin_turns;
use crate::{Audio, ErrorCode, FadeCurve, Result};

/// Fixed-point position: 32 integer and 32 fractional bits of a source frame.
pub(super) const ONE: u64 = 1 << 32;
/// Source frames each voice buffers: interpolation history, lookahead and a read chunk.
const RING: usize = 1024;
const READ: usize = 256;
/// Taps of the interpolation filter on each side of the position.
const HALF: i64 = 8;
const PHASES: usize = 256;

/// A gain that moves from `from` to `to` over `len` frames from frame `start`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Ramp {
    from: f64,
    to: f64,
    start: u64,
    len: u64,
    curve: FadeCurve,
}
impl Ramp {
    pub fn constant(value: f64) -> Self {
        Self {
            from: value,
            to: value,
            start: 0,
            len: 0,
            curve: FadeCurve::Linear,
        }
    }
    pub fn value(&self, frame: u64) -> f64 {
        if self.len == 0 || frame >= self.start + self.len {
            return self.to;
        }
        if frame <= self.start {
            return self.from;
        }
        let t = (frame - self.start) as f64 / self.len as f64;
        let shape = |t: f64| match self.curve {
            FadeCurve::Linear => t,
            FadeCurve::EqualPower => sin_turns(t / 4.0),
            FadeCurve::Exponential => t * t,
        };
        // A rising ramp follows the curve and a falling one mirrors it, as `Audio::fade` does.
        if self.to >= self.from {
            self.from + (self.to - self.from) * shape(t)
        } else {
            self.to + (self.from - self.to) * shape(1.0 - t)
        }
    }
    /// Starts moving from the current value at `frame` to `to` over `len` frames.
    pub fn retarget(&mut self, frame: u64, to: f64, len: u64, curve: FadeCurve) {
        *self = Self {
            from: self.value(frame),
            to,
            start: frame,
            len,
            curve,
        };
    }
    pub fn finished(&self, frame: u64) -> bool {
        self.len == 0 || frame >= self.start + self.len
    }
    pub fn changing(&self, frame: u64) -> bool {
        !self.finished(frame)
    }
}

/// Windowed-sinc interpolation filter for one ratio of source to output rate: 16
/// taps, Blackman window, cutoff 0.9 of the lower Nyquist frequency, 256 phases
/// interpolated linearly. Built with the crate's own sine, so it is identical on
/// every platform.
#[derive(Debug)]
pub(super) struct SincTable {
    rows: Vec<[f64; 16]>,
}
impl SincTable {
    pub fn new(ratio: f64) -> Self {
        let cutoff = 0.9 / ratio.max(1.0);
        let rows = (0..=PHASES)
            .map(|p| {
                let frac = p as f64 / PHASES as f64;
                let mut row = [0.0; 16];
                for (k, tap) in row.iter_mut().enumerate() {
                    let x = (k as i64 - HALF + 1) as f64 - frac;
                    let y = cutoff * x;
                    let sinc = if y == 0.0 {
                        1.0
                    } else {
                        sin_turns(y / 2.0) / (std::f64::consts::PI * y)
                    };
                    let w = x / HALF as f64;
                    let window = if w.abs() >= 1.0 {
                        0.0
                    } else {
                        0.42 + 0.5 * sin_turns(w / 2.0 + 0.25) + 0.08 * sin_turns(w + 0.25)
                    };
                    *tap = cutoff * sinc * window;
                }
                let sum: f64 = row.iter().sum();
                row.map(|t| t / sum)
            })
            .collect();
        Self { rows }
    }
}

pub(super) enum Feed {
    Memory(Arc<Audio>),
    Stream(Box<dyn Stream>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum State {
    Playing,
    /// Fading out after a stop or a steal; the voice ends when the fade does.
    Stopping,
    Finished,
    Failed(ErrorCode),
}

pub(super) struct Voice {
    pub id: VoiceId,
    pub bus: usize,
    pub priority: u8,
    /// Order of starting, to find the oldest voice.
    pub order: u64,
    pub state: State,
    pub gain: Ramp,
    pub pan: Ramp,
    /// Fade-in and fade-out envelope.
    pub fade: Ramp,
    /// Frames and curve of the fade-in, applied when the voice starts.
    pub fade_in: (u64, FadeCurve),
    feed: Feed,
    channels: usize,
    looping: Option<(u64, Option<u64>)>,
    /// Next source frame the feed reads.
    source_pos: u64,
    ring: Vec<f32>,
    /// Virtual frames buffered so far, and the virtual frame where the feed ended.
    filled: i64,
    end: Option<i64>,
    position: u64,
    step: u64,
    table: Option<Arc<SincTable>>,
}

impl Voice {
    pub fn new(
        id: VoiceId,
        feed: Feed,
        channels: usize,
        looping: Loop,
        step: u64,
        table: Option<Arc<SincTable>>,
    ) -> Self {
        let looping = match looping {
            Loop::Off => None,
            Loop::Whole => Some((0, None)),
            Loop::Region { start, end } => Some((start, Some(end))),
        };
        Self {
            id,
            bus: 0,
            priority: 128,
            order: 0,
            state: State::Playing,
            gain: Ramp::constant(1.0),
            pan: Ramp::constant(0.0),
            fade: Ramp::constant(1.0),
            fade_in: (0, FadeCurve::Linear),
            feed,
            channels,
            looping,
            source_pos: 0,
            ring: vec![0.0; RING * channels],
            filled: 0,
            end: None,
            position: 0,
            step,
            table,
        }
    }
    pub fn live(&self) -> bool {
        self.state == State::Playing
    }
    pub fn done(&self) -> bool {
        matches!(self.state, State::Finished | State::Failed(_))
    }
    /// Starts the fade-in at `frame`.
    pub fn start(&mut self, frame: u64) {
        let (len, curve) = self.fade_in;
        if len > 0 {
            self.fade = Ramp {
                from: 0.0,
                to: 1.0,
                start: frame,
                len,
                curve,
            };
        }
    }
    pub fn stop(&mut self, frame: u64, len: u64, curve: FadeCurve) {
        if self.done() {
            return;
        }
        if len == 0 {
            self.state = State::Finished;
        } else {
            self.fade.retarget(frame, 0.0, len, curve);
            self.state = State::Stopping;
        }
    }

    /// Adds `frames` frames from output frame `first` into `out`, interleaved with
    /// `out_channels` channels.
    pub fn render(&mut self, first: u64, frames: usize, out: &mut [f32], out_channels: usize) {
        // Ramps only start at segment boundaries, so one finished at `first` stays put.
        let constant =
            !self.gain.changing(first) && !self.fade.changing(first) && !self.pan.changing(first);
        let fixed = constant.then(|| self.gains(first, out_channels));
        for j in 0..frames {
            if self.done() {
                return;
            }
            let frame = first + j as u64;
            let Some((left, right)) = self.next_frame() else {
                return;
            };
            let (gl, gr) = fixed.unwrap_or_else(|| self.gains(frame, out_channels));
            if out_channels == 1 {
                out[j] += (left * gl + right * gr) as f32;
            } else {
                out[2 * j] += (left * gl) as f32;
                out[2 * j + 1] += (right * gr) as f32;
            }
            if self.state == State::Stopping && self.fade.finished(frame + 1) {
                self.state = State::Finished;
            }
        }
    }
    /// Gains applied to the source's left and right samples (equal for mono).
    ///
    /// A mono source is panned with a constant-power law: `cos θ` and `sin θ` with
    /// `θ = (pan + 1)·π/4`, so the centre is -3 dB on each side. A stereo source is
    /// balanced: the far side follows `√2·cos θ` or `√2·sin θ`, limited to 1, so the
    /// centre leaves both channels unchanged. Mono output sums mono sources at unit
    /// gain and averages stereo ones.
    fn gains(&self, frame: u64, out_channels: usize) -> (f64, f64) {
        let gain = self.gain.value(frame) * self.fade.value(frame);
        if out_channels == 1 {
            return if self.channels == 1 {
                (gain, 0.0)
            } else {
                (gain / 2.0, gain / 2.0)
            };
        }
        let turns = (self.pan.value(frame).clamp(-1.0, 1.0) + 1.0) / 8.0;
        let (l, r) = (sin_turns(0.25 - turns), sin_turns(turns));
        if self.channels == 1 {
            (gain * l, gain * r)
        } else {
            let s = std::f64::consts::SQRT_2;
            (gain * (s * l).min(1.0), gain * (s * r).min(1.0))
        }
    }

    /// Returns the next interpolated source frame, or `None` once the voice ended.
    fn next_frame(&mut self) -> Option<(f64, f64)> {
        let index = (self.position >> 32) as i64;
        let frac = self.position & (ONE - 1);
        let direct = self.step == ONE && frac == 0;
        let reach = if direct { 0 } else { HALF };
        if self.end.is_some_and(|end| index - reach >= end) {
            self.state = State::Finished;
            return None;
        }
        if let Err(e) = self.fill(index + HALF + 1) {
            self.state = State::Failed(e.code);
            return None;
        }
        let ch = self.channels;
        let sample = |i: i64, c: usize| -> f64 {
            if i < 0 {
                0.0
            } else {
                self.ring[(i as usize % RING) * ch + c] as f64
            }
        };
        let value = if direct {
            (sample(index, 0), sample(index, ch - 1))
        } else {
            let table = self.table.as_ref().expect("resampling voices have a table");
            let scaled = frac as f64 / ONE as f64 * PHASES as f64;
            let p = (scaled as usize).min(PHASES - 1);
            let t = scaled - p as f64;
            let (a, b) = (&table.rows[p], &table.rows[p + 1]);
            let (mut l, mut r) = (0.0, 0.0);
            for k in 0..16 {
                let h = a[k] + (b[k] - a[k]) * t;
                let i = index + k as i64 - HALF + 1;
                l += h * sample(i, 0);
                r += h * sample(i, ch - 1);
            }
            (l, r)
        };
        self.position += self.step;
        Some(value)
    }

    /// Buffers virtual frames up to `until`, applying the loop and reading ahead.
    fn fill(&mut self, until: i64) -> Result<()> {
        let ch = self.channels;
        while self.filled < until {
            let at = self.filled as usize % RING;
            let space = (RING - at).min(READ);
            let slot = &mut self.ring[at * ch..(at + space) * ch];
            if self.end.is_some() {
                slot.fill(0.0);
                self.filled += space as i64;
                continue;
            }
            let limit = match self.looping {
                Some((_, Some(end))) => (end.saturating_sub(self.source_pos) as usize).min(space),
                _ => space,
            };
            let read = if limit == 0 {
                0
            } else {
                match &mut self.feed {
                    Feed::Memory(audio) => {
                        let data = audio.all_channels();
                        let from = self.source_pos as usize;
                        let n = limit.min(data[0].len().saturating_sub(from));
                        for (c, channel) in data.iter().enumerate() {
                            for (i, &s) in channel[from..from + n].iter().enumerate() {
                                slot[i * ch + c] = s;
                            }
                        }
                        n
                    }
                    Feed::Stream(stream) => stream.read(&mut slot[..limit * ch])?,
                }
            };
            if read > 0 {
                self.source_pos += read as u64;
                self.filled += read as i64;
                continue;
            }
            // The source or the loop region ended.
            match self.looping {
                Some((start, _)) if self.source_pos > start => {
                    if let Feed::Stream(stream) = &mut self.feed {
                        stream.seek(start)?;
                    }
                    self.source_pos = start;
                }
                _ => self.end = Some(self.filled),
            }
        }
        Ok(())
    }
}
