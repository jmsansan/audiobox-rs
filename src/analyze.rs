//! Waveforms, spectral analysis and K-weighted integrated loudness.
use crate::{Audio, Error, Result};
use std::f64::consts::PI;

pub fn to_db(linear: f64) -> f64 {
    if linear > 0.0 {
        20.0 * linear.log10()
    } else {
        f64::NEG_INFINITY
    }
}
pub fn from_db(db: f64) -> f64 {
    crate::math::db_to_gain(db)
}
// Render decisions must not depend on the host libm. Report-only APIs retain
// their original math; normalization uses the same scans with deterministic math.
pub(crate) fn render_db(linear: f64) -> f64 {
    if linear > 0.0 {
        20.0 * crate::math::log2(linear) / std::f64::consts::LOG2_10
    } else {
        f64::NEG_INFINITY
    }
}
#[derive(Clone, Debug)]
pub struct LoudnessResult {
    pub integrated: f64,
    pub range: f64,
    pub peak_momentary: f64,
}
#[derive(Clone, Debug)]
pub struct Waveform {
    pub min: Vec<f32>,
    pub max: Vec<f32>,
}
#[derive(Clone, Debug)]
pub struct Spectrum {
    pub frequencies: Vec<f64>,
    pub magnitudes: Vec<f64>,
}
#[derive(Clone, Debug)]
pub struct Spectrogram {
    pub frequencies: Vec<f64>,
    pub times: Vec<f64>,
    pub magnitudes: Vec<Vec<f64>>,
}

