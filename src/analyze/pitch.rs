//! Monophonic pitch tracks, by the YIN method.
use super::measurements::reserve;
use crate::{Audio, Error, Limits, Result};
use std::ops::Range;

/// The most analysis frames one [`Audio::pitch_track`] measures.
pub const MAX_PITCH_FRAMES: usize = 1 << 20;
/// The longest period, in frames, a pitch track looks for: `min_hz` is at least
/// `sample_rate / MAX_PITCH_LAG`.
pub const MAX_PITCH_LAG: usize = 8192;
/// The widest median smoothing, in frames.
pub const MAX_PITCH_MEDIAN: usize = 15;
/// The most work one [`Audio::pitch_track`] is allowed, as an estimate: analysis
/// frames × L × (log₂ L + 1), with L the transform size, the power of two at or above two
/// periods of `min_hz` in frames. Every frame counts, silent or not.
pub const MAX_PITCH_WORK: u64 = 1 << 32;

/// How [`Audio::pitch_track`] looks for a pitch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchOptions {
    /// The lowest pitch looked for, in Hz, above 0 and at least
    /// `sample_rate / MAX_PITCH_LAG`. It sets the frame's length: two periods of it.
    pub min_hz: f64,
    /// The highest pitch looked for, in Hz, above `min_hz` and at most half the
    /// sample rate.
    pub max_hz: f64,
    /// Frames between the starts of analysis frames, at least 1.
    pub hop: usize,
    /// How aperiodic a frame may be and still be voiced, from 0 to 1, exclusive:
    /// YIN's absolute threshold on the normalized difference. Lower is stricter.
    pub threshold: f64,
    /// A frame whose RMS is at or below this level, in dBFS (finite), is silent.
    pub silence_db: f64,
    /// One channel by index, or `None` for the mean of all channels.
    pub channel: Option<usize>,
    /// Median smoothing of voiced pitches over this many frames: odd, from 1 (none)
    /// to [`MAX_PITCH_MEDIAN`].
    pub median: usize,
}

impl PitchOptions {
    /// Pitches from `min_hz` to `max_hz`, a frame every `hop` frames, threshold
    /// 0.15, silent at or below −60 dBFS, all channels and no smoothing.
    pub fn new(min_hz: f64, max_hz: f64, hop: usize) -> Self {
        Self {
            min_hz,
            max_hz,
            hop,
            threshold: 0.15,
            silence_db: -60.0,
            channel: None,
            median: 1,
        }
    }
}

/// What a frame of a pitch track holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PitchState {
    /// Periodic enough to have a pitch.
    Voiced,
    /// Sound without a clear period, such as noise, a chord or a transition.
    Unvoiced,
    /// At or below the silence level.
    Silent,
}

/// A period that a frame's sound repeats at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchCandidate {
    pub frequency_hz: f64,
    /// 1 minus YIN's normalized difference at the period, from 0 to 1: 1 for a
    /// sound that repeats exactly.
    pub confidence: f64,
}

/// One analysis frame of a pitch track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchFrame {
    /// The input frame where the analysis frame starts; it reads
    /// [`PitchTrack::span_frames`] frames from there.
    pub start_frame: usize,
    pub state: PitchState,
    /// The pitch of a voiced frame; `None` otherwise, never a guess.
    pub frequency_hz: Option<f64>,
    /// The confidence of the best period found, voiced or not; 0 when silent.
    pub confidence: f64,
    /// The deepest other dip of a voiced frame in the range, more than a semitone
    /// from the pitch, as the octave above or below: when its confidence is close
    /// to the pitch's, the octave is ambiguous. Multiples of the period, which any
    /// periodic sound repeats at, count only when clearly deeper, and of equally
    /// deep dips the shortest period is given. `None` when the
    /// frame is not voiced or has no other dip. With smoothing, it and the
    /// confidence still describe the frame's own estimate.
    pub alternative: Option<PitchCandidate>,
}

/// A monophonic pitch track, with the settings it was measured with.
#[derive(Clone, Debug, PartialEq)]
pub struct PitchTrack {
    pub options: PitchOptions,
    pub sample_rate: u32,
    /// Frames in the input.
    pub input_frames: usize,
    /// Frames each analysis frame reads: two periods of `min_hz`. A pitch describes
    /// that span, so it lags the sound by up to that much.
    pub span_frames: usize,
    pub frames: Vec<PitchFrame>,
}

