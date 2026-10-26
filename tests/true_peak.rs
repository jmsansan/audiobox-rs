use audiobox::*;
use std::f64::consts::PI;

fn audio(samples: Vec<f32>, rate: u32) -> Audio {
    Audio::new(vec![samples], rate).unwrap()
}

fn square(frames: usize, amplitude: f32, rate: u32) -> Audio {
    audio(
        (0..frames)
            .map(|i| if i % 4 < 2 { amplitude } else { -amplitude })
            .collect(),
        rate,
    )
}

fn signal(frames: usize, rate: u32, channels: usize) -> Audio {
    let mut seed = 0x2545_f491u32;
    Audio::new(
        (0..channels)
            .map(|c| {
                (0..frames)
                    .map(|i| {
                        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                        noise * 0.2
                            + if (i / (3 + c * 2)) % 2 == 0 {
                                1.4
                            } else {
                                -1.4
                            }
                    })
                    .collect()
            })
            .collect(),
        rate,
    )
    .unwrap()
}

// Independent direct reconstruction: 16 phases, a 96-tap Kaiser window (beta 9),
// platform trigonometry and explicit zero padding. Does not call engine resamplers
// or its true-peak detector. Source samples themselves remain part of the maximum.
fn reference_peak(source: &Audio) -> f64 {
    let i0 = |x: f64| {
        let mut sum = 1.0;
        let mut term = 1.0;
        for n in 1..=40 {
            term *= (x * x) / (4.0 * (n * n) as f64);
            sum += term;
        }
        sum
    };
    let norm = i0(9.0);
    let mut kernels = [[0.0; 96]; 16];
    for (phase, kernel) in kernels.iter_mut().enumerate() {
        for (tap, value) in kernel.iter_mut().enumerate() {
            let t = tap as f64 - 47.0 - phase as f64 / 16.0;
            let sinc = if t == 0.0 {
                1.0
            } else {
                (PI * t).sin() / (PI * t)
            };
            *value = sinc * i0(9.0 * (1.0 - (t / 48.0).powi(2)).max(0.0).sqrt()) / norm;
        }
        let total: f64 = kernel.iter().sum();
        for value in kernel {
            *value /= total;
        }
    }
    let mut peak = source.peak();
    for channel in source.all_channels() {
        for frame in -48..source.frames() as i64 + 48 {
            for kernel in &kernels {
                let mut sample = 0.0;
                for (tap, &k) in kernel.iter().enumerate() {
                    let at = frame + tap as i64 - 47;
                    if at >= 0 && (at as usize) < channel.len() {
                        sample += channel[at as usize] as f64 * k;
                    }
                }
                peak = peak.max(sample.abs());
            }
        }
    }
    peak
}

fn db(value: f64) -> f64 {
    if value > 0.0 {
        20.0 * value.log10()
    } else {
        f64::NEG_INFINITY
    }
}

fn hash(source: &Audio) -> u64 {
    pcm::encode_pcm(source, SampleFormat::F32, Endian::Little)
        .unwrap()
        .iter()
        .fold(0xcbf29ce484222325u64, |h, &b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        })
}

#[test]
fn intersample_peaks_pass_sample_limiting_but_meet_the_new_ceiling() {
    for rate in [1000, 8000, 32000, 44100, 48000, 96000, 192000, 768000] {
        let input = square(257, 1.0, rate);
        let samples = input.limit(LimiterOptions::default()).unwrap();
        assert!(samples.peak_db() <= -1.0 + 0.00001);
        assert!(db(reference_peak(&samples)) > -0.5);
        let limited = input
            .limit_true_peak(TruePeakLimiterOptions::default())
            .unwrap();
        assert!(limited.true_peak_db() <= -1.3 + 0.0001, "{rate} Hz");
        assert!(
            db(reference_peak(&limited)) <= -1.0 + 0.05,
            "{rate} Hz: independent {} dBTP",
            db(reference_peak(&limited))
        );
        assert_eq!(limited.frames(), input.frames());
        assert_eq!(limited.sample_rate(), rate);
    }
}

