//! Offline linked true-peak limiting. Detection reconstructs eight phases with
//! a normalized 64-tap Blackman-windowed sinc; output remains at its original rate.
use crate::math::{cos_turns, db_to_gain, exp, sin_turns};
use crate::{Audio, Error, Result};
use std::collections::VecDeque;
use std::f64::consts::PI;

const PHASES: usize = 8;
const TAPS: usize = 64;
const HALF: usize = TAPS / 2;

/// Opt-in offline true-peak limiter. The existing [`crate::LimiterOptions`] and
/// [`Audio::limit`] keep their sample-peak behavior.
#[derive(Clone, Copy, Debug)]
pub struct TruePeakLimiterOptions {
    /// Finite nominal ceiling in dBTP (default -1). Its linear value after the
    /// safety margin must be positive and finite.
    pub ceiling_db: f64,
    /// Finite nonnegative lookahead in milliseconds, rounded to input frames and
    /// clamped to the clip length. Default 5. Offline rendering compensates it.
    pub lookahead_ms: f64,
    /// Finite positive exponential release time constant in milliseconds. Default 50.
    pub release_ms: f64,
    /// Extra headroom below the nominal ceiling, 0..=12 dB (default 0.3).
    /// Different reconstruction filters can measure different intersample peaks.
    pub safety_margin_db: f64,
    /// Budget for output PCM/channel vectors, the peak queue and reconstruction
    /// kernel, in bytes (default 1 GiB). Excludes input, metadata and allocator overhead.
    pub max_working_bytes: usize,
}

impl Default for TruePeakLimiterOptions {
    fn default() -> Self {
        Self {
            ceiling_db: -1.0,
            lookahead_ms: 5.0,
            release_ms: 50.0,
            safety_margin_db: 0.3,
            max_working_bytes: 1 << 30,
        }
    }
}

struct Detector {
    // Phase zero is the original sample, included separately and exactly.
    kernel: [[f64; PHASES - 1]; TAPS],
}

impl Detector {
    fn new() -> Self {
        let mut kernel = [[0.0; PHASES - 1]; TAPS];
        for phase in 1..PHASES {
            let mut sum = 0.0;
            for (tap, row) in kernel.iter_mut().enumerate() {
                let t = tap as f64 - (HALF - 1) as f64 - phase as f64 / PHASES as f64;
                let sinc = sin_turns(t / 2.0) / (PI * t);
                let window =
                    0.42 + 0.5 * cos_turns(t / TAPS as f64) + 0.08 * cos_turns(t / HALF as f64);
                row[phase - 1] = sinc * window;
                sum += row[phase - 1];
            }
            for row in &mut kernel {
                row[phase - 1] /= sum;
            }
        }
        Self { kernel }
    }

    // The reconstructed interval [frame, frame+1), zero outside the PCM. The
    // interior path adds all phases side by side without per-tap bounds checks.
    fn peak_at(&self, audio: &Audio, frame: i64) -> f64 {
        let mut peak = 0.0f64;
        for channel in &audio.data {
            if frame >= 0 && (frame as usize) < channel.len() {
                peak = peak.max(channel[frame as usize].abs() as f64);
            }
            let mut sums = [0.0; PHASES - 1];
            let start = frame - (HALF - 1) as i64;
            if start >= 0 && start as usize + TAPS <= channel.len() {
                for (&x, row) in channel[start as usize..start as usize + TAPS]
                    .iter()
                    .zip(&self.kernel)
                {
                    for (sum, &k) in sums.iter_mut().zip(row) {
                        *sum += x as f64 * k;
                    }
                }
            } else {
                for (tap, row) in self.kernel.iter().enumerate() {
                    let at = start + tap as i64;
                    if at >= 0 && (at as usize) < channel.len() {
                        let x = channel[at as usize] as f64;
                        for (sum, &k) in sums.iter_mut().zip(row) {
                            *sum += x * k;
                        }
                    }
                }
            }
            for sum in sums {
                peak = peak.max(sum.abs());
            }
        }
        peak
    }

    fn edges(&self, audio: &Audio) -> (f64, f64) {
        let head = (-(HALF as i64)..0)
            .map(|frame| self.peak_at(audio, frame))
            .fold(0.0, f64::max);
        let end = audio.frames() as i64;
        let tail = (end..end + HALF as i64)
            .map(|frame| self.peak_at(audio, frame))
            .fold(0.0, f64::max);
        (head, tail)
    }

    fn peak(&self, audio: &Audio) -> f64 {
        let (head, tail) = self.edges(audio);
        (0..audio.frames())
            .map(|i| self.peak_at(audio, i as i64))
            .fold(head.max(tail), f64::max)
    }
}