fn fft(real: &mut [f64], imag: &mut [f64]) {
    let n = real.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            real.swap(i, j);
            imag.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let theta = -2.0 * PI / len as f64;
        let (wi, wr) = theta.sin_cos();
        for start in (0..n).step_by(len) {
            let (mut ur, mut ui) = (1.0, 0.0);
            for k in 0..len / 2 {
                let a = start + k;
                let b = a + len / 2;
                let tr = ur * real[b] - ui * imag[b];
                let ti = ur * imag[b] + ui * real[b];
                real[b] = real[a] - tr;
                imag[b] = imag[a] - ti;
                real[a] += tr;
                imag[a] += ti;
                let next = ur * wr - ui * wi;
                ui = ur * wi + ui * wr;
                ur = next;
            }
        }
        len *= 2;
    }
}
fn check_fft(sample_rate: u32, fft_size: usize) -> Result<()> {
    if sample_rate == 0 || fft_size < 2 || !fft_size.is_power_of_two() || fft_size > 1 << 20 {
        return Err(Error::invalid(
            "FFT size must be a power of two from 2 to 1048576",
        ));
    }
    Ok(())
}
fn check_finite(samples: &[f32]) -> Result<()> {
    if samples.iter().any(|s| !s.is_finite()) {
        return Err(Error::invalid("FFT samples must be finite"));
    }
    Ok(())
}
/// The Hann window of `fft_size` points and the sum of its values, which scales magnitudes.
fn hann(fft_size: usize) -> (Vec<f64>, f64) {
    let mut sum = 0.0;
    let window = (0..fft_size)
        .map(|i| {
            let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / (fft_size - 1) as f64).cos();
            sum += w;
            w
        })
        .collect();
    (window, sum)
}
/// Windows the first `real.len()` samples, zero-padded, transforms them in the given
/// buffers and returns the one-sided magnitudes.
fn transform(
    samples: &[f32],
    window: &[f64],
    sum: f64,
    real: &mut [f64],
    imag: &mut [f64],
) -> Vec<f64> {
    let fft_size = real.len();
    for (i, (v, w)) in real.iter_mut().zip(window).enumerate() {
        *v = samples.get(i).copied().unwrap_or(0.0) as f64 * w;
    }
    imag.fill(0.0);
    fft(real, imag);
    (0..=fft_size / 2)
        .map(|i| {
            real[i].hypot(imag[i]) / sum
                * if i == 0 || i == fft_size / 2 {
                    1.0
                } else {
                    2.0
                }
        })
        .collect()
}
fn frequencies(sample_rate: u32, fft_size: usize) -> Vec<f64> {
    (0..=fft_size / 2)
        .map(|i| i as f64 * sample_rate as f64 / fft_size as f64)
        .collect()
}
/// The magnitude spectrum of the first `fft_size` samples, Hann-windowed and
/// zero-padded. Every sample of `samples` must be finite, including those after the
/// window.
pub fn spectrum(samples: &[f32], sample_rate: u32, fft_size: usize) -> Result<Spectrum> {
    check_fft(sample_rate, fft_size)?;
    check_finite(samples)?;
    let (window, sum) = hann(fft_size);
    let mut real = vec![0.0; fft_size];
    let mut imag = vec![0.0; fft_size];
    Ok(Spectrum {
        frequencies: frequencies(sample_rate, fft_size),
        magnitudes: transform(samples, &window, sum, &mut real, &mut imag),
    })
}
/// Spectra of `samples` every `hop` samples, each as [`spectrum`] of the samples from
/// that point on. Every sample must be finite.
pub fn spectrogram(samples: &[f32], rate: u32, fft_size: usize, hop: usize) -> Result<Spectrogram> {
    if hop == 0 {
        return Err(Error::invalid("Spectrogram hop must be positive"));
    }
    check_fft(rate, fft_size)?;
    let columns = samples.len().div_ceil(hop);
    if columns
        .checked_mul(fft_size / 2 + 1)
        .is_none_or(|n| n > 64 * 1024 * 1024)
    {
        return Err(Error::limit("Spectrogram exceeds allocation limit"));
    }
    // Each column's suffix is part of the whole input: checking it once gives the
    // same result as checking every suffix, without rescanning the input per column.
    check_finite(samples)?;
    // The window and transform buffers serve every column; only the output grows.
    let (window, sum) = hann(fft_size);
    let mut real = vec![0.0; fft_size];
    let mut imag = vec![0.0; fft_size];
    let mut times = Vec::with_capacity(columns);
    let mut magnitudes = Vec::with_capacity(columns);
    for i in (0..samples.len()).step_by(hop) {
        times.push(i as f64 / rate as f64);
        magnitudes.push(transform(&samples[i..], &window, sum, &mut real, &mut imag));
    }
    Ok(Spectrogram {
        frequencies: frequencies(rate, fft_size),
        times,
        magnitudes,
    })
}
impl Audio {
    pub fn peak(&self) -> f64 {
        self.data
            .iter()
            .flatten()
            .map(|s| s.abs() as f64)
            .fold(0.0, f64::max)
    }
    pub fn peak_db(&self) -> f64 {
        to_db(self.peak())
    }
    pub fn rms_db(&self) -> f64 {
        let n = self.frames() * self.channels();
        to_db(if n == 0 {
            0.0
        } else {
            (self
                .data
                .iter()
                .flatten()
                .map(|&s| (s as f64).powi(2))
                .sum::<f64>()
                / n as f64)
                .sqrt()
        })
    }
    pub fn true_peak_db(&self) -> f64 {
        to_db(self.true_peak::<false>())
    }
    pub(crate) fn render_true_peak_db(&self) -> f64 {
        render_db(self.render_true_peak())
    }
    pub(crate) fn render_true_peak(&self) -> f64 {
        self.true_peak::<true>()
    }
    fn true_peak<const RENDER: bool>(&self) -> f64 {
        let mut peak = self.peak();
        let mut kernel = [0.0; 128];
        for (i, k) in kernel.iter_mut().enumerate() {
            let t = i as f64 / 4.0 - 16.0;
            let sinc = if t == 0.0 {
                1.0
            } else {
                (if RENDER {
                    crate::math::sin_turns(t / 2.0)
                } else {
                    (PI * t).sin()
                }) / (PI * t)
            };
            let w = i as f64 / 127.0;
            let cos = |turns| {
                if RENDER {
                    crate::math::cos_turns(turns)
                } else {
                    (2.0 * PI * turns).cos()
                }
            };
            *k = sinc * (0.42 - 0.5 * cos(w) + 0.08 * cos(2.0 * w));
        }
        for c in &self.data {
            for i in 0..c.len() {
                // Away from the edges every tap is in range: the four phases then add the
                // same terms in the same order without bounds checks, side by side.
                if i >= 16 && i + 16 <= c.len() {
                    let mut sums = [0.0f64; 4];
                    for (k, &x) in c[i - 16..i + 16].iter().enumerate() {
                        for (phase, sum) in sums.iter_mut().enumerate() {
                            *sum += x as f64 * kernel[k * 4 + phase];
                        }
                    }
                    for sum in sums {
                        peak = peak.max(sum.abs());
                    }
                    continue;
                }
                for phase in 0..4 {
                    let mut sum = 0.0;
                    for k in 0..32 {
                        let at = i as i64 + k as i64 - 16;
                        if at >= 0 && (at as usize) < c.len() {
                            sum += c[at as usize] as f64 * kernel[k * 4 + phase];
                        }
                    }
                    peak = peak.max(sum.abs());
                }
            }
        }
        peak
    }
    pub fn waveform(&self, buckets: usize) -> Result<Waveform> {
        if buckets == 0 || buckets > 16 * 1024 * 1024 {
            return Err(Error::invalid("Invalid waveform bucket count"));
        }
        let mut min = vec![0.0f32; buckets];
        let mut max = vec![0.0f32; buckets];
        for b in 0..buckets {
            let start = (b as u128 * self.frames() as u128 / buckets as u128) as usize;
            let end = (((b + 1) as u128 * self.frames() as u128 / buckets as u128) as usize)
                .max(start.saturating_add(1))
                .min(self.frames());
            for c in &self.data {
                for &s in &c[start..end] {
                    min[b] = min[b].min(s);
                    max[b] = max[b].max(s);
                }
            }
        }
        Ok(Waveform { min, max })
    }
    /// K-weighted, gated loudness; range is the legacy 400-ms block percentile estimate.
    pub fn loudness(&self) -> LoudnessResult {
        self.loudness_impl::<false>()
    }
    pub(crate) fn render_loudness(&self) -> LoudnessResult {
        self.loudness_impl::<true>()
    }
    fn loudness_impl<const RENDER: bool>(&self) -> LoudnessResult {
        let rate = self.rate as f64;
        let tangent = |frequency: f64| {
            if RENDER {
                let turns = frequency / rate / 2.0;
                crate::math::sin_turns(turns) / crate::math::cos_turns(turns)
            } else {
                (PI * frequency / rate).tan()
            }
        };
        let k = tangent(1681.974450955533);
        let vh = if RENDER {
            crate::math::db_to_gain(3.999843853973347)
        } else {
            10f64.powf(3.999843853973347 / 20.0)
        };
        let vb = if RENDER {
            crate::math::db_to_gain(3.999843853973347 * 0.4996667741545416)
        } else {
            vh.powf(0.4996667741545416)
        };
        let q = 0.7071752369554196;
        let a0 = 1.0 + k / q + k * k;
        let shelf = [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
            2.0 * (k * k - 1.0) / a0,
            (1.0 - k / q + k * k) / a0,
        ];
        let k = tangent(38.13547087602444);
        let q = 0.5003270373238773;
        let a0 = 1.0 + k / q + k * k;
        let hp = [
            1.0,
            -2.0,
            1.0,
            2.0 * (k * k - 1.0) / a0,
            (1.0 - k / q + k * k) / a0,
        ];
        let filtered: Vec<_> = self
            .data
            .iter()
            .map(|c| crate::dsp::biquad(&crate::dsp::biquad(c, shelf), hp))
            .collect();
        if RENDER && filtered.iter().flatten().any(|s| !s.is_finite()) {
            return LoudnessResult {
                integrated: f64::NAN,
                range: f64::NAN,
                peak_momentary: f64::NAN,
            };
        }
        let weights: Vec<f64> = match self.channels() {
            4 => vec![1.0, 1.0, 1.41, 1.41],
            5 => vec![1.0, 1.0, 1.0, 1.41, 1.41],
            6 => vec![1.0, 1.0, 1.0, 0.0, 1.41, 1.41],
            _ => vec![1.0; self.channels()],
        };
        let block = (0.4 * rate).round().max(1.0) as usize;
        let step = (0.1 * rate).round().max(1.0) as usize;
        let power = |start: usize, end: usize| {
            if start == end {
                return 0.0;
            }
            filtered
                .iter()
                .zip(&weights)
                .map(|(c, w)| {
                    w * c[start..end]
                        .iter()
                        .map(|&s| (s as f64).powi(2))
                        .sum::<f64>()
                        / (end - start) as f64
                })
                .sum::<f64>()
        };
        let lufs = |p: f64| {
            if p > 0.0 {
                -0.691
                    + if RENDER {
                        10.0 * crate::math::log2(p) / std::f64::consts::LOG2_10
                    } else {
                        10.0 * p.log10()
                    }
            } else {
                f64::NEG_INFINITY
            }
        };
        if self.frames() < block {
            let value = lufs(power(0, self.frames()));
            return LoudnessResult {
                integrated: value,
                range: 0.0,
                peak_momentary: value,
            };
        }
        let blocks: Vec<_> = (0..=self.frames() - block)
            .step_by(step)
            .map(|i| power(i, i + block))
            .collect();
        gated_loudness(&blocks, lufs)
    }
}