#[test]
fn independent_reconstruction_checks_edges_impulses_noise_and_near_nyquist() {
    let mut impulse = vec![0.0; 257];
    impulse[0] = 4.0;
    impulse[256] = -4.0;
    let mut burst = vec![0.0; 257];
    for (i, sample) in burst[80..180].iter_mut().enumerate() {
        *sample = if i % 4 < 2 { 3.0 } else { -3.0 };
    }
    let high = |frequency: f64| {
        (0..257)
            .map(|i| (2.0 * (2.0 * PI * frequency * i as f64 + 0.37).sin()) as f32)
            .collect()
    };
    for source in [
        audio(impulse, 48000),
        audio(burst, 48000),
        audio(high(0.45), 44100),
        audio(high(0.49), 48000),
        signal(257, 48000, 2),
        Audio::new(vec![vec![1.0; 97], vec![-0.5; 97]], 8000).unwrap(),
    ] {
        let out = source
            .limit_true_peak(TruePeakLimiterOptions::default())
            .unwrap();
        let independent = db(reference_peak(&out));
        assert!(independent <= -0.95, "independent peak: {independent} dBTP");
        assert!(out.true_peak_db() <= -1.2999);
        assert_eq!(out.frames(), source.frames());
        assert_eq!(out.channels(), source.channels());
        assert!(out.all_channels().iter().flatten().all(|s| s.is_finite()));
    }
}

#[test]
fn empty_silent_quiet_and_short_inputs_preserve_length_and_alignment() {
    for frames in [0, 1, 2, 31, 32, 33, 63, 64, 65, 257] {
        let silence = audio(vec![0.0; frames], 48000);
        let out = silence
            .limit_true_peak(TruePeakLimiterOptions::default())
            .unwrap();
        assert_eq!(hash(&out), hash(&silence));
        let quiet = square(frames, 0.05, 48000);
        let out = quiet
            .limit_true_peak(TruePeakLimiterOptions::default())
            .unwrap();
        assert_eq!(hash(&out), hash(&quiet));
        let hot = square(frames, 4.0, 48000);
        let out = hot
            .limit_true_peak(TruePeakLimiterOptions::default())
            .unwrap();
        assert_eq!(out.frames(), frames);
        assert!(out.true_peak_db() <= -1.2999);
    }
    let mut impulse = vec![0.0; 1024];
    impulse[400] = 4.0;
    let input = audio(impulse, 48000);
    let out = input
        .limit_true_peak(TruePeakLimiterOptions::default())
        .unwrap();
    assert_eq!(out.frames(), input.frames());
    let nonzero: Vec<_> = out
        .channel_data(0)
        .unwrap()
        .iter()
        .enumerate()
        .filter(|(_, s)| **s != 0.0)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(nonzero, [400]);
}

#[test]
fn channels_are_linked_and_release_recovers_on_the_existing_tail() {
    let mut samples = vec![0.1; 4096];
    samples[500] = 4.0;
    let input = Audio::new(
        vec![samples.iter().map(|s| s * 0.25).collect(), samples],
        8000,
    )
    .unwrap();
    let mut outputs = Vec::new();
    for release_ms in [5.0, 50.0] {
        let out = input
            .limit_true_peak(TruePeakLimiterOptions {
                release_ms,
                lookahead_ms: 0.0,
                ..Default::default()
            })
            .unwrap();
        let left = out.channel_data(0).unwrap();
        let right = out.channel_data(1).unwrap();
        for (&l, &r) in left.iter().zip(right) {
            assert_eq!(l * 4.0, r);
        }
        assert!(right[1000] < right[3500]);
        outputs.push(right[1000] / right[3500]);
    }
    assert!(
        outputs[0] > outputs[1] + 0.1,
        "release recovery: {outputs:?}"
    );
}

