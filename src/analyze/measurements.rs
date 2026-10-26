//! Offline, non-mutating measurements for numerical render review.
use crate::{Audio, Error, Limits, Result};
use std::ops::Range;

/// Maximum requests in one [`Audio::measure_boundaries`] call.
pub const MAX_MEASUREMENT_BOUNDARIES: usize = 4096;

/// Frame-exact leading/trailing silence under a caller's linear amplitude threshold.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThresholdSilence {
    pub frames: usize,
    /// Frames before the first frame where any channel exceeds the threshold.
    pub leading_frames: usize,
    /// Frames after the last frame where any channel exceeds the threshold.
    pub trailing_frames: usize,
    /// First through last active frame, half-open. None for empty/wholly quiet PCM.
    /// For wholly quiet PCM both counts equal frames; they overlap, not add.
    pub active_range: Option<Range<usize>>,
}

/// Unweighted stereo statistics. Power units are squared input-sample amplitude.
/// Empty input has no power result; silence has Some(0). No width/quality verdict.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StereoMeasurements {
    pub frames: usize,
    /// Mean-centered Pearson correlation in [-1, 1]. None for fewer than two
    /// frames or either constant channel, including silence. DC is removed only
    /// for this statistic; all four power measurements retain DC.
    pub correlation: Option<f64>,
    pub left_mean_square: Option<f64>,
    pub right_mean_square: Option<f64>,
    /// Mean square of M=(L+R)/2; no sqrt(2) normalization or loudness weighting.
    pub mid_mean_square: Option<f64>,
    /// Mean square of S=(L-R)/2, with the same scaling as mid.
    pub side_mean_square: Option<f64>,
}

/// An explicitly requested adjacent-sample join; no automatic transient search.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeasurementBoundary {
    /// Between frame-1 and frame. Requires 0 < frame < Audio::frames().
    Frame(usize),
    /// Last frame to frame zero. Fewer than two frames gives no measurement.
    LoopSeam,
}
/// Signed step in input-sample amplitude units, preserving headroom outside +/-1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelStep {
    pub before: f64,
    pub after: f64,
    /// after - before; this number alone cannot establish an audible defect.
    pub difference: f64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct BoundarySamples {
    pub before_frame: usize,
    pub after_frame: usize,
    /// One entry per input channel, in original order.
    pub channels: Vec<ChannelStep>,
    pub max_abs_step: f64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct BoundaryMeasurement {
    pub boundary: MeasurementBoundary,
    /// None only for an empty/one-frame LoopSeam, without synthetic zero padding.
    pub samples: Option<BoundarySamples>,
}

// Compensated f64 accumulation retains small terms when positive/negative samples
// cancel. Finite f32 samples, differences and their squared sums fit in f64 for
// any addressable slice; no platform logarithms/trigonometry are needed.
#[derive(Default)]
struct Sum {
    sum: f64,
    correction: f64,
}
impl Sum {
    fn add(&mut self, value: f64) {
        let next = self.sum + value;
        self.correction += if self.sum.abs() >= value.abs() {
            (self.sum - next) + value
        } else {
            (value - next) + self.sum
        };
        self.sum = next;
    }
    fn value(&self) -> f64 {
        self.sum + self.correction
    }
}
fn finite(sample: f32) -> Result<f64> {
    if sample.is_finite() {
        Ok(sample as f64)
    } else {
        Err(Error::invalid("Measurement samples must be finite"))
    }
}
fn reserve<T>(count: usize) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("Cannot allocate measurement output"))?;
    Ok(values)
}
/// Arithmetic DC mean of one channel, in signed input-sample amplitude units.
/// Empty input returns None; silence returns Some(0). All samples must be finite.
/// Compensated f64 summation, O(samples) time and O(1) storage; no allocation.
/// This is numerical reporting, not an extension of the rendered-PCM bit contract.
pub fn dc_mean(samples: &[f32]) -> Result<Option<f64>> {
    let mut sum = Sum::default();
    for &sample in samples {
        sum.add(finite(sample)?);
    }
    Ok((!samples.is_empty()).then(|| sum.value() / samples.len() as f64))
}
/// Statistics of equal-length stereo channels; no allocation or PCM modification.
/// Requires finite samples, including constant/silent inputs. A two-pass,
/// compensated f64 scan avoids raw-moment variance cancellation under large DC.
/// Power is the same unweighted mean-square concept as [`Audio::mean_square`];
/// accumulation order/compensation can change last bits. Pearson uses platform
/// sqrt and is report-only; compare results numerically, not as portable bytes.
pub fn stereo_measurements(left: &[f32], right: &[f32]) -> Result<StereoMeasurements> {
    if left.len() != right.len() {
        return Err(Error::invalid("Stereo channels must have equal lengths"));
    }
    let mut out = StereoMeasurements {
        frames: left.len(),
        correlation: None,
        left_mean_square: None,
        right_mean_square: None,
        mid_mean_square: None,
        side_mean_square: None,
    };
    let l_mean = dc_mean(left)?;
    let r_mean = dc_mean(right)?;
    let (Some(l_mean), Some(r_mean)) = (l_mean, r_mean) else {
        return Ok(out);
    };
    let [mut lp, mut rp, mut mp, mut sp, mut lv, mut rv, mut cov] =
        std::array::from_fn::<_, 7, _>(|_| Sum::default());
    let (mut l_varies, mut r_varies) = (false, false);
    for (&l, &r) in left.iter().zip(right) {
        l_varies |= l != left[0];
        r_varies |= r != right[0];
        let (l, r) = (l as f64, r as f64);
        let (m, s) = ((l + r) * 0.5, (l - r) * 0.5);
        let (ld, rd) = (l - l_mean, r - r_mean);
        lp.add(l * l);
        rp.add(r * r);
        mp.add(m * m);
        sp.add(s * s);
        lv.add(ld * ld);
        rv.add(rd * rd);
        cov.add(ld * rd);
    }
    let n = left.len() as f64;
    out.left_mean_square = Some(lp.value() / n);
    out.right_mean_square = Some(rp.value() / n);
    out.mid_mean_square = Some(mp.value() / n);
    out.side_mean_square = Some(sp.value() / n);
    if left.len() >= 2 && l_varies && r_varies {
        let denominator = lv.value().sqrt() * rv.value().sqrt();
        if denominator > 0.0 {
            out.correlation = Some((cov.value() / denominator).clamp(-1.0, 1.0));
        }
    }
    Ok(out)
}

