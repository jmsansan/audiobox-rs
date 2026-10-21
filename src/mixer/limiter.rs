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
    /// The last `window` input frames, in a ring.
    delay: Vec<f32>,
    at: usize,
    gain: f64,
    /// A monotonic queue of (frame, peak) over the window, as `Audio::limit` keeps:
    /// peaks decrease from front to back, so the front is the window's maximum.
    queue: Vec<(u64, f64)>,
    queue_head: usize,
    queue_len: usize,
    frames: u64,
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
            at: 0,
            gain: 1.0,
            queue: vec![(0, 0.0); lookahead + 1],
            queue_head: 0,
            queue_len: 0,
            frames: 0,
        })
    }
    /// Frames by which the limiter delays the output.
    pub fn latency(&self) -> usize {
        self.window - 1
    }
    /// Adds the current frame's peak and returns the loudest peak in the window, in
    /// constant amortized time.
    fn push_peak(&mut self, peak: f64) -> f64 {
        let w = self.window;
        while self.queue_len > 0 && self.queue[(self.queue_head + self.queue_len - 1) % w].1 <= peak
        {
            self.queue_len -= 1;
        }
        self.queue[(self.queue_head + self.queue_len) % w] = (self.frames, peak);
        self.queue_len += 1;
        while self.queue[self.queue_head].0 + w as u64 <= self.frames {
            self.queue_head = (self.queue_head + 1) % w;
            self.queue_len -= 1;
        }
        self.frames += 1;
        self.queue[self.queue_head].1
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
            let loudest = self.push_peak(peak);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_tracks_the_window_maximum() {
        let mut limiter = Limiter::new(
            MasterLimiter {
                lookahead_ms: 1.0,
                ..MasterLimiter::default()
            },
            48_000,
            1,
        )
        .unwrap();
        let window = limiter.window;
        let mut seed = 0x9e37_79b9u32;
        let peaks: Vec<f64> = (0..5000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f64 / (1u32 << 24) as f64 * 2.0
            })
            .collect();
        for (n, &peak) in peaks.iter().enumerate() {
            let expected = peaks[n.saturating_sub(window - 1)..=n]
                .iter()
                .copied()
                .fold(0.0, f64::max);
            assert_eq!(limiter.push_peak(peak), expected, "frame {n}");
        }
    }
}
