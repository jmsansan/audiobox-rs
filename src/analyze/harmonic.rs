//! Shares of spectral energy at the harmonics of a fundamental and elsewhere.
use super::measurements::reserve;
use crate::{Audio, Error, Limits, Result};
use std::ops::Range;

/// The most analysis frames one [`Audio::harmonic_energy`] measures.
pub const MAX_HARMONIC_FRAMES: usize = 1 << 20;
/// The most work one [`Audio::harmonic_energy`] is allowed, as an estimate:
/// analysis frames × channels × `fft_size` × log₂ `fft_size`. Every frame counts.
pub const MAX_HARMONIC_WORK: u64 = 1 << 32;
/// The fewest FFT bins between harmonics: the fundamental is at least this many
/// bins, so that each harmonic's band leaves room between them.
pub const MIN_HARMONIC_BINS: usize = 8;

/// How [`Audio::harmonic_energy`] measures.
#[derive(Clone, Debug, PartialEq)]
pub struct HarmonicOptions {
    /// The fundamental, in Hz: finite, at least [`MIN_HARMONIC_BINS`] bins
    /// (`8 × sample_rate / fft_size`) and below half the sample rate. Supply it, or
    /// take one from [`super::PitchTrack::median_frequency`].
    pub fundamental_hz: f64,
    /// Samples per analysis frame: a power of two from 16 to 1048576,
    /// Hann-windowed as [`super::spectrum`] does.
    pub fft_size: usize,
    /// Frames between the starts of analysis frames, at least 1.
    pub hop: usize,
    /// How far from each harmonic its band reaches, in cents below and above it
    /// (finite, at least 0). Each side of the band is at least 2 bins wide, the
    /// Hann window's main lobe, and at most a quarter of the fundamental, so that
    /// bands never meet.
    pub tolerance_cents: f64,
    /// The input frames measured, within the input.
    pub range: Range<usize>,
    /// One channel by index, or `None` for all channels together.
    pub channel: Option<usize>,
}

impl HarmonicOptions {
    /// The harmonics of `fundamental_hz` over `range`, with 8192-sample frames every
    /// 4096, bands of 25 cents and all channels. At 48 kHz that takes fundamentals
    /// from 46.875 Hz; a lower one, or a higher sample rate, needs a longer frame.
    pub fn new(fundamental_hz: f64, range: Range<usize>) -> Self {
        Self {
            fundamental_hz,
            fft_size: 8192,
            hop: 4096,
            tolerance_cents: 25.0,
            range,
            channel: None,
        }
    }
}

/// The shares of spectral energy at the harmonics of a fundamental and elsewhere,
/// with the settings they were measured with.
///
/// The energy is the Hann-windowed one-sided power spectrum (from the magnitudes of
/// [`super::spectrum`]: half an interior bin's magnitude squared, all of it at
/// Nyquist) summed over the analysis frames and channels. DC and the bin beside it
/// are not counted; rumble and aliases below the fundamental are, as inharmonic.
#[derive(Clone, Debug, PartialEq)]
pub struct HarmonicEnergy {
    pub options: HarmonicOptions,
    pub sample_rate: u32,
    /// The analysis frames measured.
    pub frames: usize,
    /// The harmonics counted: every multiple of the fundamental up to half the
    /// sample rate.
    pub harmonics: usize,
    /// Each harmonic's band, its lowest and highest frequency in Hz, from the
    /// first harmonic.
    pub bands_hz: Vec<(f64, f64)>,
    /// The share of the energy in each harmonic's band, from the first harmonic.
    pub harmonic_shares: Vec<f64>,
    /// The share of the energy in all the harmonics' bands, from 0 to 1; `None`
    /// when there is no energy to share, as for silence or no frames.
    pub harmonic_share: Option<f64>,
    /// The share of the energy elsewhere: 1 minus the harmonic share.
    pub inharmonic_share: Option<f64>,
}

