use audiobox::analyze::{SpectralOptions, SpectralTimeline};
use audiobox::{Audio, ErrorCode};
use std::f64::consts::PI;

const RATE: u32 = 48_000;

fn mono(samples: Vec<f32>) -> Audio {
    Audio::new(vec![samples], RATE).unwrap()
}

/// A sum of sines `(frequency, amplitude, phase)`, `frames` long.
fn tones(partials: &[(f64, f64, f64)], frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            let t = i as f64 / RATE as f64;
            partials
                .iter()
                .map(|&(f, a, p)| a * (2.0 * PI * f * t + p).sin())
                .sum::<f64>() as f32
        })
        .collect()
}

fn timeline(audio: &Audio, fft: usize, hop: usize) -> SpectralTimeline {
    audio
        .spectral_timeline(&SpectralOptions::new(fft, hop))
        .unwrap()
}

/// The summary of the frames that lie wholly inside the input.
fn inner(t: &SpectralTimeline) -> audiobox::analyze::SpectralSummary {
    t.summary(0..t.input_frames + 1 - t.fft_size).unwrap()
}

#[test]
fn a_sine_has_its_frequency_as_centroid() {
    for f in [250.0, 1000.0, 5000.0] {
        let t = timeline(&mono(tones(&[(f, 0.5, 0.0)], 48_000)), 4096, 2048);
        assert_eq!(inner(&t).measured, t.frames.len());
        for frame in &t.frames {
            let c = frame.centroid_hz.unwrap();
            assert!((c - f).abs() / f < 0.01, "{f} Hz: centroid {c}");
        }
    }
}

#[test]
fn a_harmonic_tone_has_its_analytic_centroid() {
    // Harmonics k = 1..=10 of 200 Hz at amplitude 1 / k: the centroid is
    // 200 × Σ k (1 / k) / Σ 1 / k = 2000 / H(10).
    let partials: Vec<(f64, f64, f64)> = (1..=10)
        .map(|k| (200.0 * k as f64, 0.3 / k as f64, k as f64))
        .collect();
    let t = timeline(&mono(tones(&partials, 48_000)), 4096, 2048);
    let harmonic: f64 = (1..=10).map(|k| 1.0 / k as f64).sum();
    let expected = 2000.0 / harmonic;
    // Within 2 %, in every frame: the window's leakage spreads a little magnitude
    // over other bins.
    for frame in &t.frames {
        let c = frame.centroid_hz.unwrap();
        assert!((c - expected).abs() / expected < 0.02, "{c} vs {expected}");
    }
}

#[test]
fn band_limited_noise_centers_on_its_band() {
    // Equal-amplitude partials on every bin from 2 to 4 kHz with random phases.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let bin = RATE as f64 / 4096.0;
    let partials: Vec<(f64, f64, f64)> = (0..)
        .map(|k| 2000.0 + k as f64 * bin)
        .take_while(|&f| f <= 4000.0)
        .map(|f| (f, 0.01, 2.0 * PI * random()))
        .collect();
    let t = timeline(&mono(tones(&partials, 24_000)), 4096, 2048);
    let summary = inner(&t);
    let c = summary.centroid_mean_hz.unwrap();
    assert!((c - 3000.0).abs() < 30.0, "{c}");
    assert!(summary.centroid_min_hz.unwrap() > 2900.0);
    assert!(summary.centroid_max_hz.unwrap() < 3100.0);
}

