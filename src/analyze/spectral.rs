//! Spectral centroid and spectral flux over time.
use super::measurements::reserve;
use crate::{Audio, Error, Limits, Result};
use std::ops::Range;

/// The most analysis frames one [`Audio::spectral_timeline`] measures.
pub const MAX_SPECTRAL_FRAMES: usize = 1 << 20;

/// How [`Audio::spectral_timeline`] frames and measures the input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectralOptions {
    /// Samples per analysis frame: a power of two from 2 to 1048576, Hann-windowed
    /// as [`super::spectrum`] does. Bins are `sample_rate / fft_size` Hz apart.
    pub fft_size: usize,
    /// Frames between the starts of analysis frames, at least 1.
    pub hop: usize,
    /// One channel by index, or `None` for all channels together.
    pub channel: Option<usize>,
    /// An analysis frame whose RMS is at or below this level, in dBFS (finite), is
    /// silent and has no measurements.
    pub silence_db: f64,
}

impl SpectralOptions {
    /// Frames of `fft_size` samples every `hop` frames, over all channels, silent at
    /// or below −80 dBFS.
    pub fn new(fft_size: usize, hop: usize) -> Self {
        Self {
            fft_size,
            hop,
            channel: None,
            silence_db: -80.0,
        }
    }
}

/// The measurements of one analysis frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectralFrame {
    /// The input frame where the analysis frame starts.
    pub start_frame: usize,
    /// The amplitude-weighted mean frequency of the spectrum, in Hz: the sum of
    /// each bin's frequency times its magnitude over the sum of the magnitudes, DC
    /// and Nyquist included. The window's leakage spreads a little magnitude over
    /// every bin, which pulls the centroid slightly toward the middle of the
    /// spectrum. `None` when the frame is silent.
    pub centroid_hz: Option<f64>,
    /// How much the spectrum's shape changed since the previous frame: the
    /// Euclidean distance between the two magnitude spectra, each scaled to unit
    /// length, divided by √2. It runs from 0 (the same shape at any level) to 1
    /// (no bin in common). `None` for the first frame and when this frame or the
    /// previous one is silent.
    pub flux: Option<f64>,
}

/// Spectral centroid and flux per analysis frame, with the settings they were
/// measured with.
///
/// Analysis frame `i` starts at input frame `i * hop` and reads `fft_size`
/// samples, zero-padded past the end of the input, as [`super::spectrogram`]
/// does. With all channels, each bin's magnitude is the root mean square of the
/// channels' magnitudes, so that channels in opposite phase do not cancel.
#[derive(Clone, Debug, PartialEq)]
pub struct SpectralTimeline {
    pub fft_size: usize,
    pub hop: usize,
    pub channel: Option<usize>,
    pub silence_db: f64,
    pub sample_rate: u32,
    /// Frames in the input.
    pub input_frames: usize,
    pub frames: Vec<SpectralFrame>,
}

/// The measurements of the analysis frames that start in a range of input frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectralSummary {
    /// Analysis frames that start in the range.
    pub frames: usize,
    /// Of those, the frames with a centroid: the ones that are not silent.
    pub measured: usize,
    /// The mean, lowest and highest centroid, in Hz; `None` without a measured frame.
    pub centroid_mean_hz: Option<f64>,
    pub centroid_min_hz: Option<f64>,
    pub centroid_max_hz: Option<f64>,
    /// The mean and highest flux of the frames that have one; `None` without one.
    pub flux_mean: Option<f64>,
    pub flux_max: Option<f64>,
}

impl SpectralTimeline {
    /// The analysis frames that start in `range`, which lies within the input.
    pub fn summary(&self, range: Range<usize>) -> Result<SpectralSummary> {
        if range.start > range.end || range.end > self.input_frames {
            return Err(Error::invalid("Summary range must lie within the input"));
        }
        let selected = self
            .frames
            .iter()
            .filter(|f| range.contains(&f.start_frame));
        let mut out = SpectralSummary {
            frames: 0,
            measured: 0,
            centroid_mean_hz: None,
            centroid_min_hz: None,
            centroid_max_hz: None,
            flux_mean: None,
            flux_max: None,
        };
        let (mut centroid_sum, mut flux_sum, mut fluxes) = (0.0, 0.0, 0usize);
        for frame in selected {
            out.frames += 1;
            if let Some(c) = frame.centroid_hz {
                out.measured += 1;
                centroid_sum += c;
                out.centroid_min_hz = Some(out.centroid_min_hz.map_or(c, |m| m.min(c)));
                out.centroid_max_hz = Some(out.centroid_max_hz.map_or(c, |m| m.max(c)));
            }
            if let Some(f) = frame.flux {
                fluxes += 1;
                flux_sum += f;
                out.flux_max = Some(out.flux_max.map_or(f, |m| m.max(f)));
            }
        }
        if out.measured > 0 {
            out.centroid_mean_hz = Some(centroid_sum / out.measured as f64);
        }
        if fluxes > 0 {
            out.flux_mean = Some(flux_sum / fluxes as f64);
        }
        Ok(out)
    }
}

