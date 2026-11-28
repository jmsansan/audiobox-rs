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
/// The bins per fundamental [`HarmonicOptions::new`] chooses frames for: enough
/// to keep a partial between harmonics, such as a bell's at 2.76 times the
/// fundamental, a few bins clear of their bands.
pub const HARMONIC_BINS: usize = 24;

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
    /// Frames between the starts of analysis frames, from 1 to `fft_size / 2`, so
    /// that the Hann windows overlap by half or more and leave no blind spot.
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
    /// The harmonics of `fundamental_hz` over `range` of audio at `sample_rate`,
    /// with bands of 25 cents and all channels. Frames are long enough for the
    /// fundamental to span [`HARMONIC_BINS`] bins: the shortest power of two of at
    /// least 4096 samples that does (8192 at 220 Hz and 48 kHz, 32768 at 55 Hz),
    /// at most 1048576, with a hop of half a frame. A range shorter than that
    /// gets the longest power of two within it, down to the length at which the
    /// fundamental spans [`MIN_HARMONIC_BINS`] bins; shorter still, it is one
    /// padded frame (see [`HarmonicEnergy::padded`]).
    pub fn new(fundamental_hz: f64, sample_rate: u32, range: Range<usize>) -> Self {
        // The shortest power of two, from 16 to 2^20, in which the fundamental
        // spans `bins` bins.
        let spanning = |bins: usize| {
            let wanted = bins as f64 * f64::from(sample_rate) / fundamental_hz;
            if wanted.is_finite() && wanted > 0.0 {
                (wanted.ceil().min((1 << 20) as f64) as usize)
                    .next_power_of_two()
                    .clamp(16, 1 << 20)
            } else {
                4096
            }
        };
        let within = match range.len() {
            0..16 => 16,
            n => 1 << n.ilog2(),
        };
        let fft_size = spanning(HARMONIC_BINS)
            .max(4096)
            .min(within)
            .max(spanning(MIN_HARMONIC_BINS))
            .min(1 << 20);
        Self {
            fundamental_hz,
            fft_size,
            hop: fft_size / 2,
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
/// Nyquist) summed over the analysis frames and channels, each frame with its mean
/// removed, so a DC offset adds nothing. Every bin but DC counts: rumble and
/// aliases away from the harmonics as inharmonic, an alias that folds into a
/// harmonic's band as harmonic.
#[derive(Clone, Debug, PartialEq)]
pub struct HarmonicEnergy {
    pub options: HarmonicOptions,
    pub sample_rate: u32,
    /// The analysis frames measured.
    pub frames: usize,
    /// Whether the range is shorter than one frame, so that its one frame is
    /// zero-padded: the cut-off edges of the sound then spread its energy over
    /// other bins, and the shares say more about the cut than the sound.
    pub padded: bool,
    /// The bins the fundamental spans, `fundamental_hz / (sample_rate / fft_size)`:
    /// how finely the window separates frequencies. At the default 25 cents,
    /// [`HARMONIC_BINS`] (24) or more keep a partial a sixth of the fundamental from
    /// a low harmonic out of its
    /// band; fewer resolve less (a short range gets fewer), and wider bands hold
    /// more of it (at 100 cents, most of it near the third harmonic). The bands
    /// widen with the harmonic: at 25 cents, from the 12th harmonic on, a band is
    /// wider than a sixth of the fundamental.
    pub fundamental_bins: f64,
    /// The harmonics counted: every multiple of the fundamental up to half the
    /// sample rate.
    pub harmonics: usize,
    /// Each harmonic's band, its lowest and highest frequency in Hz, from the
    /// first harmonic.
    pub bands_hz: Vec<(f64, f64)>,
    /// The share of the energy in each harmonic's band, from the first harmonic.
    pub harmonic_shares: Vec<f64>,
    /// The share of the energy in all the harmonics' bands, from 0 to 1; `None`
    /// when there is no energy to share, as for silence, a DC offset alone or no
    /// frames.
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
    /// within the range; when the hops leave a tail, one more frame ends at the
    /// range's end, weighed by the fraction of a hop it adds. Only whole frames are
    /// measured; a range shorter than one
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
    /// let energy = audio.harmonic_energy(&HarmonicOptions::new(220.0, 48_000, 0..48_000))?;
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
        if o.hop == 0 || o.hop > o.fft_size / 2 {
            return Err(Error::invalid(
                "Harmonic hop must be from 1 to half the FFT size, so that windows overlap",
            ));
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
        // Whole frames every hop, and one more ending at the range's end when the
        // hops leave a tail, so that every sample is measured. The last frame
        // weighs as much as the fraction of a hop it adds, so that the shares do
        // not jump as a range grows by a sample.
        let (count, tail, tail_weight) = match length {
            0 => (0, false, 0.0),
            n if n < o.fft_size => (1, false, 0.0),
            n => {
                let whole = (n - o.fft_size) / o.hop + 1;
                let left = n - ((whole - 1) * o.hop + o.fft_size);
                (
                    whole + usize::from(left > 0),
                    left > 0,
                    left as f64 / o.hop as f64,
                )
            }
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
        // Every multiple at or below Nyquist, counted exactly: the division can
        // round a harmonic that lands on Nyquist out.
        let mut harmonics = (nyquist / o.fundamental_hz).floor() as usize;
        while (harmonics + 1) as f64 * o.fundamental_hz <= nyquist {
            harmonics += 1;
        }
        while harmonics > 0 && harmonics as f64 * o.fundamental_hz > nyquist {
            harmonics -= 1;
        }
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
        // DC is not counted: the frame means are removed, so what is left there is
        // the window's own lobe. Every other bin is, rumble and aliases below the
        // fundamental included.
        let counted = |k: usize| k >= 1;
        // One-sided power: the spectrum's amplitudes are doubled except at DC and
        // Nyquist, so an interior bin's power is half its amplitude squared.
        let weight = |k: usize| if k == 0 || k == bins - 1 { 1.0 } else { 0.5 };
        let (window, sum) = super::hann(o.fft_size);
        let mut real = vec![0.0; o.fft_size];
        let mut imag = vec![0.0; o.fft_size];
        let mut power = vec![0.0; bins];
        let mut frame = vec![0.0f32; o.fft_size];
        for i in 0..count {
            let last = tail && i == count - 1;
            let share = if last { tail_weight } else { 1.0 };
            let start = if last {
                o.range.end - o.fft_size
            } else {
                o.range.start + i * o.hop
            };
            let end = (start + o.fft_size).min(o.range.end);
            for c in channels.iter().map(|c| &c[start..end]) {
                // A frame at one level, as silence or a DC offset alone, adds no
                // power: skipping it changes nothing.
                if c.iter().all(|&s| s == c[0]) {
                    continue;
                }
                // The frame's mean is removed, so that a DC offset leaks no power
                // through the window into other bins.
                let mean = c.iter().map(|&s| f64::from(s)).sum::<f64>() / c.len() as f64;
                frame.clear();
                frame.extend(c.iter().map(|&s| (f64::from(s) - mean) as f32));
                let m = super::transform(&frame, &window, sum, &mut real, &mut imag);
                for (k, (p, v)) in power.iter_mut().zip(m).enumerate() {
                    *p += share * weight(k) * v * v;
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
        let padded = count == 1 && length < o.fft_size;
        let fundamental_bins = o.fundamental_hz / bin;
        Ok(HarmonicEnergy {
            options: o,
            sample_rate: self.rate,
            frames: count,
            padded,
            fundamental_bins,
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
                a.harmonic_energy(&HarmonicOptions::new(220.0, 48_000, 0..3))
                    .unwrap_err()
                    .code,
                crate::ErrorCode::InvalidArgument
            );
        }
    }
}
