//! Amplitude envelopes and the times read from them: attack, decay and release tail.
use super::measurements::reserve;
use crate::{Audio, Error, Limits, Result};

/// How each envelope value summarizes its window of frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvelopeMethod {
    /// The largest absolute sample in the window, over the channels measured.
    Peak,
    /// The root mean square of the window's samples, over the channels measured.
    Rms,
}

/// The envelope [`Audio::envelope`] follows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnvelopeOptions {
    pub method: EnvelopeMethod,
    /// Frames per envelope value, at least 1. Every time read from the envelope has
    /// this resolution. For [`EnvelopeMethod::Peak`] on a tone, a window of at
    /// least one period of its lowest note keeps the envelope from following the
    /// waveform itself.
    pub window_frames: usize,
    /// One channel by index, or `None` for all channels together.
    pub channel: Option<usize>,
}

impl EnvelopeOptions {
    /// An envelope of `method` over all channels, `window_frames` frames per value.
    pub fn new(method: EnvelopeMethod, window_frames: usize) -> Self {
        Self {
            method,
            window_frames,
            channel: None,
        }
    }
}

/// An amplitude envelope: one value per window of [`Envelope::window_frames`]
/// frames, in linear input-sample units.
///
/// Value `i` covers frames `i * window_frames` up to the next window or the end of
/// the input; the last window may be shorter. Times read from the envelope are the
/// starts of windows, so each is exact to one window.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    pub method: EnvelopeMethod,
    pub window_frames: usize,
    pub channel: Option<usize>,
    pub sample_rate: u32,
    /// Frames in the input.
    pub frames: usize,
    pub values: Vec<f64>,
}

/// A time measured on an envelope, between the starts of two windows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnvelopeSpan {
    pub start_frame: usize,
    pub end_frame: usize,
}

impl EnvelopeSpan {
    /// The span's length in frames.
    pub fn frames(&self) -> usize {
        self.end_frame - self.start_frame
    }
    /// The span's length in seconds at `sample_rate`.
    pub fn seconds(&self, sample_rate: u32) -> f64 {
        self.frames() as f64 / sample_rate as f64
    }
}

/// The thresholds [`Envelope::times`] measures with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvelopeThresholds {
    /// The attack runs from this fraction of the peak, above 0...
    pub attack_from: f64,
    /// ...to this one, above `attack_from` and at most 1.
    pub attack_to: f64,
    /// The decay runs from the peak until the envelope has fallen this many dB, above 0.
    pub decay_db: f64,
    /// The frame where the note's gate closed, at most the input's frames: when
    /// given, the tail is measured from it.
    pub gate_off_frame: Option<usize>,
    /// The tail lasts until the envelope stays at or below this level, in dB
    /// relative to the peak, below 0.
    pub tail_floor_db: f64,
}

impl Default for EnvelopeThresholds {
    /// 10 % to 90 % attack, 20 dB decay, no gate and a tail floor 60 dB below the peak.
    fn default() -> Self {
        Self {
            attack_from: 0.1,
            attack_to: 0.9,
            decay_db: 20.0,
            gate_off_frame: None,
            tail_floor_db: -60.0,
        }
    }
}

/// The times of one envelope, with the settings and thresholds they were measured with.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvelopeTimes {
    pub method: EnvelopeMethod,
    pub window_frames: usize,
    pub channel: Option<usize>,
    pub sample_rate: u32,
    pub thresholds: EnvelopeThresholds,
    /// The largest envelope value; 0 for silent or empty input.
    pub peak: f64,
    /// The start of the first window that reaches the peak; `None` for silent or empty input.
    pub peak_frame: Option<usize>,
    /// See [`Envelope::attack`].
    pub attack: Option<EnvelopeSpan>,
    /// See [`Envelope::decay`].
    pub decay: Option<EnvelopeSpan>,
    /// See [`Envelope::tail`]; `None` also when no gate was given.
    pub tail: Option<EnvelopeSpan>,
}