fn gated_loudness(blocks: &[f64], lufs: impl Fn(f64) -> f64) -> LoudnessResult {
    let absolute: Vec<_> = blocks
        .iter()
        .copied()
        .filter(|&p| lufs(p) >= -70.0)
        .collect();
    if absolute.is_empty() {
        return LoudnessResult {
            integrated: f64::NEG_INFINITY,
            range: 0.0,
            peak_momentary: f64::NEG_INFINITY,
        };
    }
    let relative = lufs(absolute.iter().sum::<f64>() / absolute.len() as f64) - 10.0;
    let gated: Vec<_> = absolute
        .iter()
        .copied()
        .filter(|&p| lufs(p) >= relative)
        .collect();
    let integrated = lufs(gated.iter().sum::<f64>() / gated.len() as f64);
    let mut sorted: Vec<_> = gated.iter().map(|&p| lufs(p)).collect();
    sorted.sort_by(f64::total_cmp);
    let range = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)] - sorted[sorted.len() / 10];
    LoudnessResult {
        integrated,
        range,
        peak_momentary: blocks
            .iter()
            .map(|&p| lufs(p))
            .fold(f64::NEG_INFINITY, f64::max),
    }
}

#[cfg(test)]
mod normalization_tests {
    use super::*;

    #[test]
    fn gates_include_boundary_levels_and_separate_adjacent_powers() {
        let lufs = |p: f64| -0.691 + render_db(p) / 2.0;
        // Find the adjacent f64 powers bracketing -70 LUFS, without assuming
        // that converting -70 to power and back is an exact inverse.
        let mut bits = crate::math::db_to_gain((-70.0 + 0.691) * 2.0).to_bits();
        while lufs(f64::from_bits(bits)) < -70.0 {
            bits += 1;
        }
        while lufs(f64::from_bits(bits - 1)) >= -70.0 {
            bits -= 1;
        }
        let below = f64::from_bits(bits - 1);
        let above = f64::from_bits(bits);
        assert_eq!(gated_loudness(&[below], lufs).integrated, f64::NEG_INFINITY);
        assert_eq!(gated_loudness(&[above], lufs).integrated, lufs(above));
        // Equality must be admitted even when the level is exactly the gate.
        assert_eq!(gated_loudness(&[1.0], |_| -70.0).integrated, -70.0);
        // Two-block relative gate: q >= mean(1,q)/10 iff q >= 1/19.
        let below = (1.0 / 19.0) * (1.0 - 1e-10);
        let above = (1.0 / 19.0) * (1.0 + 1e-10);
        assert_eq!(gated_loudness(&[1.0, below], lufs).integrated, lufs(1.0));
        assert_eq!(
            gated_loudness(&[1.0, above], lufs).integrated,
            lufs((1.0 + above) / 2.0)
        );
        let equality = |p: f64| {
            if p == 1.0 {
                -20.0
            } else if p == 0.1 {
                -30.0
            } else {
                -20.0
            }
        };
        assert_eq!(gated_loudness(&[1.0, 0.1], equality).range, 10.0);
    }
}