impl Audio {
    /// The spectral centroid and flux of the input over time, as `options` say.
    ///
    /// Samples must be finite, `hop` at least 1, `silence_db` finite and a channel
    /// must exist; otherwise the result is `InvalidArgument`. Default [`Limits`]
    /// apply, and at most [`MAX_SPECTRAL_FRAMES`] analysis frames. The work is
    /// O(frames × channels × fft_size × log fft_size) and the storage O(fft_size)
    /// plus one entry per frame. The FFT uses the platform's math, so results are
    /// for reports, not bit-identical across hosts.
    ///
    /// ```
    /// use audiobox::analyze::SpectralOptions;
    ///
    /// // A 1 kHz sine, one second.
    /// let sine: Vec<f32> = (0..48_000)
    ///     .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin())
    ///     .collect();
    /// let audio = audiobox::Audio::new(vec![sine], 48_000)?;
    /// let timeline = audio.spectral_timeline(&SpectralOptions::new(2048, 1024))?;
    /// let summary = timeline.summary(0..24_000)?;
    /// let centroid = summary.centroid_mean_hz.unwrap();
    /// assert!((centroid - 1000.0).abs() < 20.0, "{centroid}");
    /// assert!(summary.flux_max.unwrap() < 0.01);
    /// # Ok::<(), audiobox::Error>(())
    /// ```
    pub fn spectral_timeline(&self, options: &SpectralOptions) -> Result<SpectralTimeline> {
        let SpectralOptions {
            fft_size,
            hop,
            channel,
            silence_db,
        } = *options;
        if hop == 0 {
            return Err(Error::invalid("Spectral hop must be at least one frame"));
        }
        if !silence_db.is_finite() {
            return Err(Error::invalid("Silence level must be finite"));
        }
        super::check_fft(self.rate, fft_size)?;
        Limits::default().check(self.frames(), self.channels(), self.rate)?;
        let channels: &[Vec<f32>] = match channel {
            Some(c) => self
                .data
                .get(c..=c)
                .ok_or_else(|| Error::invalid("Spectral channel does not exist"))?,
            None => &self.data,
        };
        if channels.iter().flatten().any(|s| !s.is_finite()) {
            return Err(Error::invalid("Spectral samples must be finite"));
        }
        let input_frames = self.frames();
        let count = input_frames.div_ceil(hop);
        if count > MAX_SPECTRAL_FRAMES {
            return Err(Error::limit("Too many spectral analysis frames"));
        }
        let mut frames = reserve(count)?;
        let (window, sum) = super::hann(fft_size);
        let mut real = vec![0.0; fft_size];
        let mut imag = vec![0.0; fft_size];
        let bins = fft_size / 2 + 1;
        let frequencies = super::frequencies(self.rate, fft_size);
        let mut power = vec![0.0; bins];
        let mut previous: Option<Vec<f64>> = None;
        let silence = super::from_db(silence_db);
        let silent_power = silence * silence;
        for start in (0..input_frames).step_by(hop) {
            let end = (start + fft_size).min(input_frames);
            let mean_square = channels
                .iter()
                .flat_map(|c| &c[start..end])
                .map(|&s| s as f64 * s as f64)
                .sum::<f64>()
                / ((end - start) * channels.len()) as f64;
            power.fill(0.0);
            for c in channels {
                let m = super::transform(&c[start..], &window, sum, &mut real, &mut imag);
                for (p, v) in power.iter_mut().zip(m) {
                    *p += v * v;
                }
            }
            let magnitudes: Vec<f64> = power
                .iter()
                .map(|p| (p / channels.len() as f64).sqrt())
                .collect();
            let total: f64 = magnitudes.iter().sum();
            let length = magnitudes.iter().map(|m| m * m).sum::<f64>().sqrt();
            // A window can hide what the samples hold (see `spectrum`): with no
            // magnitude, there is nothing to measure.
            let measured = mean_square > silent_power && total > 0.0 && total.is_finite();
            let mut frame = SpectralFrame {
                start_frame: start,
                centroid_hz: None,
                flux: None,
            };
            if measured {
                let weighted: f64 = frequencies
                    .iter()
                    .zip(&magnitudes)
                    .map(|(f, m)| f * m)
                    .sum();
                frame.centroid_hz = Some(weighted / total);
                let shape: Vec<f64> = magnitudes.iter().map(|m| m / length).collect();
                if let Some(before) = &previous {
                    let distance = shape
                        .iter()
                        .zip(before)
                        .map(|(a, b)| (a - b) * (a - b))
                        .sum::<f64>()
                        .sqrt();
                    frame.flux = Some((distance / std::f64::consts::SQRT_2).min(1.0));
                }
                previous = Some(shape);
            } else {
                previous = None;
            }
            frames.push(frame);
        }
        Ok(SpectralTimeline {
            fft_size,
            hop,
            channel,
            silence_db,
            sample_rate: self.rate,
            input_frames,
            frames,
        })
    }
}