impl Envelope {
    /// The index and value of the first window that reaches the largest value;
    /// `None` when every value is 0.
    fn peak_window(&self) -> Option<(usize, f64)> {
        let mut best: Option<(usize, f64)> = None;
        for (i, &v) in self.values.iter().enumerate() {
            if v > best.map_or(0.0, |(_, b)| b) {
                best = Some((i, v));
            }
        }
        best
    }

    fn span(&self, start: usize, end: usize) -> EnvelopeSpan {
        let at = |i: usize| (i * self.window_frames).min(self.frames);
        EnvelopeSpan {
            start_frame: at(start),
            end_frame: at(end),
        }
    }

    /// The attack: from the first window at or above `from` × the peak to the first
    /// at or above `to` × the peak, with `0 < from < to <= 1`. `None` for silent
    /// or empty input.
    pub fn attack(&self, from: f64, to: f64) -> Result<Option<EnvelopeSpan>> {
        if !(from > 0.0 && from < to && to <= 1.0) {
            return Err(Error::invalid("Attack fractions need 0 < from < to <= 1"));
        }
        let Some((_, peak)) = self.peak_window() else {
            return Ok(None);
        };
        let first = |fraction: f64| self.values.iter().position(|&v| v >= fraction * peak);
        // The peak window reaches both, so both exist.
        Ok(first(from)
            .zip(first(to))
            .map(|(start, end)| self.span(start, end)))
    }

    /// The decay: from the first window that reaches the peak to the first after it
    /// at or below the peak lowered by `drop_db` dB, a finite amount above 0.
    /// `None` when the envelope never falls that far, or for silent or empty input.
    pub fn decay(&self, drop_db: f64) -> Result<Option<EnvelopeSpan>> {
        if !(drop_db.is_finite() && drop_db > 0.0) {
            return Err(Error::invalid("Decay must be a finite drop above 0 dB"));
        }
        let Some((p, peak)) = self.peak_window() else {
            return Ok(None);
        };
        let level = peak * super::from_db(-drop_db);
        Ok(self.values[p + 1..]
            .iter()
            .position(|&v| v <= level)
            .map(|i| self.span(p, p + 1 + i)))
    }

    /// The release tail: from `gate_off_frame` (at most the input's frames) until the
    /// envelope stays at or below `floor_db`, a finite level below 0 dB relative to
    /// the peak, to the end of the input. It ends at the end of the last window
    /// above the floor; it is empty when no window from the gate on is above it.
    /// `None` when the last window is still above the floor, or for silent or
    /// empty input.
    pub fn tail(&self, gate_off_frame: usize, floor_db: f64) -> Result<Option<EnvelopeSpan>> {
        if gate_off_frame > self.frames {
            return Err(Error::invalid(
                "Gate-off frame is past the end of the input",
            ));
        }
        if !(floor_db.is_finite() && floor_db < 0.0) {
            return Err(Error::invalid(
                "Tail floor must be a finite level below 0 dB relative to the peak",
            ));
        }
        let Some((_, peak)) = self.peak_window() else {
            return Ok(None);
        };
        let floor = peak * super::from_db(floor_db);
        let gate_window = gate_off_frame / self.window_frames;
        let end = match self.values.iter().rposition(|&v| v > floor) {
            Some(last) if last + 1 == self.values.len() => return Ok(None),
            Some(last) if last >= gate_window => {
                ((last + 1) * self.window_frames).max(gate_off_frame)
            }
            _ => gate_off_frame,
        };
        Ok(Some(EnvelopeSpan {
            start_frame: gate_off_frame,
            end_frame: end,
        }))
    }