impl Audio {
    /// Limit reconstructed peaks offline, linking gain across every channel.
    ///
    /// Eight-times/64-tap detection drives instantaneous attack, lookahead and
    /// exponential release at the source rate. A final whole-output check against
    /// that reconstruction and the existing four-times estimator can apply an
    /// additional uniform attenuation. The default 0.3 dB safety margin is explicit;
    /// this finite reconstruction is not a proof for every continuous-time signal.
    ///
    /// Output has exactly the input's rate, channels, frame count and leading
    /// alignment. Lookahead and FIR context are read ahead from the complete input;
    /// no leading delay or release frames are added. Zero-extended FIR tails are
    /// checked even though they are not appended to the returned PCM.
    ///
    /// Workspace is checked before allocation. Invalid settings return
    /// [`crate::ErrorCode::InvalidArgument`]; workspace budget/size/reservation failures return
    /// [`crate::ErrorCode::LimitExceeded`]. This is not a realtime callback API.
    pub fn limit_true_peak(&self, options: TruePeakLimiterOptions) -> Result<Self> {
        if !options.ceiling_db.is_finite()
            || !options.lookahead_ms.is_finite()
            || options.lookahead_ms < 0.0
            || !options.release_ms.is_finite()
            || options.release_ms <= 0.0
            || !options.safety_margin_db.is_finite()
            || !(0.0..=12.0).contains(&options.safety_margin_db)
        {
            return Err(Error::invalid("Invalid true-peak limiter settings"));
        }
        let ceiling = db_to_gain(options.ceiling_db - options.safety_margin_db);
        if !ceiling.is_finite() || ceiling <= 0.0 {
            return Err(Error::invalid("Invalid true-peak limiter ceiling"));
        }
        let frames = self.frames();
        if frames as u128 + TAPS as u128 > i64::MAX as u128 {
            return Err(Error::limit("True-peak limiter frame count overflow"));
        }
        let look = (options.lookahead_ms * self.rate as f64 / 1000.0)
            .round()
            .min(frames as f64) as usize;
        let capacity = frames.min(look.saturating_add(1));
        let bytes = frames
            .checked_mul(self.channels())
            .and_then(|v| v.checked_mul(size_of::<f32>()))
            .and_then(|v| v.checked_add(self.channels().checked_mul(size_of::<Vec<f32>>())?))
            .and_then(|v| v.checked_add(capacity.checked_mul(size_of::<(usize, f64)>())?))
            .and_then(|v| v.checked_add(size_of::<Detector>()))
            .ok_or_else(|| Error::limit("True-peak limiter workspace size overflow"))?;
        if bytes > options.max_working_bytes {
            return Err(Error::limit("True-peak limiter exceeds workspace budget"));
        }
        let allocation = |_| Error::limit("Cannot allocate true-peak limiter workspace");
        let mut data = Vec::new();
        data.try_reserve_exact(self.channels())
            .map_err(allocation)?;
        for channel in &self.data {
            let mut copy = Vec::new();
            copy.try_reserve_exact(frames).map_err(allocation)?;
            copy.extend_from_slice(channel);
            data.push(copy);
        }
        let mut peaks = VecDeque::<(usize, f64)>::new();
        peaks.try_reserve_exact(capacity).map_err(allocation)?;
        if frames == 0 {
            return Ok(self.with_data(data));
        }
        let detector = Detector::new();
        let (head, tail) = detector.edges(self);
        // Reserve a few f32 ULPs before the final rounded PCM multiplication.
        let protected = ceiling * (1.0 - 8.0 * f32::EPSILON as f64);
        let release = exp(-1.0 / (options.release_ms * self.rate as f64 / 1000.0));
        let mut gain = 1.0;
        let mut next = 0;
        for i in 0..frames {
            // Expire before adding the next frame, keeping the prepared queue
            // bounded by min(frames, lookahead+1), even for decreasing peaks.
            while peaks.front().is_some_and(|(at, _)| *at < i) {
                peaks.pop_front();
            }
            while next < frames && next <= i.saturating_add(look) {
                let mut peak = detector.peak_at(self, next as i64);
                if next == 0 {
                    peak = peak.max(head);
                }
                if next + 1 == frames {
                    peak = peak.max(tail);
                }
                while peaks.back().is_some_and(|(_, p)| *p <= peak) {
                    peaks.pop_back();
                }
                peaks.push_back((next, peak));
                next += 1;
            }
            let peak = peaks.front().map_or(0.0, |(_, p)| *p);
            let target = if peak > protected {
                protected / peak
            } else {
                1.0
            };
            gain = if target < gain {
                target
            } else {
                target + (gain - target) * release
            };
            for channel in &mut data {
                channel[i] = (channel[i] as f64 * gain) as f32;
            }
        }
        let mut output = self.with_data(data);
        let peak = detector.peak(&output).max(output.render_true_peak());
        if peak > protected {
            let correction = protected / peak;
            for sample in output.data.iter_mut().flatten() {
                *sample = (*sample as f64 * correction) as f32;
            }
        }
        Ok(output)
    }
}