#[test]
fn invalid_settings_and_workspace_bounds_fail_before_rendering() {
    let input = signal(512, 48000, 2);
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for settings in [
            TruePeakLimiterOptions {
                ceiling_db: value,
                ..Default::default()
            },
            TruePeakLimiterOptions {
                lookahead_ms: value,
                ..Default::default()
            },
            TruePeakLimiterOptions {
                release_ms: value,
                ..Default::default()
            },
            TruePeakLimiterOptions {
                safety_margin_db: value,
                ..Default::default()
            },
        ] {
            assert_eq!(
                input.limit_true_peak(settings).unwrap_err().code,
                ErrorCode::InvalidArgument
            );
        }
    }
    for settings in [
        TruePeakLimiterOptions {
            lookahead_ms: -1.0,
            ..Default::default()
        },
        TruePeakLimiterOptions {
            release_ms: 0.0,
            ..Default::default()
        },
        TruePeakLimiterOptions {
            release_ms: -1.0,
            ..Default::default()
        },
        TruePeakLimiterOptions {
            safety_margin_db: -0.1,
            ..Default::default()
        },
        TruePeakLimiterOptions {
            safety_margin_db: 12.1,
            ..Default::default()
        },
        TruePeakLimiterOptions {
            ceiling_db: f64::MAX,
            ..Default::default()
        },
        TruePeakLimiterOptions {
            ceiling_db: -f64::MAX,
            ..Default::default()
        },
    ] {
        assert_eq!(
            input.limit_true_peak(settings).unwrap_err().code,
            ErrorCode::InvalidArgument
        );
    }
    for budget in [0, 1, 100] {
        assert_eq!(
            input
                .limit_true_peak(TruePeakLimiterOptions {
                    max_working_bytes: budget,
                    ..Default::default()
                })
                .unwrap_err()
                .code,
            ErrorCode::LimitExceeded
        );
    }
    // A longer lookahead consumes a larger prepared queue, while the input/output
    // shape and caller-selected budget stay fixed.
    input
        .limit_true_peak(TruePeakLimiterOptions {
            max_working_bytes: 10000,
            lookahead_ms: 0.0,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        input
            .limit_true_peak(TruePeakLimiterOptions {
                max_working_bytes: 10000,
                lookahead_ms: 1000.0,
                ..Default::default()
            })
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    input
        .limit_true_peak(TruePeakLimiterOptions {
            lookahead_ms: f64::MAX,
            ..Default::default()
        })
        .unwrap();
}

#[test]
fn extreme_samples_and_zero_margin_remain_finite_and_bounded() {
    for source in [
        square(129, f32::MAX, 48000),
        audio(vec![f32::from_bits(1); 97], 1000),
        audio(vec![f32::MAX], 768000),
    ] {
        for safety_margin_db in [0.0, 0.3, 12.0] {
            let out = source
                .limit_true_peak(TruePeakLimiterOptions {
                    safety_margin_db,
                    ..Default::default()
                })
                .unwrap();
            assert!(out.all_channels().iter().flatten().all(|s| s.is_finite()));
            assert!(out.true_peak_db() <= -1.0 - safety_margin_db + 0.0001);
        }
    }
    let quiet = square(97, 0.01, 48000);
    let out = quiet
        .limit_true_peak(TruePeakLimiterOptions {
            ceiling_db: -3000.0,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(out.peak(), 0.0);
}

#[test]
fn output_has_cross_host_golden_hashes_and_repeatable_exports() {
    let mut hashes = Vec::new();
    for (rate, channels) in [
        (1000, 1),
        (8000, 2),
        (32000, 4),
        (44100, 2),
        (48000, 6),
        (96000, 8),
        (768000, 1),
    ] {
        let source = signal(1025, rate, channels);
        for lookahead_ms in [0.0, 5.0] {
            let settings = TruePeakLimiterOptions {
                lookahead_ms,
                ..Default::default()
            };
            let out = source.limit_true_peak(settings).unwrap();
            assert_eq!(hash(&out), hash(&source.limit_true_peak(settings).unwrap()));
            hashes.push(hash(&out));
            if channels <= 2 {
                let bytes = encode_with_options(
                    &out,
                    AudioFormat::Wav,
                    &EncodeOptions {
                        dither: Dither::Tpdf { seed: 7 },
                        ..Default::default()
                    },
                )
                .unwrap();
                let stream = decode(&bytes).unwrap();
                assert_eq!(stream.frames(), source.frames());
            }
        }
    }
    assert_eq!(
        hashes,
        [
            0x83badf2270c89635,
            0x43ca22ef77757f5f,
            0x3b2d55451da4b81c,
            0x3f9ef85235c28146,
            0x5ecf60ca8c6a7cdd,
            0xf55ffa87c7b15ce2,
            0x41c5ea5b340d991b,
            0xbdcbeaa695fa9241,
            0x4cda8b1362630327,
            0x1d100e1d6806937d,
            0xc896b86846ca840a,
            0x8755679e3433b541,
            0x403a567b07b995e3,
            0xf18816c9cf36bd29,
        ]
    );
}

#[test]
fn fast_release_keeps_gain_modulation_overs_below_the_ceiling() {
    let source = signal(641, 48000, 2);
    for release_ms in [0.02, 0.1, 1.0] {
        let out = source
            .limit_true_peak(TruePeakLimiterOptions {
                lookahead_ms: 0.0,
                release_ms,
                ..Default::default()
            })
            .unwrap();
        assert!(out.true_peak_db() <= -1.2999);
        assert!(db(reference_peak(&out)) <= -0.95);
    }
}