impl Audio {
    /// Per-channel [`dc_mean`], with None for each empty channel. Allocates at
    /// most one Vec of channel-count options, not a PCM copy. Default Limits
    /// apply, including to input decoded with custom larger limits.
    pub fn dc_means(&self) -> Result<Vec<Option<f64>>> {
        Limits::default().check(self.frames(), self.channels(), self.rate)?;
        let mut values = reserve(self.channels())?;
        for channel in &self.data {
            values.push(dc_mean(channel)?);
        }
        Ok(values)
    }
    /// Frame-exact threshold silence: a frame is quiet iff every channel's
    /// absolute sample is <= threshold (linear input units, finite and >=0).
    /// Checks all samples, including inside the active range. Empty input has
    /// zero counts; wholly quiet input has two overlapping full-length counts.
    /// This is independent of detect_silence's window-RMS/min-duration policy.
    /// Default Limits apply. O(samples) time, O(1) storage, no allocation.
    pub fn threshold_silence(&self, threshold: f64) -> Result<ThresholdSilence> {
        if !threshold.is_finite() || threshold < 0.0 {
            return Err(Error::invalid(
                "Silence threshold must be finite and nonnegative",
            ));
        }
        Limits::default().check(self.frames(), self.channels(), self.rate)?;
        let (mut first, mut end) = (self.frames(), 0);
        for channel in &self.data {
            for (i, &sample) in channel.iter().enumerate() {
                if finite(sample)?.abs() > threshold {
                    first = first.min(i);
                    end = end.max(i + 1);
                }
            }
        }
        let active_range = (end > 0).then_some(first..end);
        Ok(ThresholdSilence {
            frames: self.frames(),
            leading_frames: first,
            trailing_frames: if end > 0 {
                self.frames() - end
            } else {
                self.frames()
            },
            active_range,
        })
    }
    /// [`stereo_measurements`] of this Audio, requiring exactly two channels.
    /// No downmixing, channel selection or allocation. Default Limits apply.
    pub fn stereo_measurements(&self) -> Result<StereoMeasurements> {
        if self.channels() != 2 {
            return Err(Error::invalid("Stereo measurements require two channels"));
        }
        Limits::default().check(self.frames(), self.channels(), self.rate)?;
        stereo_measurements(&self.data[0], &self.data[1])
    }
    /// Measure explicit joins, in request order (including duplicates), without
    /// changing samples/metadata or labeling steps as audible clicks. All ranges
    /// and finite samples are checked before allocating output. Frame boundaries
    /// must have two existing neighbors; LoopSeam yields None below two frames.
    ///
    /// Default Limits cap input shape, including 64 channels. At most
    /// MAX_MEASUREMENT_BOUNDARIES requests (4,096), each with one bounded channel
    /// Vec; allocation failure/oversize returns LimitExceeded. Empty requests
    /// still validate PCM and return an empty Vec. Time O(samples+requests*channels),
    /// storage O(requests*channels), with no per-frame PCM copy/window scan.
    pub fn measure_boundaries(
        &self,
        boundaries: &[MeasurementBoundary],
    ) -> Result<Vec<BoundaryMeasurement>> {
        if boundaries.len() > MAX_MEASUREMENT_BOUNDARIES {
            return Err(Error::limit("Too many measurement boundaries"));
        }
        Limits::default().check(self.frames(), self.channels(), self.rate)?;
        for boundary in boundaries {
            if let MeasurementBoundary::Frame(frame) = *boundary {
                if frame == 0 || frame >= self.frames() {
                    return Err(Error::invalid("Boundary must lie between existing frames"));
                }
            }
        }
        for &sample in self.data.iter().flatten() {
            finite(sample)?;
        }
        let mut out = reserve(boundaries.len())?;
        for &boundary in boundaries {
            let frames = match boundary {
                MeasurementBoundary::Frame(frame) => Some((frame - 1, frame)),
                MeasurementBoundary::LoopSeam if self.frames() >= 2 => Some((self.frames() - 1, 0)),
                MeasurementBoundary::LoopSeam => None,
            };
            let samples = if let Some((before_frame, after_frame)) = frames {
                let mut channels = reserve(self.channels())?;
                let mut max_abs_step = 0.0f64;
                for channel in &self.data {
                    let before = channel[before_frame] as f64;
                    let after = channel[after_frame] as f64;
                    let difference = after - before;
                    max_abs_step = max_abs_step.max(difference.abs());
                    channels.push(ChannelStep {
                        before,
                        after,
                        difference,
                    });
                }
                Some(BoundarySamples {
                    before_frame,
                    after_frame,
                    channels,
                    max_abs_step,
                })
            } else {
                None
            };
            out.push(BoundaryMeasurement { boundary, samples });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defensive_audio_scans_reject_nonfinite_samples_even_without_boundaries() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let a = Audio::new(vec![vec![0.0; 3]; 2], 48000)
                .unwrap()
                .with_data(vec![vec![0.0, 0.0, 0.0], vec![0.0, value, 0.0]]);
            assert_eq!(
                a.dc_means().unwrap_err().code,
                crate::ErrorCode::InvalidArgument
            );
            assert_eq!(
                a.threshold_silence(0.0).unwrap_err().code,
                crate::ErrorCode::InvalidArgument
            );
            assert_eq!(
                a.stereo_measurements().unwrap_err().code,
                crate::ErrorCode::InvalidArgument
            );
            assert_eq!(
                a.measure_boundaries(&[]).unwrap_err().code,
                crate::ErrorCode::InvalidArgument
            );
            assert_eq!(
                a.measure_boundaries(&[MeasurementBoundary::LoopSeam])
                    .unwrap_err()
                    .code,
                crate::ErrorCode::InvalidArgument
            );
        }
    }
}