impl PitchTrack {
    /// The median pitch of the voiced frames that start in `range`, which lies
    /// within the input; `None` without one.
    pub fn median_frequency(&self, range: Range<usize>) -> Result<Option<f64>> {
        if range.start > range.end || range.end > self.input_frames {
            return Err(Error::invalid("Pitch range must lie within the input"));
        }
        let mut pitches: Vec<f64> = self
            .frames
            .iter()
            .filter(|f| range.contains(&f.start_frame))
            .filter_map(|f| f.frequency_hz)
            .collect();
        Ok(median(&mut pitches))
    }
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let n = values.len();
    Some(if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    })
}

/// The samples a pitch track reads: one channel as it is, or several mixed.
enum Signal<'a> {
    One(&'a [f32]),
    Mixed(&'a [f64]),
}

impl Signal<'_> {
    fn at(&self, i: usize) -> f64 {
        match self {
            Signal::One(s) => s[i] as f64,
            Signal::Mixed(s) => s[i],
        }
    }
}

/// The FFT of `real` + i `imag` in place, inverse when `inverse`, unscaled.
fn transform(real: &mut [f64], imag: &mut [f64], inverse: bool) {
    if inverse {
        imag.iter_mut().for_each(|v| *v = -*v);
    }
    super::fft(real, imag);
    if inverse {
        imag.iter_mut().for_each(|v| *v = -*v);
    }
}

/// YIN's cumulative mean normalized difference of `x` for lags 0..=max_lag, over an
/// integration window of `window` samples: `x` holds `window + max_lag` samples.
struct Yin {
    window: usize,
    max_lag: usize,
    size: usize,
    a_real: Vec<f64>,
    a_imag: Vec<f64>,
    b_real: Vec<f64>,
    b_imag: Vec<f64>,
    squares: Vec<f64>,
    normalized: Vec<f64>,
}

impl Yin {
    fn new(window: usize, max_lag: usize) -> Self {
        let size = (window + max_lag).next_power_of_two();
        Self {
            window,
            max_lag,
            size,
            a_real: vec![0.0; size],
            a_imag: vec![0.0; size],
            b_real: vec![0.0; size],
            b_imag: vec![0.0; size],
            squares: vec![0.0; window + max_lag + 1],
            normalized: vec![1.0; max_lag + 1],
        }
    }

    fn run(&mut self, x: &[f64]) {
        let (w, n) = (self.window, self.size);
        // r(tau) = sum over j < w of x[j] x[j + tau], by correlation through the FFT.
        // The buffers are at least w + max_lag long, so the lags do not wrap.
        self.a_real.fill(0.0);
        self.a_imag.fill(0.0);
        self.b_real.fill(0.0);
        self.b_imag.fill(0.0);
        self.a_real[..w].copy_from_slice(&x[..w]);
        self.b_real[..x.len()].copy_from_slice(x);
        transform(&mut self.a_real, &mut self.a_imag, false);
        transform(&mut self.b_real, &mut self.b_imag, false);
        for k in 0..n {
            let (ar, ai) = (self.a_real[k], -self.a_imag[k]);
            let (br, bi) = (self.b_real[k], self.b_imag[k]);
            self.b_real[k] = ar * br - ai * bi;
            self.b_imag[k] = ar * bi + ai * br;
        }
        transform(&mut self.b_real, &mut self.b_imag, true);
        // squares[j] = sum of x[i]^2 for i < j.
        self.squares[0] = 0.0;
        for (j, v) in x.iter().enumerate() {
            self.squares[j + 1] = self.squares[j] + v * v;
        }
        let energy = |from: usize| self.squares[from + w] - self.squares[from];
        let e0 = energy(0);
        let mut running = 0.0;
        self.normalized[0] = 1.0;
        for tau in 1..=self.max_lag {
            let r = self.b_real[tau] / n as f64;
            let d = (e0 + energy(tau) - 2.0 * r).max(0.0);
            running += d;
            self.normalized[tau] = if running > 0.0 {
                d * tau as f64 / running
            } else {
                1.0
            };
        }
    }