impl Audio {
    /// The shares of spectral energy within a band around each harmonic of a
    /// fundamental, and elsewhere, over a range of input frames.
    ///
    /// It is an indicator, not a verdict. Energy away from the harmonics can be
    /// aliasing that folded back from above half the sample rate, the partials of
    /// a bell or a metallic FM patch on purpose, noise, a second note, or vibrato
    /// and a fundamental that drift out of their bands. A share says how much,
    /// not which, nor whether it sounds wrong.
    ///
    /// Frame `i` starts at `range.start + i * hop` and reads `fft_size` samples
    /// within the range. Only whole frames are measured; a range shorter than one
    /// frame is a single frame, zero-padded. Bad options, a range outside the
    /// input, a missing channel and non-finite samples return `InvalidArgument`.
    /// Default [`Limits`] apply, plus at most [`MAX_HARMONIC_FRAMES`] frames and
    /// [`MAX_HARMONIC_WORK`], or `LimitExceeded`. The FFT uses the platform's math,
    /// so results are for reports, not bit-identical across hosts.
    ///
    /// ```
    /// use audiobox::analyze::HarmonicOptions;
    ///
    /// // 220 Hz with its first three harmonics, and a bell-like partial at 2.76 × 220.
    /// let partials = [(220.0, 0.4), (440.0, 0.2), (660.0, 0.1), (607.2, 0.2)];
    /// let samples: Vec<f32> = (0..48_000)
    ///     .map(|i| {
    ///         let t = i as f64 / 48_000.0;
    ///         partials
    ///             .iter()
    ///             .map(|&(f, a)| a * (2.0 * std::f64::consts::PI * f * t).sin())
    ///             .sum::<f64>() as f32
    ///     })
    ///     .collect();
    /// let audio = audiobox::Audio::new(vec![samples], 48_000)?;
    /// let energy = audio.harmonic_energy(&HarmonicOptions::new(220.0, 0..48_000))?;
    /// // The partial holds 0.04 of 0.25 of the power: 16 %.
    /// let inharmonic = energy.inharmonic_share.unwrap();
    /// assert!((inharmonic - 0.16).abs() < 0.01, "{inharmonic}");
    /// # Ok::<(), audiobox::Error>(())
    /// ```
    pub fn harmonic_energy(&self, options: &HarmonicOptions) -> Result<HarmonicEnergy> {
        let o = options.clone();
        let rate = self.rate as f64;
        super::check_fft(self.rate, o.fft_size)?;
        if o.fft_size < super::MIN_SPECTRAL_FFT {
            return Err(Error::invalid("Harmonic FFT size must be at least 16"));
        }
        let bin = rate / o.fft_size as f64;
        if !(o.fundamental_hz.is_finite()
            && o.fundamental_hz >= MIN_HARMONIC_BINS as f64 * bin
            && o.fundamental_hz < rate / 2.0)
        {
            return Err(Error::invalid(
                "Fundamental must be at least 8 FFT bins and below half the sample rate",
            ));
        }
        if o.hop == 0 {
            return Err(Error::invalid("Harmonic hop must be at least one frame"));
        }
        if !(o.tolerance_cents.is_finite() && o.tolerance_cents >= 0.0) {
            return Err(Error::invalid(
                "Harmonic tolerance must be finite and at least 0",
            ));
        }
        if o.range.start > o.range.end || o.range.end > self.frames() {
            return Err(Error::invalid("Harmonic range must lie within the input"));
        }
        Limits::default().check(self.frames(), self.channels(), self.rate)?;
        let channels: &[Vec<f32>] = match o.channel {
            Some(c) => self
                .data
                .get(c..=c)
                .ok_or_else(|| Error::invalid("Harmonic channel does not exist"))?,
            None => &self.data,
        };
        if channels
            .iter()
            .flat_map(|c| &c[o.range.clone()])
            .any(|s| !s.is_finite())
        {
            return Err(Error::invalid("Harmonic samples must be finite"));
        }
        let length = o.range.len();
        let count = match length {
            0 => 0,
            n if n < o.fft_size => 1,
            n => (n - o.fft_size) / o.hop + 1,
        };
        if count > MAX_HARMONIC_FRAMES {
            return Err(Error::limit("Too many harmonic analysis frames"));
        }
        if (count as u64)
            .checked_mul(channels.len() as u64)
            .and_then(|n| n.checked_mul(o.fft_size as u64))
            .and_then(|n| n.checked_mul(u64::from(o.fft_size.trailing_zeros())))
            .is_none_or(|n| n > MAX_HARMONIC_WORK)
        {
            return Err(Error::limit("Harmonic analysis exceeds its work limit"));
        }
        // The bands: from `tolerance_cents` below each harmonic to as far above it,
        // each side at least 2 bins and at most a quarter of the fundamental.
        let nyquist = rate / 2.0;
        let harmonics = (nyquist / o.fundamental_hz).floor() as usize;
        let ratio = 2f64.powf(o.tolerance_cents / 1200.0);
        let side = |hz: f64| hz.max(2.0 * bin).min(o.fundamental_hz / 4.0);
        let mut bands_hz = reserve(harmonics)?;
        for h in 1..=harmonics {
            let f = h as f64 * o.fundamental_hz;
            bands_hz.push((f - side(f - f / ratio), f + side(f * ratio - f)));
        }
        let bins = o.fft_size / 2 + 1;
        // Which harmonic each bin belongs to, if any. The bands never meet, so a
        // bin can only be in the band of its nearest harmonic.
        let owner: Vec<Option<usize>> = (0..bins)
            .map(|k| {
                let f = k as f64 * bin;
                let h = (f / o.fundamental_hz).round() as usize;
                (h >= 1 && h <= harmonics && {
                    let (low, high) = bands_hz[h - 1];
                    (low..=high).contains(&f)
                })
                .then(|| h - 1)
            })
            .collect();
        // DC and the bin beside it, where a DC offset's window lobe falls, are not
        // counted; every other bin is, aliases below the fundamental included.
        let counted = |k: usize| k >= 2;
        // One-sided power: the spectrum's amplitudes are doubled except at DC and
        // Nyquist, so an interior bin's power is half its amplitude squared.
        let weight = |k: usize| if k == 0 || k == bins - 1 { 1.0 } else { 0.5 };
        let (window, sum) = super::hann(o.fft_size);
        let mut real = vec![0.0; o.fft_size];
        let mut imag = vec![0.0; o.fft_size];
        let mut power = vec![0.0; bins];
        for i in 0..count {
            let start = o.range.start + i * o.hop;
            let end = (start + o.fft_size).min(o.range.end);
            // An all-zero frame adds no power: skipping it changes nothing.
            for c in channels
                .iter()
                .filter(|c| c[start..end].iter().any(|&s| s != 0.0))
            {
                let m = super::transform(&c[start..end], &window, sum, &mut real, &mut imag);
                for (k, (p, v)) in power.iter_mut().zip(m).enumerate() {
                    *p += weight(k) * v * v;
                }
            }
        }
        let mut shares = vec![0.0; harmonics];
        let mut total = 0.0;
        for k in (0..bins).filter(|&k| counted(k)) {
            total += power[k];
            if let Some(h) = owner[k] {
                shares[h] += power[k];
            }
        }
        let (harmonic_shares, harmonic_share) = if total > 0.0 && total.is_finite() {
            shares.iter_mut().for_each(|s| *s /= total);
            let share = shares.iter().sum::<f64>().min(1.0);
            (shares, Some(share))
        } else {
            (vec![0.0; harmonics], None)
        };
        Ok(HarmonicEnergy {
            options: o,
            sample_rate: self.rate,
            frames: count,
            harmonics,
            bands_hz,
            harmonic_shares,
            harmonic_share,
            inharmonic_share: harmonic_share.map(|s| 1.0 - s),
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
                a.harmonic_energy(&HarmonicOptions::new(220.0, 0..3))
                    .unwrap_err()
                    .code,
                crate::ErrorCode::InvalidArgument
            );
        }
    }
}
