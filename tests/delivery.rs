//! Public contracts consumed by Studio: spectral units, linear power, resampling
//! boundaries and deterministic validation of the actual delivered PCM.
use audiobox::*;

fn signal(frames: usize, rate: u32, channels: usize) -> Audio {
    let mut seed = 7u32;
    Audio::new(
        (0..channels)
            .map(|c| {
                (0..frames)
                    .map(|i| {
                        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                        (seed >> 8) as f32 / (1u32 << 24) as f32 / 16.0
                            + if i % (11 + c) < 5 { 0.25 } else { -0.25 }
                    })
                    .collect()
            })
            .collect(),
        rate,
    )
    .unwrap()
}

fn bits(audio: &Audio) -> Vec<u32> {
    audio
        .all_channels()
        .iter()
        .flatten()
        .map(|s| s.to_bits())
        .collect()
}

fn values(m: &analyze::DeterministicMeasurements) -> [f64; 5] {
    [
        m.loudness.integrated,
        m.loudness.range,
        m.loudness.peak_momentary,
        m.peak_db,
        m.true_peak_db,
    ]
}

fn hash(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

#[test]
fn spectrum_units_endpoints_and_partial_windows() {
    let n = 256;
    for level in [0.0, 0.25, -2.0] {
        let dc = analyze::spectrum(&vec![level; n], 8000, n).unwrap();
        assert_eq!(dc.frequencies.len(), n / 2 + 1);
        assert_eq!(dc.frequencies[0], 0.0);
        assert_eq!(dc.frequencies[n / 2], 4000.0);
        assert_eq!(dc.frequencies[1], 31.25);
        assert!((dc.magnitudes[0] - level.abs() as f64).abs() < 1e-12);
        let nyquist: Vec<f32> = (0..n)
            .map(|i| if i % 2 == 0 { level } else { -level })
            .collect();
        let nyquist = analyze::spectrum(&nyquist, 8000, n).unwrap();
        assert!((nyquist.magnitudes[n / 2] - level.abs() as f64).abs() < 1e-12);
    }
    let tone: Vec<f32> = (0..n)
        .map(|i| (std::f64::consts::TAU * 17.0 * i as f64 / n as f64).sin() as f32 * 0.5)
        .collect();
    let tone = analyze::spectrum(&tone, 8000, n).unwrap();
    assert!((tone.magnitudes[17] - 0.5).abs() < 0.00001);
    for input in [&[][..], &[0.25][..], &[0.0, 0.25][..]] {
        let short = analyze::spectrum(input, 8000, n).unwrap();
        let mut padded = input.to_vec();
        padded.resize(n, 0.0);
        assert_eq!(
            short.magnitudes,
            analyze::spectrum(&padded, 8000, n).unwrap().magnitudes
        );
        if input.len() < 2 {
            assert!(short.magnitudes.iter().all(|&x| x == 0.0));
        }
    }
    let mut endpoints = vec![0.0; n];
    endpoints[0] = 2.0;
    endpoints[n - 1] = -3.0;
    assert!(
        analyze::spectrum(&endpoints, 8000, n)
            .unwrap()
            .magnitudes
            .iter()
            .all(|&x| x == 0.0)
    );
    let full = signal(n, 8000, 1).channel_data(0).unwrap().to_vec();
    let mut extra = full.clone();
    extra.extend([10.0; 17]);
    assert_eq!(
        analyze::spectrum(&full, 8000, n).unwrap().magnitudes,
        analyze::spectrum(&extra, 8000, n).unwrap().magnitudes
    );
    extra.push(f32::NAN);
    assert_eq!(
        analyze::spectrum(&extra, 8000, n).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert!(
        analyze::spectrum(&[1.0, 2.0], 8000, 2)
            .unwrap()
            .magnitudes
            .iter()
            .all(|x| x.is_nan())
    );
}

#[test]
fn linear_power_has_explicit_channel_and_frame_denominators() {
    for channels in [1, 2, 6, 64] {
        for frames in [0, 1, 17, 1000] {
            for level in [0.0f32, -0.25, 2.0, f32::MAX, f32::from_bits(1)] {
                let audio = Audio::new(vec![vec![level; frames]; channels], 8000).unwrap();
                let expected = if frames == 0 {
                    0.0
                } else {
                    (level as f64).powi(2)
                };
                if expected == 0.0 {
                    assert_eq!(audio.mean_square(), 0.0);
                } else {
                    assert!((audio.mean_square() / expected - 1.0).abs() < 1e-11);
                }
                let legacy = if frames == 0 {
                    0.0
                } else {
                    (audio
                        .all_channels()
                        .iter()
                        .flatten()
                        .map(|&s| (s as f64).powi(2))
                        .sum::<f64>()
                        / (frames * channels) as f64)
                        .sqrt()
                };
                assert_eq!(audio.rms(), legacy);
                assert_eq!(audio.rms_db().to_bits(), analyze::to_db(legacy).to_bits());
            }
        }
    }
    let stereo = Audio::new(vec![vec![0.5; 10], vec![0.0; 10]], 8000).unwrap();
    assert_eq!(stereo.mean_square(), 0.125);
    let short = Audio::new(vec![vec![0.5; 3]], 8000).unwrap();
    let long = Audio::new(vec![vec![0.25; 7]], 8000).unwrap();
    let combined = short.concat(&long).unwrap();
    assert_eq!(
        combined.mean_square(),
        (short.mean_square() * 3.0 + long.mean_square() * 7.0) / 10.0
    );
    assert_eq!(
        short
            .pad(0.0, TimePosition::Frames(7))
            .unwrap()
            .mean_square(),
        short.mean_square() * 3.0 / 10.0
    );
    let tone = Audio::new(
        vec![
            (0..4000)
                .map(|i| match i % 4 {
                    0 | 2 => 0.0,
                    1 => 0.5,
                    _ => -0.5,
                })
                .collect(),
        ],
        8000,
    )
    .unwrap();
    assert_eq!(tone.mean_square(), 0.125);
}

#[test]
fn resampling_maps_absolute_boundaries_to_the_actual_output() {
    let mut fingerprint = Vec::new();
    for from in [32000, 44100, 48000, 96000, 192000] {
        for to in [32000, 44100, 48000, 96000, 192000] {
            for n in [0, 1, 2, 79, 80, 81, 160, 257] {
                let audio = signal(n, from, 1);
                let out = audio.resample(to).unwrap();
                assert_eq!(out.frames(), audio.resampled_frame_index(n, to).unwrap());
                let mut previous = 0;
                for boundary in 0..=n {
                    let mapped = audio.resampled_frame_index(boundary, to).unwrap();
                    assert!(mapped >= previous && mapped <= out.frames());
                    previous = mapped;
                    fingerprint.extend_from_slice(&(mapped as u64).to_le_bytes());
                }
                if from == to {
                    assert_eq!(bits(&out), bits(&audio));
                }
                assert_eq!(audio.resampled_frame_index(0, to).unwrap(), 0);
                assert_eq!(
                    audio.resampled_frame_index(n + 1, to).unwrap_err().code,
                    ErrorCode::InvalidArgument
                );
            }
        }
    }
    let audio = signal(160, 48000, 1);
    assert_eq!(audio.resampled_frame_index(80, 44100).unwrap(), 74);
    assert_eq!(audio.resampled_frame_index(160, 44100).unwrap(), 147);
    assert_eq!(
        audio.resampled_frame_index(0, 0).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        audio.resampled_frame_index(0, 768001).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    // The legacy floating result differs from exact rational halves-up rounding.
    let tie = signal(500, 1000, 1);
    assert_eq!(tie.resampled_frame_index(500, 1001).unwrap(), 500);
    assert_eq!(tie.resample(1001).unwrap().frames(), 500);
    assert_eq!((500u64 * 1001 + 500) / 1000, 501);
    assert_eq!(
        hash(fingerprint),
        12474448276517550570,
        "boundary mapping hash"
    );
}

#[test]
fn resampling_has_source_time_zero_and_clamped_edges_without_added_tail() {
    for n in [1, 2, 31, 80, 101] {
        for (from, to) in [(48000, 44100), (32000, 192000), (192000, 32000)] {
            let dc = Audio::new(vec![vec![0.25; n]], from).unwrap();
            let out = dc.resample(to).unwrap();
            assert!(
                out.channel_data(0)
                    .unwrap()
                    .iter()
                    .all(|&x| (x - 0.25).abs() < 1e-6)
            );
            let mut impulse = vec![0.0; n];
            impulse[0] = 1.0;
            let impulse = Audio::new(vec![impulse], from)
                .unwrap()
                .resample(to)
                .unwrap();
            // A symmetric kernel centered on source zero reads the clamped first
            // sample to its left; an impulse away from edges would differ.
            if impulse.frames() > 0 {
                assert!(impulse.channel_data(0).unwrap()[0] > 0.5);
            }
            assert_eq!(impulse.frames(), out.frames());
        }
    }
}

#[test]
fn deterministic_measurements_match_render_controls_without_touching_pcm() {
    for (rate, channels, frames) in [
        (4000, 1, 2001),
        (8000, 2, 4001),
        (32000, 4, 16001),
        (44100, 5, 22051),
        (48000, 6, 24001),
        (96000, 8, 123),
        (192000, 1, 31),
        (768000, 2, 31),
    ] {
        let audio = signal(frames, rate, channels);
        let before = bits(&audio);
        let measured = audio.measure_deterministic().unwrap();
        for (unit, expected) in [
            (NormalizeUnit::Peak, measured.peak_db),
            (NormalizeUnit::TruePeak, measured.true_peak_db),
            (NormalizeUnit::Lufs, measured.loudness.integrated),
        ] {
            let (_, report) = audio
                .normalize_with_report(NormalizeOptions {
                    to: -18.0,
                    unit,
                    peak_ceiling_db: 6.0,
                })
                .unwrap();
            assert_eq!(expected.to_bits(), report.measured.to_bits());
        }
        let reference = audio.loudness();
        for (a, b) in [
            (measured.loudness.integrated, reference.integrated),
            (measured.loudness.range, reference.range),
            (measured.loudness.peak_momentary, reference.peak_momentary),
            (measured.true_peak_db, audio.true_peak_db()),
        ] {
            assert!((a - b).abs() <= 0.0001, "{a} vs {b} at {rate} Hz");
        }
        assert_eq!(
            values(&measured).map(f64::to_bits),
            values(&audio.measure_deterministic().unwrap()).map(f64::to_bits)
        );
        assert_eq!(bits(&audio), before);
    }
}

#[test]
fn deterministic_measurements_define_empty_gated_lfe_short_and_overflow() {
    for frames in [0, 1, 31, 3199, 3200, 4000] {
        let audio = Audio::new(vec![vec![0.0; frames]; 2], 8000).unwrap();
        assert_eq!(
            values(&audio.measure_deterministic().unwrap()),
            [
                f64::NEG_INFINITY,
                0.0,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY
            ]
        );
    }
    let quiet = signal(4000, 8000, 1).gain(1e-10).unwrap();
    assert_eq!(
        quiet.measure_deterministic().unwrap().loudness.integrated,
        f64::NEG_INFINITY
    );
    assert!(
        quiet
            .cut(TimePosition::Frames(0), TimePosition::Frames(100))
            .unwrap()
            .measure_deterministic()
            .unwrap()
            .loudness
            .integrated
            .is_finite()
    );
    let mut lfe = vec![vec![0.0; 4000]; 6];
    lfe[3].fill(0.25);
    let lfe = Audio::new(lfe, 8000)
        .unwrap()
        .measure_deterministic()
        .unwrap();
    assert_eq!(lfe.loudness.integrated, f64::NEG_INFINITY);
    assert!(lfe.true_peak_db.is_finite());
    for rate in [1000, 3363, 3999] {
        for frames in [0, 1, 31] {
            assert_eq!(
                Audio::new(vec![vec![0.0; frames]], rate)
                    .unwrap()
                    .measure_deterministic()
                    .unwrap_err()
                    .code,
                ErrorCode::InvalidArgument
            );
        }
    }
    assert_eq!(
        Audio::new(vec![vec![f32::MAX; 31]], 48000)
            .unwrap()
            .measure_deterministic()
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    // A custom decoder may accept shapes beyond the default analysis budget.
    let mut wav = encode(&signal(31, 48000, 1), AudioFormat::Wav).unwrap();
    wav[24..28].copy_from_slice(&800000u32.to_le_bytes());
    wav[28..32].copy_from_slice(&1600000u32.to_le_bytes());
    let expanded = decode_with_options(
        &wav,
        &DecodeOptions {
            limits: Limits {
                max_sample_rate: 800000,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        expanded.measure_deterministic().unwrap_err().code,
        ErrorCode::LimitExceeded
    );
}

#[test]
fn delivery_measurements_after_limiter_resample_and_seeded_export_are_pinned() {
    let mut fingerprint = Vec::new();
    for rate in [32000, 44100, 48000] {
        let audio = signal(1601, 48000, 2)
            .gain(8.0)
            .unwrap()
            .resample(rate)
            .unwrap()
            .limit_true_peak(TruePeakLimiterOptions::default())
            .unwrap();
        for format in [AudioFormat::Wav, AudioFormat::Flac] {
            let encoded = encode_with_options(
                &audio,
                format,
                &EncodeOptions {
                    dither: Dither::Tpdf { seed: 7 },
                    ..Default::default()
                },
            )
            .unwrap();
            let decoded = decode(&encoded).unwrap();
            let before = bits(&decoded);
            let measurement = decoded.measure_deterministic().unwrap();
            assert!(measurement.true_peak_db <= -1.29);
            // A deterministic comparison has no implicit rounding/tolerance:
            // thresholds on adjacent f64 values must remain distinct.
            let ceiling = measurement.true_peak_db;
            let lower = f64::from_bits(ceiling.to_bits() + 1);
            let upper = f64::from_bits(ceiling.to_bits() - 1);
            let accepts = |limit: f64| measurement.true_peak_db <= limit;
            assert!(!accepts(lower));
            assert!(accepts(ceiling));
            assert!(accepts(upper));
            for x in values(&measurement) {
                fingerprint.extend_from_slice(&x.to_bits().to_le_bytes());
            }
            for x in before {
                fingerprint.extend_from_slice(&x.to_le_bytes());
            }
            assert_eq!(bits(&decoded), bits(&decode(&encoded).unwrap()));
        }
    }
    // Adjacent positive f32 amplitudes straddling the inclusive absolute gate.
    let mut low = 0u32;
    let mut high = 0.25f32.to_bits();
    let periodic = |bits| {
        Audio::new(
            vec![
                (0..2000)
                    .map(|i| {
                        if i % 4 < 2 {
                            f32::from_bits(bits)
                        } else {
                            -f32::from_bits(bits)
                        }
                    })
                    .collect(),
            ],
            4000,
        )
        .unwrap()
    };
    while high - low > 1 {
        let mid = low + (high - low) / 2;
        if periodic(mid)
            .measure_deterministic()
            .unwrap()
            .loudness
            .integrated
            .is_finite()
        {
            high = mid;
        } else {
            low = mid;
        }
    }
    assert_eq!(
        periodic(low)
            .measure_deterministic()
            .unwrap()
            .loudness
            .integrated,
        f64::NEG_INFINITY
    );
    assert!(
        periodic(high)
            .measure_deterministic()
            .unwrap()
            .loudness
            .integrated
            >= -70.0
    );
    for b in [low, high] {
        fingerprint.extend_from_slice(&b.to_le_bytes());
        for x in values(&periodic(b).measure_deterministic().unwrap()) {
            fingerprint.extend_from_slice(&x.to_bits().to_le_bytes());
        }
    }
    assert_eq!(
        hash(fingerprint),
        3059188768134729667,
        "delivered measurement/PCM hash"
    );
}
