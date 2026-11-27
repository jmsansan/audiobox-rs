use audiobox::analyze::{HarmonicEnergy, HarmonicOptions, PitchOptions};
use audiobox::{Audio, ErrorCode};
use std::f64::consts::PI;

const RATE: u32 = 48_000;

fn mono(samples: Vec<f32>) -> Audio {
    Audio::new(vec![samples], RATE).unwrap()
}

/// A sum of sines `(frequency, amplitude)`, `frames` long, with varied phases.
fn partials(list: &[(f64, f64)], frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            let t = i as f64 / RATE as f64;
            list.iter()
                .enumerate()
                .map(|(n, &(f, a))| a * (2.0 * PI * f * t + n as f64).sin())
                .sum::<f64>() as f32
        })
        .collect()
}

fn energy(samples: Vec<f32>, f0: f64) -> HarmonicEnergy {
    let frames = samples.len();
    mono(samples)
        .harmonic_energy(&HarmonicOptions::new(f0, 0..frames))
        .unwrap()
}

/// The power share of `part` among `all`, from their amplitudes.
fn power_share(part: &[f64], all: &[f64]) -> f64 {
    part.iter().map(|a| a * a).sum::<f64>() / all.iter().map(|a| a * a).sum::<f64>()
}

#[test]
fn exact_harmonics_are_all_harmonic() {
    for f0 in [110.0, 220.0, 523.25] {
        let list: Vec<(f64, f64)> = (1..=10).map(|k| (k as f64 * f0, 0.3 / k as f64)).collect();
        let e = energy(partials(&list, 48_000), f0);
        let share = e.harmonic_share.unwrap();
        assert!(share > 0.999, "{f0} Hz: {share}");
        assert!((e.inharmonic_share.unwrap() - (1.0 - share)).abs() < 1e-15);
        // Each harmonic's share is its power's.
        let amplitudes: Vec<f64> = list.iter().map(|p| p.1).collect();
        for (k, s) in e.harmonic_shares.iter().take(10).enumerate() {
            let expected = power_share(&amplitudes[k..=k], &amplitudes);
            assert!(
                (s - expected).abs() < 0.002,
                "{f0} Hz, harmonic {}: {s}",
                k + 1
            );
        }
        let total: f64 = e.harmonic_shares.iter().sum();
        assert!((total - share).abs() < 1e-12);
    }
}

#[test]
fn a_bell_partial_adds_its_energy_as_inharmonic() {
    // Harmonics of 200 Hz, then the same with a partial at 2.76 × 200 Hz.
    let harmonics: Vec<(f64, f64)> = (1..=6)
        .map(|k| (k as f64 * 200.0, 0.3 / k as f64))
        .collect();
    let plain = energy(partials(&harmonics, 48_000), 200.0);
    for bell in [0.05, 0.15, 0.3] {
        let mut list = harmonics.clone();
        list.push((2.76 * 200.0, bell));
        let e = energy(partials(&list, 48_000), 200.0);
        let amplitudes: Vec<f64> = list.iter().map(|p| p.1).collect();
        let expected = power_share(&[bell], &amplitudes);
        let raised = e.inharmonic_share.unwrap() - plain.inharmonic_share.unwrap();
        assert!(
            (raised - expected).abs() < 0.005,
            "{bell}: {raised} vs {expected}"
        );
    }
}

#[test]
fn a_naive_sawtooth_near_nyquist_shows_its_aliases() {
    // 1234.5 Hz: harmonics above 24 kHz fold back between the harmonics.
    let f0 = 1234.5;
    let naive: Vec<f32> = (0..48_000)
        .map(|i| {
            let phase = (i as f64 * f0 / RATE as f64).fract();
            (0.5 * (2.0 * phase - 1.0)) as f32
        })
        .collect();
    // The same sawtooth with only the harmonics below half the sample rate.
    let list: Vec<(f64, f64)> = (1..)
        .map(|k| (k as f64 * f0, 0.5 * 2.0 / (PI * k as f64)))
        .take_while(|p| p.0 < RATE as f64 / 2.0)
        .collect();
    let limited = energy(partials(&list, 48_000), f0);
    let aliased = energy(naive, f0);
    let (clean, dirty) = (
        limited.inharmonic_share.unwrap(),
        aliased.inharmonic_share.unwrap(),
    );
    assert!(clean < 0.001, "{clean}");
    assert!(dirty > 0.005 && dirty > 10.0 * clean, "{dirty} vs {clean}");
}

#[test]
fn a_fundamental_from_the_pitch_track_works_as_well() {
    let f0 = 196.0;
    let list: Vec<(f64, f64)> = (1..=8).map(|k| (k as f64 * f0, 0.3 / k as f64)).collect();
    let audio = mono(partials(&list, 48_000));
    let track = audio
        .pitch_track(&PitchOptions::new(60.0, 1000.0, 480))
        .unwrap();
    let estimated = track.median_frequency(0..48_000).unwrap().unwrap();
    let e = audio
        .harmonic_energy(&HarmonicOptions::new(estimated, 0..48_000))
        .unwrap();
    assert!(e.harmonic_share.unwrap() > 0.999, "{e:?}");
}