#[test]
fn a_steady_sound_has_no_flux() {
    // A constant level gives identical frames: exactly no change.
    let t = timeline(&mono(vec![0.25; 8192]), 1024, 512);
    let summary = inner(&t);
    assert_eq!(summary.flux_max, Some(0.0));
    // Its centroid is the DC bin's, spread by the window into the next bin.
    assert_eq!(summary.measured, t.frames.len());
    for frame in &t.frames {
        let c = frame.centroid_hz.unwrap();
        assert!(c.is_finite() && c > 0.0 && c < 48_000.0 / 1024.0, "{c}");
    }
    // A steady chord at any phase: next to none.
    let chord = tones(
        &[(440.0, 0.3, 0.0), (660.0, 0.2, 1.0), (1320.0, 0.1, 2.0)],
        48_000,
    );
    let summary = inner(&timeline(&mono(chord), 4096, 1000));
    assert!(summary.flux_max.unwrap() < 0.01, "{summary:?}");
    // The same shape at another level is the same spectrum.
    let mut swell = tones(&[(1000.0, 1.0, 0.0)], 48_000);
    for (i, s) in swell.iter_mut().enumerate() {
        *s *= 0.1 + 0.9 * (i as f32 / 48_000.0);
    }
    let summary = inner(&timeline(&mono(swell), 2048, 2048));
    assert!(summary.flux_max.unwrap() < 0.01, "{summary:?}");
}

#[test]
fn a_change_of_spectrum_shows_as_flux_and_a_centroid_step() {
    // 500 Hz for half a second, then 4 kHz.
    let mut samples = tones(&[(500.0, 0.5, 0.0)], 24_000);
    samples.extend(tones(&[(4000.0, 0.5, 0.0)], 24_000));
    let t = timeline(&mono(samples), 2048, 1024);
    let before = t.summary(0..24_000 - 2048).unwrap();
    // Flux compares with the frame before, so the first frame after the change
    // still sees it.
    let after = t.summary(24_000 + 2048..48_000 - 2048).unwrap();
    assert!((before.centroid_mean_hz.unwrap() - 500.0).abs() < 10.0);
    assert!((after.centroid_mean_hz.unwrap() - 4000.0).abs() < 40.0);
    assert!(before.flux_max.unwrap() < 0.01 && after.flux_max.unwrap() < 0.01);
    // The frames across the change have flux; the largest is near 1.
    let across = t.summary(24_000 - 2048..24_000 + 2048).unwrap();
    assert!(across.flux_max.unwrap() > 0.5, "{across:?}");
    let all = t.summary(0..48_000).unwrap();
    assert_eq!(all.flux_max, across.flux_max);
}

#[test]
fn silence_and_short_inputs_measure_nothing_or_what_they_hold() {
    let t = timeline(&mono(vec![0.0; 4000]), 1024, 1024);
    assert_eq!(t.frames.len(), 3);
    let summary = t.summary(0..4000).unwrap();
    assert_eq!((summary.frames, summary.measured), (3, 0));
    assert_eq!(summary.centroid_mean_hz, None);
    assert_eq!(summary.flux_mean, None);
    for frame in &t.frames {
        assert_eq!((frame.centroid_hz, frame.flux), (None, None));
    }

    let empty = Audio::new(vec![Vec::new()], RATE).unwrap();
    let t = timeline(&empty, 1024, 256);
    assert!(t.frames.is_empty());
    assert_eq!(t.summary(0..0).unwrap().frames, 0);

    // Shorter than one frame: zero-padded, measured once, with no flux.
    let t = timeline(&mono(tones(&[(3000.0, 0.5, 0.0)], 500)), 1024, 256);
    assert_eq!(t.frames.len(), 1);
    let first = t.frames[0];
    let c = first.centroid_hz.unwrap();
    // Cut off by the padding, the tone spreads upward: finite, above its pitch.
    assert!(c.is_finite() && c > 2700.0 && c < 12_000.0, "{c}");
    assert_eq!(first.flux, None);

    // A silent frame between two sounds: no flux on either side of it.
    let mut samples = tones(&[(1000.0, 0.5, 0.0)], 1024);
    samples.extend(vec![0.0; 1024]);
    samples.extend(tones(&[(1000.0, 0.5, 0.0)], 1024));
    let t = timeline(&mono(samples), 1024, 1024);
    let flux: Vec<_> = t.frames.iter().map(|f| f.flux).collect();
    assert_eq!(flux, [None, None, None]);
    assert_eq!(t.frames[1].centroid_hz, None);
}