    /// The lag and depth of a local minimum at `tau`, refined between samples by
    /// the parabola through it and its neighbours.
    fn bottom(&self, tau: usize) -> (f64, f64) {
        let d = &self.normalized;
        if tau < 1 || tau + 1 > self.max_lag {
            return (tau as f64, d[tau]);
        }
        let (a, b, c) = (d[tau - 1], d[tau], d[tau + 1]);
        let curve = a - 2.0 * b + c;
        if curve > 0.0 {
            let shift = ((a - c) / (2.0 * curve)).clamp(-0.5, 0.5);
            let depth = b - 0.25 * (a - c) * shift;
            (tau as f64 + shift, depth.clamp(0.0, b))
        } else {
            (tau as f64, b)
        }
    }

    /// Whether `tau` is a local minimum with both neighbours computed.
    fn is_dip(&self, tau: usize) -> bool {
        let d = &self.normalized;
        tau >= 1 && tau < self.max_lag && d[tau] <= d[tau - 1] && d[tau] < d[tau + 1]
    }
}

impl Audio {
    /// A monophonic pitch track: per analysis frame, the pitch, how periodic the
    /// sound is, and the best other period, by the YIN method (de Cheveigné and
    /// Kawahara, 2002).
    ///
    /// Frame `i` starts at input frame `i * hop` and reads two periods of
    /// `min_hz`: the first period is compared with the samples up to one period
    /// later. Only whole frames are measured, so the last frames of input may fall
    /// in none; an input shorter than one frame is a single frame, zero-padded.
    /// A frame is silent when the RMS of its first period, over the samples it
    /// holds and with their mean removed, is at or below `silence_db`: a DC offset
    /// or a level stuck at one value is silence. A frame is voiced when YIN's
    /// normalized difference has a dip below `threshold` whose period, refined
    /// between samples, lies in the range (within 5 cents of an end, and then
    /// set to it); the pitch is the first such dip. After a dip above the range,
    /// its multiples count only when clearly deeper, so a tone above `max_hz`
    /// has no pitch rather than a subharmonic. Other frames have no pitch. With
    /// all channels, the mean of the channels is tracked.
    ///
    /// It is a tool for one voice: in a chord or a mix it finds the period the
    /// sounds repeat at together, if any, or none, and it can confuse octaves
    /// (see [`PitchFrame::alternative`]). Note names, the choice of a part and
    /// melody extraction are the caller's.
    ///
    /// Bad options, a missing channel and non-finite samples return
    /// `InvalidArgument`. Default [`Limits`] apply, plus at most
    /// [`MAX_PITCH_FRAMES`] frames and [`MAX_PITCH_WORK`] for frames × L × (log₂ L + 1), or
    /// `LimitExceeded`, with L the power of two at or above two periods of
    /// `min_hz` in frames. The work is O(samples × channels) to mix the channels once, plus O(frames × L log L); silent
    /// frames skip the transforms. The storage is O(L) plus one entry per frame, and with several channels one f64 per input frame. The FFT uses the platform's math, so results are
    /// for reports, not bit-identical across hosts.
    ///
    /// ```
    /// use audiobox::analyze::{PitchOptions, PitchState};
    ///
    /// // A 220 Hz sine, half a second.
    /// let sine: Vec<f32> = (0..24_000)
    ///     .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 48_000.0).sin())
    ///     .collect();
    /// let audio = audiobox::Audio::new(vec![sine], 48_000)?;
    /// let track = audio.pitch_track(&PitchOptions::new(60.0, 1000.0, 480))?;
    /// assert_eq!(track.frames[10].state, PitchState::Voiced);
    /// let pitch = track.median_frequency(0..24_000)?.unwrap();
    /// assert!((pitch - 220.0).abs() < 0.5, "{pitch}");
    /// # Ok::<(), audiobox::Error>(())
    /// ```
    pub fn pitch_track(&self, options: &PitchOptions) -> Result<PitchTrack> {
        let o = *options;
        let rate = self.rate as f64;
        if !(o.min_hz.is_finite() && o.min_hz > 0.0 && o.min_hz * MAX_PITCH_LAG as f64 >= rate) {
            return Err(Error::invalid(
                "Lowest pitch must be finite, above 0 and at least the sample rate / 8192",
            ));
        }
        if !(o.max_hz > o.min_hz && o.max_hz <= rate / 2.0) {
            return Err(Error::invalid(
                "Highest pitch must be above the lowest and at most half the sample rate",
            ));
        }
        if o.hop == 0 {
            return Err(Error::invalid("Pitch hop must be at least one frame"));
        }
        if !(o.threshold > 0.0 && o.threshold < 1.0) {
            return Err(Error::invalid("Pitch threshold must lie between 0 and 1"));
        }
        if !o.silence_db.is_finite() {
            return Err(Error::invalid("Silence level must be finite"));
        }
        if o.median % 2 == 0 || o.median > MAX_PITCH_MEDIAN {
            return Err(Error::invalid("Pitch median must be odd, from 1 to 15"));
        }
        Limits::default().check(self.frames(), self.channels(), self.rate)?;
        let channels: &[Vec<f32>] = match o.channel {
            Some(c) => self
                .data
                .get(c..=c)
                .ok_or_else(|| Error::invalid("Pitch channel does not exist"))?,
            None => &self.data,
        };
        if channels.iter().flatten().any(|s| !s.is_finite()) {
            return Err(Error::invalid("Pitch samples must be finite"));
        }
        let input_frames = self.frames();
        let max_lag = (rate / o.min_hz).ceil() as usize;
        let min_lag = ((rate / o.max_hz).floor() as usize).max(2);
        let window = max_lag - 1;
        // One lag more on each side than the range needs, so that a dip at either
        // end of it is a local minimum with both neighbours.
        let top = max_lag + 1;
        let span = window + top;
        // Whole frames only, so that no frame is padded with silence; an input
        // shorter than one frame is one padded frame.
        let count = match input_frames {
            0 => 0,
            n if n < span => 1,
            n => (n - span) / o.hop + 1,
        };
        if count > MAX_PITCH_FRAMES {
            return Err(Error::limit("Too many pitch analysis frames"));
        }
        let size = span.next_power_of_two();
        // In u64, so that the limit is exact on 32-bit targets too. Each frame
        // reads its span and runs three transforms of `size`.
        if (count as u64)
            .checked_mul(size as u64)
            .and_then(|n| n.checked_mul(u64::from(size.trailing_zeros()) + 1))
            .is_none_or(|n| n > MAX_PITCH_WORK)
        {
            return Err(Error::limit("Pitch analysis exceeds its work limit"));
        }
        // The channels' mean, once, so that overlapping frames do not mix it again.
        let mixed: Vec<f64>;
        let signal: Signal = match channels {
            [one] => Signal::One(one),
            _ => {
                let mut mix = reserve(input_frames)?;
                let n = channels.len() as f64;
                mix.extend(
                    (0..input_frames)
                        .map(|i| channels.iter().map(|c| c[i] as f64).sum::<f64>() / n),
                );
                mixed = mix;
                Signal::Mixed(&mixed)
            }
        };
        let mut yin = Yin::new(window, top);
        let (lo, hi) = (min_lag - 1, max_lag);
        // The range, with a margin of the estimate's accuracy (5 cents): a pitch
        // estimated just past an end is set to it.
        let margin = 2f64.powf(5.0 / 1200.0);
        let in_range = |lag: f64| (o.min_hz / margin..=o.max_hz * margin).contains(&(rate / lag));
        let pitch = |lag: f64| (rate / lag).clamp(o.min_hz, o.max_hz);
        let mut x = vec![0.0; span];
        let silence = super::from_db(o.silence_db);
        let semitone = 2f64.powf(1.0 / 12.0);
        let mut frames = reserve(count)?;
        for start in (0..count).map(|i| i * o.hop) {
            let end = (start + span).min(input_frames);
            for (j, v) in x.iter_mut().enumerate() {
                *v = if start + j < end {
                    signal.at(start + j)
                } else {
                    0.0
                };
            }
            // The silence gate reads the first period with its mean removed, so
            // that a DC offset or a level stuck at one value counts as silence.
            let heard = (start + window).min(input_frames) - start;
            let mean = x[..heard].iter().sum::<f64>() / heard as f64;
            let variance = x[..heard]
                .iter()
                .map(|v| (v - mean) * (v - mean))
                .sum::<f64>()
                / heard as f64;
            let mut frame = PitchFrame {
                start_frame: start,
                state: PitchState::Silent,
                frequency_hz: None,
                confidence: 0.0,
                alternative: None,
            };
            if variance > silence * silence {
                yin.run(&x);
                let d = &yin.normalized;
                // Two periods related by a whole multiple, within a semitone.
                let near_multiple = |long: f64, short: f64| {
                    let ratio = long / short;
                    let k = ratio.round();
                    k >= 2.0 && (ratio / k).max(k / ratio) < semitone
                };
                // The first dip below the threshold whose period lies in the range.
                // A sound that repeats every period also repeats every two or three,
                // so after a dip outside the range (a tone above `max_hz`) its
                // multiples count only when clearly deeper: a weak fundamental under
                // a strong harmonic, not the same tone again.
                let mut outside: Option<(f64, f64)> = None;
                let mut chosen: Option<(f64, f64)> = None;
                // From the shortest lag, so that a dip above the range is seen.
                let mut t = 1;
                while t <= hi {
                    if d[t] < o.threshold {
                        // Down to the bottom of the dip.
                        while t <= hi && d[t + 1] < d[t] {
                            t += 1;
                        }
                        if t > hi {
                            break;
                        }
                        if yin.is_dip(t) {
                            let (lag, depth) = yin.bottom(t);
                            if in_range(lag) {
                                if outside.is_none_or(|(l, dd)| {
                                    !near_multiple(lag, l) || depth < dd - 0.01
                                }) {
                                    chosen = Some((lag, depth));
                                    break;
                                }
                            } else if rate / lag < o.min_hz {
                                break;
                            } else {
                                outside.get_or_insert((lag, depth));
                            }
                        }
                    }
                    t += 1;
                }
                let dips = || {
                    (lo..=hi)
                        .filter(|&t| yin.is_dip(t))
                        .map(|t| yin.bottom(t))
                        .filter(|&(l, _)| in_range(l))
                };
                if let Some((lag, depth)) = chosen {
                    frame.state = PitchState::Voiced;
                    frame.frequency_hz = Some(pitch(lag));
                    frame.confidence = (1.0 - depth).clamp(0.0, 1.0);
                    // The deepest other dip in the range more than a semitone away;
                    // multiples of the period count only when clearly deeper, and
                    // so do multiples of the alternative's own period.
                    let others = || {
                        dips().filter(|&(l, dd)| {
                            let ratio = l / lag;
                            (ratio > semitone || ratio < 1.0 / semitone)
                                && (!near_multiple(l, lag) || dd < depth - 0.01)
                        })
                    };
                    frame.alternative = others()
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(l, dd)| {
                            others()
                                .find(|&(s, sd)| (s == l || near_multiple(l, s)) && sd < dd + 0.01)
                                .unwrap_or((l, dd))
                        })
                        .map(|(l, dd)| PitchCandidate {
                            frequency_hz: pitch(l),
                            confidence: (1.0 - dd).clamp(0.0, 1.0),
                        });
                } else {
                    frame.state = PitchState::Unvoiced;
                    // How periodic the frame is at its best period in the range.
                    let best = dips().map(|(_, dd)| dd).fold(
                        (min_lag..=max_lag).map(|t| d[t]).fold(1.0, f64::min),
                        f64::min,
                    );
                    frame.confidence = (1.0 - best).clamp(0.0, 1.0);
                }
            }
            frames.push(frame);
        }
        if o.median > 1 {
            let raw: Vec<Option<f64>> = frames.iter().map(|f| f.frequency_hz).collect();
            let half = o.median / 2;
            let mut near = Vec::with_capacity(o.median);
            for (i, frame) in frames.iter_mut().enumerate() {
                if raw[i].is_some() {
                    near.clear();
                    near.extend(
                        raw[i.saturating_sub(half)..(i + half + 1).min(raw.len())]
                            .iter()
                            .flatten(),
                    );
                    frame.frequency_hz = median(&mut near);
                }
            }
        }
        Ok(PitchTrack {
            options: o,
            sample_rate: self.rate,
            input_frames,
            span_frames: span,
            frames,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nonfinite_samples_are_refused() {
        let a = Audio::new(vec![vec![0.0; 3]], 48000).unwrap();
        for value in [f32::NAN, f32::INFINITY] {
            let a = a.with_data(vec![vec![0.0, value, 0.0]]);
            assert_eq!(
                a.pitch_track(&PitchOptions::new(100.0, 1000.0, 10))
                    .unwrap_err()
                    .code,
                crate::ErrorCode::InvalidArgument
            );
        }
    }
}
