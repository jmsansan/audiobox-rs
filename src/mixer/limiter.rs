//! A look-ahead peak limiter on the master bus, after the offline `Audio::limit`.
use crate::math::exp2;
use crate::{Error, Result};

/// Settings of the master bus's limiter, enabled with [`MixerConfig::limiter`].
///
/// The output is delayed by `lookahead_ms`, and the gain at each frame comes from the
/// loudest frame within that window, so the gain is already down when a peak leaves
/// the delay: the output never exceeds `ceiling_db`. The gain drops at once and
/// recovers exponentially with a time constant of `release_ms`. A look-ahead of 0
/// adds no latency and still keeps every sample at or below the ceiling, at the cost
/// of a sharper attack. Below the ceiling, the signal passes unchanged.
///
/// [`MixerConfig::limiter`]: super::MixerConfig::limiter
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MasterLimiter {
    /// The highest output level, in dBFS, -24..=0. Default -1.
    pub ceiling_db: f32,
    /// Delay that lets the gain anticipate peaks, 0..=10 ms. Default 0.
    pub lookahead_ms: f32,
    /// Time constant of the gain's recovery, 1..=2000 ms. Default 50, as `Audio::limit`.
    pub release_ms: f32,
}
impl Default for MasterLimiter {
    fn default() -> Self {
        Self {
            ceiling_db: -1.0,
            lookahead_ms: 0.0,
            release_ms: 50.0,
        }
    }
}

pub(super) struct Limiter {
    ceiling: f64,
    /// Per-frame factor of the release's exponential recovery.
    release: f64,
    channels: usize,
    /// Frames of look-ahead plus the current one.
    window: usize,
    /// The last `window` input frames, and their peaks, in a ring.
    delay: Vec<f32>,
    peaks: Vec<f64>,
    at: usize,
    gain: f64,
}

impl Limiter {
    pub fn new(settings: MasterLimiter, rate: u32, channels: usize) -> Result<Self> {
        let MasterLimiter {
            ceiling_db,
            lookahead_ms,
            release_ms,
        } = settings;
        if !(-24.0..=0.0).contains(&ceiling_db)
            || !(0.0..=10.0).contains(&lookahead_ms)
            || !(1.0..=2000.0).contains(&release_ms)
        {
            return Err(Error::invalid(
                "The limiter needs a ceiling of -24..=0 dBFS, a look-ahead of 0..=10 ms and a release of 1..=2000 ms",
            ));
        }
        let lookahead = (lookahead_ms as f64 * rate as f64 / 1000.0).round() as usize;
        let release_frames = release_ms as f64 * rate as f64 / 1000.0;
        Ok(Self {
            ceiling: exp2(ceiling_db as f64 * std::f64::consts::LOG2_10 / 20.0),
            // exp(-1 / frames), with the crate's own exp2.
            release: exp2(-std::f64::consts::LOG2_E / release_frames),
            channels,
            window: lookahead + 1,
            delay: vec![0.0; (lookahead + 1) * channels],
            peaks: vec![0.0; lookahead + 1],
            at: 0,
            gain: 1.0,
        })
    }
    /// Frames by which the limiter delays the output.
    pub fn latency(&self) -> usize {
        self.window - 1
    }
    /// Limits interleaved `out` in place and returns the lowest gain applied.
    pub fn process(&mut self, out: &mut [f32]) -> f64 {
        let ch = self.channels;
        let mut lowest = 1.0f64;
        let ceiling = self.ceiling as f32;
        for frame in out.chunks_exact_mut(ch) {
            // A sum can overflow f32 with extreme gains: hold an infinite sample at full
            // scale and silence a NaN, so the output stays bounded whatever comes in.
            for s in frame.iter_mut() {
                if s.is_nan() {
                    *s = 0.0;
                } else if s.is_infinite() {
                    *s = f32::MAX.copysign(*s);
                }
            }
            let peak = frame.iter().map(|s| s.abs() as f64).fold(0.0, f64::max);
            let slot = self.at;
            self.peaks[slot] = peak;
            // The frame that leaves the delay is the oldest in the window: it sits
            // where the new frame goes. With no look-ahead it is the new frame itself.
            let next = (slot + 1) % self.window;
            let mut delayed = [0.0f32; 2];
            if self.window == 1 {
                delayed[..ch].copy_from_slice(frame);
            } else {
                delayed[..ch].copy_from_slice(&self.delay[next * ch..next * ch + ch]);
                self.delay[slot * ch..slot * ch + ch].copy_from_slice(frame);
            }
            let loudest = self.peaks.iter().copied().fold(0.0, f64::max);
            let target = if loudest > self.ceiling {
                self.ceiling / loudest
            } else {
                1.0
            };
            self.gain = if target < self.gain {
                target
            } else {
                target + (self.gain - target) * self.release
            };
            lowest = lowest.min(self.gain);
            for (o, &d) in frame.iter_mut().zip(&delayed[..ch]) {
                // Rounding can leave a sample a hair above the ceiling; hold it there.
                *o = ((d as f64 * self.gain) as f32).clamp(-ceiling, ceiling);
            }
            self.at = next;
        }
        lowest
    }
}
