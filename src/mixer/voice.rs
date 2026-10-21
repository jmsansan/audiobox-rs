use std::sync::Arc;

use super::{Loop, Stream, VoiceId};
use crate::math::{exp2, sin_turns};
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
    /// The frame where the ramp reaches its target.
    pub fn end(&self) -> u64 {
        self.start + self.len
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

/// Ratios of the interpolation tables shared by voices whose playback speed differs
/// from 1. Each covers speeds up to its ratio of source to output frames.
const BANK: [f64; 9] = [1.0, 1.25, 1.5, 2.0, 2.5, 3.0, 4.0, 6.0, 8.0];

/// Interpolation tables for changing playback speeds, built once when the mixer is
/// made, since `render` cannot build tables. A voice uses the table of the smallest
/// ratio at or above its current one, whose cutoff is low enough not to alias.
#[derive(Debug)]
pub(super) struct SincBank {
    tables: Vec<SincTable>,
}
impl SincBank {
    pub fn new() -> Self {
        Self {
            tables: BANK.iter().map(|&ratio| SincTable::new(ratio)).collect(),
        }
    }
    fn index(ratio: f64) -> usize {
        BANK.iter()
            .position(|&r| ratio <= r)
            .unwrap_or(BANK.len() - 1)
    }
}

/// How a voice reads its source at speed 1, and the tables for other speeds.
pub(super) struct Resampling {
    /// Fixed-point source frames per output frame.
    pub step: u64,
    /// Source frames per output frame.
    pub base_ratio: f64,
    /// The exact table for `base_ratio`, or `None` at the output rate.
    pub table: Option<Arc<SincTable>>,
    pub bank: Arc<SincBank>,
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
    /// Fixed-point source frames per output frame, now and at speed 1.
    step: u64,
    base_step: u64,
    /// Source frames per output frame at speed 1.
    base_ratio: f64,
    /// The exact table for speed 1, or `None` when the source is at the output rate.
    table: Option<Arc<SincTable>>,
    bank: Arc<SincBank>,
    /// The bank table in use while the speed differs from 1.
    bank_index: Option<usize>,
    /// Playback rate, and pitch in semitones: together, the playback speed.
    pub rate: Ramp,
    pub pitch: Ramp,
    /// Output frames during which the voice was frozen by a pause. The voice's own
    /// time, which its ramps follow, is the output frame minus this.
    offset: u64,
    /// Gain of pause and resume fades, in output frames.
    pause: Ramp,
    /// Fading out towards a pause, or paused.
    pausing: bool,
    paused: bool,
    /// Virtual frames where the source restarted, and the source frame it restarted
    /// at, to map the playback position back to source frames across loops.
    jumps: [(i64, u64); JUMPS],
    jump_count: usize,
}

/// Loop restarts remembered for mapping positions; enough for loops longer than the
/// voice's buffer, and an approximation for shorter ones.
const JUMPS: usize = 16;

impl Voice {
    pub fn new(
        id: VoiceId,
        feed: Feed,
        channels: usize,
        looping: Loop,
        resampling: Resampling,
    ) -> Self {
        let Resampling {
            step,
            base_ratio,
            table,
            bank,
        } = resampling;
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
            base_step: step,
            base_ratio,
            table,
            bank,
            bank_index: None,
            rate: Ramp::constant(1.0),
            pitch: Ramp::constant(0.0),
            offset: 0,
            pause: Ramp::constant(1.0),
            pausing: false,
            paused: false,
            jumps: [(0, 0); JUMPS],
            jump_count: 1,
        }
    }
    /// Starts the source at `frame`. Runs on the game's thread, where a stream may seek.
    pub fn set_start(&mut self, frame: u64) -> Result<()> {
        if frame > 0 {
            if let Feed::Stream(stream) = &mut self.feed {
                stream.seek(frame)?;
            }
        }
        self.source_pos = frame;
        self.jumps[0] = (0, frame);
        Ok(())
    }
    /// The voice's own time at output frame `frame`, which stops while it is paused.
    pub fn time(&self, frame: u64) -> u64 {
        frame - self.offset
    }
    /// Whether the voice is frozen by its own pause.
    pub fn paused(&self) -> bool {
        self.paused
    }
    /// Advances the frozen time by `frames` output frames, while the voice or its bus is paused.
    pub fn freeze(&mut self, frames: u64) {
        self.offset += frames;
    }
    pub fn pause(&mut self, frame: u64, len: u64, curve: FadeCurve) {
        if self.done() || self.paused {
            return;
        }
        if len == 0 {
            self.pause = Ramp::constant(0.0);
            self.paused = true;
            self.pausing = false;
        } else {
            self.pause.retarget(frame, 0.0, len, curve);
            self.pausing = true;
        }
    }
    pub fn resume(&mut self, frame: u64, len: u64, curve: FadeCurve) {
        if self.done() || !(self.paused || self.pausing) {
            return;
        }
        self.paused = false;
        self.pausing = false;
        self.pause.retarget(frame, 1.0, len, curve);
    }
    /// The source frame being played, mapped back through loop restarts.
    pub fn source_position(&self) -> u64 {
        let index = (self.position >> 32) as i64;
        let count = self.jump_count.min(JUMPS);
        let latest = (0..count)
            .map(|k| self.jumps[(self.jump_count - 1 - k) % JUMPS])
            .find(|&(at, _)| at <= index)
            .unwrap_or(self.jumps[(self.jump_count - count) % JUMPS]);
        latest.1 + index.saturating_sub(latest.0).max(0) as u64
    }
    /// Sets the step and table for the playback speed at `frame`. At speed 1 the voice
    /// uses its exact step and table, so unpitched voices render as before.
    fn update_speed(&mut self, frame: u64) {
        let rate = self.rate.value(frame);
        let pitch = self.pitch.value(frame);
        if rate == 1.0 && pitch == 0.0 {
            self.step = self.base_step;
            self.bank_index = None;
            return;
        }
        let speed = (rate * exp2(pitch / 12.0)).clamp(1.0 / 64.0, 64.0);
        self.step = ((self.base_step as f64 * speed).round() as u64).max(1);
        self.bank_index = Some(SincBank::index(self.base_ratio * speed));
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
        // A paused voice is silent and frozen, so it stops at once.
        if len == 0 || self.paused {
            self.state = State::Finished;
        } else {
            self.fade.retarget(frame, 0.0, len, curve);
            self.state = State::Stopping;
        }
    }

    /// Adds `frames` frames from output frame `first` into `out`, interleaved with
    /// `out_channels` channels. A pause that completes freezes the rest of the block.
    pub fn render(&mut self, first: u64, frames: usize, out: &mut [f32], out_channels: usize) {
        let local = self.time(first);
        // Ramps only start at segment boundaries, so one finished at `first` stays put.
        let constant = !self.gain.changing(local)
            && !self.fade.changing(local)
            && !self.pan.changing(local)
            && !self.pause.changing(first);
        let fixed = constant.then(|| self.gains(local, first, out_channels));
        let speeding = self.rate.changing(local) || self.pitch.changing(local);
        self.update_speed(local);
        for j in 0..frames {
            if self.done() {
                return;
            }
            let frame = first + j as u64;
            let time = local + j as u64;
            if speeding {
                self.update_speed(time);
            }
            let Some((left, right)) = self.next_frame() else {
                return;
            };
            let (gl, gr) = fixed.unwrap_or_else(|| self.gains(time, frame, out_channels));
            if out_channels == 1 {
                out[j] += (left * gl + right * gr) as f32;
            } else {
                out[2 * j] += (left * gl) as f32;
                out[2 * j + 1] += (right * gr) as f32;
            }
            if self.state == State::Stopping && self.fade.finished(time + 1) {
                self.state = State::Finished;
            }
            if self.pausing && self.pause.finished(frame + 1) {
                self.pausing = false;
                self.paused = true;
                self.freeze((frames - j - 1) as u64);
                return;
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
    fn gains(&self, time: u64, frame: u64, out_channels: usize) -> (f64, f64) {
        let gain = self.gain.value(time) * self.fade.value(time) * self.pause.value(frame);
        if out_channels == 1 {
            return if self.channels == 1 {
                (gain, 0.0)
            } else {
                (gain / 2.0, gain / 2.0)
            };
        }
        let turns = (self.pan.value(time).clamp(-1.0, 1.0) + 1.0) / 8.0;
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
            let bank = &self.bank.tables;
            let table = match self.bank_index {
                Some(i) => &bank[i],
                // A voice at the output rate whose position is between frames after a
                // speed change uses the table for ratio 1.
                None => self.table.as_deref().unwrap_or(&bank[0]),
            };
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
                    self.jumps[self.jump_count % JUMPS] = (self.filled, start);
                    self.jump_count += 1;
                }
                _ => self.end = Some(self.filled),
            }
        }
        Ok(())
    }
}