#[test]
fn settings_are_echoed_and_bands_are_bounded() {
    let e = energy(partials(&[(300.0, 0.5)], 10_000), 300.0);
    assert_eq!(e.options, HarmonicOptions::new(300.0, 0..10_000));
    assert_eq!(e.sample_rate, RATE);
    assert_eq!(e.frames, (10_000 - 4096) / 2048 + 1);
    assert_eq!(e.harmonics, 80);
    assert_eq!(e.band_hz.len(), 80);
    let bin = RATE as f64 / 4096.0;
    for (h, &band) in e.band_hz.iter().enumerate() {
        let cents = (h + 1) as f64 * 300.0 * (2f64.powf(25.0 / 1200.0) - 1.0);
        assert_eq!(band, cents.max(2.0 * bin).min(75.0), "harmonic {}", h + 1);
    }
    // A range: the second half only, silent, has nothing to share.
    let mut samples = partials(&[(300.0, 0.5)], 8192);
    samples.extend(vec![0.0; 8192]);
    let audio = mono(samples);
    let quiet = audio
        .harmonic_energy(&HarmonicOptions::new(300.0, 8192..16_384))
        .unwrap();
    assert_eq!((quiet.harmonic_share, quiet.inharmonic_share), (None, None));
    assert!(quiet.harmonic_shares.iter().all(|&s| s == 0.0));
    let empty = audio
        .harmonic_energy(&HarmonicOptions::new(300.0, 100..100))
        .unwrap();
    assert_eq!((empty.frames, empty.harmonic_share), (0, None));
    // Shorter than a frame: one padded frame.
    let short = audio
        .harmonic_energy(&HarmonicOptions::new(300.0, 0..1000))
        .unwrap();
    assert_eq!(short.frames, 1);
    // Its cut-off edges spread the tone: a share, but not a clean one.
    let share = short.harmonic_share.unwrap();
    assert!((0.0..=1.0).contains(&share), "{share}");
}

#[test]
fn channels_are_measured_alone_or_together() {
    let left = partials(&[(220.0, 0.5)], 16_384);
    let right = partials(&[(220.0 * 2.76, 0.5)], 16_384);
    let audio = Audio::new(vec![left, right], RATE).unwrap();
    let share = |channel| {
        audio
            .harmonic_energy(&HarmonicOptions {
                channel,
                ..HarmonicOptions::new(220.0, 0..16_384)
            })
            .unwrap()
            .inharmonic_share
            .unwrap()
    };
    assert!(share(Some(0)) < 0.001);
    assert!(share(Some(1)) > 0.999);
    assert!((share(None) - 0.5).abs() < 0.001);
}

#[test]
fn bad_arguments_are_refused() {
    let audio = mono(vec![0.5; 10_000]);
    let options = HarmonicOptions::new(220.0, 0..10_000);
    let code = |o: HarmonicOptions| audio.harmonic_energy(&o).unwrap_err().code;
    // 8 bins of a 4096-point FFT at 48 kHz: 93.75 Hz.
    let fine = audio.harmonic_energy(&HarmonicOptions::new(93.75, 0..10_000));
    assert!(fine.is_ok());
    for bad in [
        HarmonicOptions::new(93.7, 0..10_000),
        HarmonicOptions::new(24_000.0, 0..10_000),
        HarmonicOptions::new(f64::NAN, 0..10_000),
        HarmonicOptions::new(220.0, 0..10_001),
        HarmonicOptions {
            hop: 0,
            ..options.clone()
        },
        HarmonicOptions {
            fft_size: 1000,
            ..options.clone()
        },
        HarmonicOptions {
            fft_size: 8,
            ..options.clone()
        },
        HarmonicOptions {
            tolerance_cents: -1.0,
            ..options.clone()
        },
        HarmonicOptions {
            tolerance_cents: f64::INFINITY,
            ..options.clone()
        },
        HarmonicOptions {
            channel: Some(1),
            ..options.clone()
        },
    ] {
        assert_eq!(code(bad.clone()), ErrorCode::InvalidArgument, "{bad:?}");
    }
    #[allow(clippy::reversed_empty_ranges)]
    let reversed = 50..10;
    assert_eq!(
        code(HarmonicOptions::new(220.0, reversed)),
        ErrorCode::InvalidArgument
    );
    // The work limit, 2^32, at its edge: 87,381 frames × 4096 × 12 fit and
    // 87,382 do not.
    for (frames, fits) in [(87_381, true), (87_382, false)] {
        let long = Audio::new(vec![vec![0.0; frames + 4095]], RATE).unwrap();
        let options = HarmonicOptions {
            hop: 1,
            ..HarmonicOptions::new(220.0, 0..frames + 4095)
        };
        let result = long.harmonic_energy(&options);
        match fits {
            true => assert_eq!(result.unwrap().frames, frames),
            false => assert_eq!(result.unwrap_err().code, ErrorCode::LimitExceeded),
        }
    }
}