    /// The peak, attack, decay and tail at `thresholds`, which are checked first.
    pub fn times(&self, thresholds: &EnvelopeThresholds) -> Result<EnvelopeTimes> {
        let attack = self.attack(thresholds.attack_from, thresholds.attack_to)?;
        let decay = self.decay(thresholds.decay_db)?;
        let tail = match thresholds.gate_off_frame {
            Some(gate) => self.tail(gate, thresholds.tail_floor_db)?,
            None => {
                // Checked even without a gate, so that a bad floor is not silent.
                self.tail(self.frames, thresholds.tail_floor_db)?;
                None
            }
        };
        let peak = self.peak_window();
        Ok(EnvelopeTimes {
            method: self.method,
            window_frames: self.window_frames,
            channel: self.channel,
            sample_rate: self.sample_rate,
            thresholds: *thresholds,
            peak: peak.map_or(0.0, |(_, v)| v),
            peak_frame: peak.map(|(i, _)| self.span(i, i).start_frame),
            attack,
            decay,
            tail,
        })
    }
}

impl Audio {
    /// The amplitude envelope of one channel or all of them, as `options` say.
    ///
    /// With all channels, [`EnvelopeMethod::Peak`] takes the largest absolute sample
    /// of any channel and [`EnvelopeMethod::Rms`] the root mean square of every
    /// channel's samples together. Samples must be finite and `window_frames` at
    /// least 1; a channel must exist. Default [`Limits`] apply. The work is
    /// O(samples) and the storage one value per window. The result is the same on
    /// every host: sums run in a fixed order and square roots are exactly rounded.
    ///
    /// ```
    /// use audiobox::analyze::{EnvelopeMethod, EnvelopeOptions, EnvelopeThresholds};
    ///
    /// // A ramp up to full level over 0.1 s, held for 0.1 s.
    /// let samples: Vec<f32> = (0..9600).map(|i| (i as f32 / 4800.0).min(1.0)).collect();
    /// let note = audiobox::Audio::new(vec![samples], 48_000)?;
    /// let envelope = note.envelope(&EnvelopeOptions::new(EnvelopeMethod::Peak, 48))?;
    /// let times = envelope.times(&EnvelopeThresholds::default())?;
    /// // 10 % to 90 % of the ramp: 80 ms, to one 1 ms window.
    /// let attack = times.attack.unwrap().seconds(times.sample_rate);
    /// assert!((attack - 0.080).abs() <= 0.001);
    /// # Ok::<(), audiobox::Error>(())
    /// ```
    pub fn envelope(&self, options: &EnvelopeOptions) -> Result<Envelope> {
        let w = options.window_frames;
        if w == 0 {
            return Err(Error::invalid("Envelope window must be at least one frame"));
        }
        Limits::default().check(self.frames(), self.channels(), self.rate)?;
        let channels: &[Vec<f32>] = match options.channel {
            Some(c) => self
                .data
                .get(c..=c)
                .ok_or_else(|| Error::invalid("Envelope channel does not exist"))?,
            None => &self.data,
        };
        if channels.iter().flatten().any(|s| !s.is_finite()) {
            return Err(Error::invalid("Envelope samples must be finite"));
        }
        let frames = self.frames();
        let mut values = reserve(frames.div_ceil(w))?;
        for start in (0..frames).step_by(w) {
            let end = (start + w).min(frames);
            let value = match options.method {
                EnvelopeMethod::Peak => channels
                    .iter()
                    .flat_map(|c| &c[start..end])
                    .fold(0.0f64, |m, &s| m.max((s as f64).abs())),
                EnvelopeMethod::Rms => {
                    let sum: f64 = channels
                        .iter()
                        .flat_map(|c| &c[start..end])
                        .map(|&s| s as f64 * s as f64)
                        .sum();
                    (sum / ((end - start) * channels.len()) as f64).sqrt()
                }
            };
            values.push(value);
        }
        Ok(Envelope {
            method: options.method,
            window_frames: w,
            channel: options.channel,
            sample_rate: self.rate,
            frames,
            values,
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
            let options = EnvelopeOptions::new(EnvelopeMethod::Rms, 2);
            assert_eq!(
                a.envelope(&options).unwrap_err().code,
                crate::ErrorCode::InvalidArgument
            );
        }
    }
}