#[test]
fn channels_are_measured_alone_or_together() {
    let left = tones(&[(500.0, 0.5, 0.0)], 8192);
    let right = tones(&[(2000.0, 0.5, 0.0)], 8192);
    let audio = Audio::new(vec![left.clone(), right], RATE).unwrap();
    let centroid = |channel| {
        let t = audio
            .spectral_timeline(&SpectralOptions {
                channel,
                ..SpectralOptions::new(2048, 2048)
            })
            .unwrap();
        inner(&t).centroid_mean_hz.unwrap()
    };
    assert!((centroid(Some(0)) - 500.0).abs() < 10.0);
    assert!((centroid(Some(1)) - 2000.0).abs() < 20.0);
    let both = centroid(None);
    assert!((both - 1250.0).abs() < 20.0, "{both}");
    // Opposite phases do not cancel.
    let inverted: Vec<f32> = left.iter().map(|s| -s).collect();
    let audio = Audio::new(vec![left, inverted], RATE).unwrap();
    let t = timeline(&audio, 2048, 2048);
    assert!((inner(&t).centroid_mean_hz.unwrap() - 500.0).abs() < 10.0);
}

#[test]
fn results_are_finite_and_repeatable() {
    let samples = tones(&[(330.0, 0.4, 0.0), (2500.0, 0.1, 0.5)], 20_000);
    let audio = mono(samples);
    let a = timeline(&audio, 1024, 300);
    assert_eq!(a, timeline(&audio, 1024, 300));
    assert_eq!(a.frames.len(), (20_000 - 1024) / 300 + 1);
    for frame in &a.frames {
        if let Some(c) = frame.centroid_hz {
            assert!(c.is_finite() && (0.0..=24_000.0).contains(&c));
        }
        if let Some(f) = frame.flux {
            assert!(f.is_finite() && (0.0..=1.0).contains(&f));
        }
    }
}

#[test]
fn bad_arguments_are_refused() {
    let audio = mono(vec![0.5; 100]);
    let code = |options: SpectralOptions| audio.spectral_timeline(&options).unwrap_err().code;
    let options = SpectralOptions::new(64, 32);
    for bad in [
        SpectralOptions { hop: 0, ..options },
        SpectralOptions {
            fft_size: 100,
            ..options
        },
        SpectralOptions {
            fft_size: 8,
            ..options
        },
        SpectralOptions {
            channel: Some(1),
            ..options
        },
        SpectralOptions {
            silence_db: f64::NAN,
            ..options
        },
        SpectralOptions {
            silence_db: f64::NEG_INFINITY,
            ..options
        },
    ] {
        assert_eq!(code(bad), ErrorCode::InvalidArgument, "{bad:?}");
    }
    let t = audio.spectral_timeline(&options).unwrap();
    assert_eq!(
        t.summary(0..101).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    #[allow(clippy::reversed_empty_ranges)]
    let reversed = 50..10;
    assert_eq!(
        t.summary(reversed).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    // More analysis frames than the limit.
    let long = Audio::new(vec![vec![0.0; (1 << 20) + 16]], RATE).unwrap();
    assert_eq!(
        long.spectral_timeline(&SpectralOptions::new(16, 1))
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    // More samples transformed than the limit, in fewer frames.
    let long = Audio::new(vec![vec![0.0; 524_288 + 4096]], RATE).unwrap();
    assert_eq!(
        long.spectral_timeline(&SpectralOptions::new(4096, 1))
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
}

#[test]
fn the_last_frames_are_whole_and_a_lone_sample_measures_nothing() {
    // A steady sine over the whole input: no padded frame at the end shows flux.
    let t = timeline(&mono(tones(&[(1000.0, 0.5, 0.0)], 48_000)), 2048, 1024);
    let last = t.frames.last().unwrap();
    assert!(last.start_frame + 2048 <= 48_000);
    let summary = t.summary(0..48_000).unwrap();
    assert!(summary.flux_max.unwrap() < 0.01, "{summary:?}");
    // One loud sample at the start: the window hides it.
    let t = timeline(&mono(vec![0.5]), 16, 4);
    assert_eq!(t.frames.len(), 1);
    assert_eq!((t.frames[0].centroid_hz, t.frames[0].flux), (None, None));
}
