use audiobox::*;

fn signal(frames: usize, rate: u32, channels: usize) -> Audio {
    let mut seed = 0x2545_f491u32;
    Audio::new(
        (0..channels)
            .map(|c| {
                (0..frames)
                    .map(|i| {
                        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                        noise / 8.0
                            + if (i / (29 + c * 7)) % 2 == 0 {
                                0.2
                            } else {
                                -0.2
                            }
                    })
                    .collect()
            })
            .collect(),
        rate,
    )
    .unwrap()
}

fn options(unit: NormalizeUnit) -> NormalizeOptions {
    NormalizeOptions {
        to: -18.0,
        unit,
        peak_ceiling_db: 6.0,
    }
}

fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

fn pcm_hash(audio: &Audio) -> u64 {
    hash(&pcm::encode_pcm(audio, SampleFormat::F32, Endian::Little).unwrap())
}

fn report_bits(report: &NormalizeReport) -> [u64; 3] {
    [
        report.measured.to_bits(),
        report.gain_db.to_bits(),
        report.limited_by_peak as u64,
    ]
}

#[test]
fn render_measurements_agree_with_the_report_only_reference() {
    // Full blocks at common rates, and short/high-rate/DC/low-level controls.
    for (rate, channels, frames) in [
        (4000, 1, 2001),
        (8000, 2, 4001),
        (32000, 4, 16001),
        (44100, 5, 22051),
        (48000, 6, 24001),
        (96000, 8, 1234),
        (192000, 1, 101),
        (768000, 2, 101),
    ] {
        let source = signal(frames, rate, channels);
        for audio in [
            source.clone(),
            source.gain(0.0001).unwrap(),
            Audio::new(vec![vec![0.01; frames]; channels], rate).unwrap(),
        ] {
            for unit in [NormalizeUnit::TruePeak, NormalizeUnit::Lufs] {
                let reference = match unit {
                    NormalizeUnit::TruePeak => audio.true_peak_db(),
                    NormalizeUnit::Lufs => audio.loudness().integrated,
                    _ => unreachable!(),
                };
                let (normalized, report) = audio.normalize_with_report(options(unit)).unwrap();
                if reference == f64::NEG_INFINITY {
                    assert_eq!(report.measured, reference);
                    assert_eq!(normalized.all_channels(), audio.all_channels());
                    continue;
                }
                assert!(
                    (report.measured - reference).abs() <= 0.0001,
                    "{unit:?} {rate} Hz: {} vs {reference}",
                    report.measured
                );
                let reference_gain = if matches!(unit, NormalizeUnit::Lufs) {
                    (-18.0 - reference).min(6.0 - audio.true_peak_db())
                } else {
                    -18.0 - reference
                };
                assert!((report.gain_db - reference_gain).abs() <= 0.0001);
                let gain = 10f64.powf(reference_gain / 20.0);
                let max_error = audio
                    .all_channels()
                    .iter()
                    .flatten()
                    .zip(normalized.all_channels().iter().flatten())
                    .map(|(&x, &y)| (y as f64 - x as f64 * gain).abs())
                    .fold(0.0, f64::max);
                assert!(
                    max_error < 0.00001,
                    "{unit:?} {rate} Hz: PCM error {max_error}"
                );
            }
        }
    }
}

#[test]
fn silence_empty_short_and_gated_input_have_explicit_results() {
    for frames in [0, 1, 31, 3199, 3200, 4000] {
        let silence = Audio::new(vec![vec![0.0; frames]; 2], 8000).unwrap();
        for unit in [
            NormalizeUnit::Peak,
            NormalizeUnit::TruePeak,
            NormalizeUnit::Lufs,
        ] {
            let (out, report) = silence.normalize_with_report(options(unit)).unwrap();
            assert_eq!(out.all_channels(), silence.all_channels());
            assert_eq!(report.measured, f64::NEG_INFINITY);
            assert_eq!(report.gain_db, 0.0);
            assert!(!report.limited_by_peak);
        }
    }
    let quiet = signal(4000, 8000, 1).gain(1e-10).unwrap();
    let (out, report) = quiet
        .normalize_with_report(options(NormalizeUnit::Lufs))
        .unwrap();
    assert_eq!(report.measured, f64::NEG_INFINITY);
    assert_eq!(out.all_channels(), quiet.all_channels());
    // The existing sub-400 ms fallback measures the entire available signal,
    // without gating. Very quiet short material is therefore measurable.
    let short = quiet
        .cut(TimePosition::Frames(0), TimePosition::Frames(100))
        .unwrap();
    let (_, report) = short
        .normalize_with_report(options(NormalizeUnit::Lufs))
        .unwrap();
    assert!(report.measured.is_finite());
    assert!(report.gain_db > 100.0);
    let lfe = Audio::new(
        vec![
            vec![0.0; 4000],
            vec![0.0; 4000],
            vec![0.0; 4000],
            vec![0.1; 4000],
            vec![0.0; 4000],
            vec![0.0; 4000],
        ],
        8000,
    )
    .unwrap();
    let (out, report) = lfe
        .normalize_with_report(options(NormalizeUnit::Lufs))
        .unwrap();
    assert_eq!(report.measured, f64::NEG_INFINITY);
    assert_eq!(out.all_channels(), lfe.all_channels());
}

#[test]
fn absolute_gate_and_peak_ceiling_transitions_are_reproducible() {
    let periodic = Audio::new(
        vec![
            (0..8000)
                .map(|i| if i % 4 < 2 { 0.25 } else { -0.25 })
                .collect(),
        ],
        8000,
    )
    .unwrap();
    for target in [-70.001, -69.999] {
        let quiet = periodic
            .normalize(NormalizeOptions {
                to: target,
                ..options(NormalizeUnit::Lufs)
            })
            .unwrap();
        let (out, report) = quiet
            .normalize_with_report(options(NormalizeUnit::Lufs))
            .unwrap();
        assert_eq!(
            report.measured.is_finite(),
            target > -70.0,
            "{target}: {}",
            report.measured
        );
        if target < -70.0 {
            assert_eq!(out.all_channels(), quiet.all_channels());
        }
    }
    let audio = signal(24001, 48000, 2);
    let (_, base) = audio
        .normalize_with_report(options(NormalizeUnit::Lufs))
        .unwrap();
    let (_, peak) = audio
        .normalize_with_report(options(NormalizeUnit::TruePeak))
        .unwrap();
    let ceiling = -1.0;
    let boundary = base.measured + ceiling - peak.measured;
    for delta in [-1e-9, 0.0, 1e-9] {
        let settings = NormalizeOptions {
            to: boundary + delta,
            unit: NormalizeUnit::Lufs,
            peak_ceiling_db: ceiling,
        };
        let (out, report) = audio.normalize_with_report(settings).unwrap();
        assert_eq!(
            report.limited_by_peak,
            settings.to - base.measured > ceiling - peak.measured
        );
        assert!(
            (report.gain_db - (settings.to - base.measured).min(ceiling - peak.measured)).abs()
                < 1e-12
        );
        assert!(out.true_peak_db() <= ceiling + 0.00001);
        let (repeat, again) = audio.normalize_with_report(settings).unwrap();
        assert_eq!(report_bits(&report), report_bits(&again));
        assert_eq!(pcm_hash(&out), pcm_hash(&repeat));
    }
}

#[test]
fn global_gain_and_final_quantization_are_independent_of_output_chunks() {
    let audio = signal(24001, 48000, 2);
    for unit in [NormalizeUnit::TruePeak, NormalizeUnit::Lufs] {
        let (whole, report) = audio.normalize_with_report(options(unit)).unwrap();
        let dither = Dither::Tpdf { seed: 7 };
        let expected =
            pcm::encode_pcm_with_dither(&whole, SampleFormat::S16, Endian::Little, dither).unwrap();
        for size in [1, 7, 256, 4096, 24001] {
            let mut encoder =
                stream::PcmEncoder::with_dither(SampleFormat::S16, Endian::Little, dither).unwrap();
            let mut bytes = Vec::new();
            for start in (0..audio.frames()).step_by(size) {
                let end = (start + size).min(audio.frames());
                let part = audio
                    .cut(
                        TimePosition::Frames(start as i64),
                        TimePosition::Frames(end as i64),
                    )
                    .unwrap()
                    .gain_db(report.gain_db)
                    .unwrap();
                bytes.extend(encoder.push(&part).unwrap());
            }
            assert_eq!(bytes, expected, "{unit:?} {size} frames");
        }
    }
}

#[test]
fn threshold_reports_and_pcm_are_pinned_across_hosts() {
    let mut fingerprint = Vec::new();
    let mut record = |audio: &Audio, settings: NormalizeOptions| {
        let (out, report) = audio.normalize_with_report(settings).unwrap();
        for value in report_bits(&report).into_iter().chain([pcm_hash(&out)]) {
            fingerprint.extend_from_slice(&value.to_le_bytes());
        }
    };
    let periodic = Audio::new(
        vec![
            (0..8000)
                .map(|i| if i % 4 < 2 { 0.25 } else { -0.25 })
                .collect(),
        ],
        8000,
    )
    .unwrap();
    for to in [-70.001, -69.999] {
        let quiet = periodic
            .normalize(NormalizeOptions {
                to,
                ..options(NormalizeUnit::Lufs)
            })
            .unwrap();
        record(&quiet, options(NormalizeUnit::Lufs));
    }
    let audio = signal(24001, 48000, 2);
    let (_, loudness) = audio
        .normalize_with_report(options(NormalizeUnit::Lufs))
        .unwrap();
    let (_, peak) = audio
        .normalize_with_report(options(NormalizeUnit::TruePeak))
        .unwrap();
    for delta in [-1e-9, 0.0, 1e-9] {
        record(
            &audio,
            NormalizeOptions {
                to: loudness.measured - 1.0 - peak.measured + delta,
                unit: NormalizeUnit::Lufs,
                peak_ceiling_db: -1.0,
            },
        );
    }
    for quiet in [0.2294157f32, 0.2294158f32] {
        let relative = Audio::new(
            vec![
                (0..19200)
                    .map(|i| {
                        let level = if i < 9600 { 0.25 } else { 0.25 * quiet };
                        if i % 4 < 2 { level } else { -level }
                    })
                    .collect(),
            ],
            8000,
        )
        .unwrap();
        record(&relative, options(NormalizeUnit::Lufs));
    }
    assert_eq!(
        hash(&fingerprint),
        0xde89031ef3572a00,
        "boundary hash: {:x}",
        hash(&fingerprint)
    );
}

#[test]
fn invalid_levels_rates_and_filter_overflow_are_stable_errors() {
    let audio = signal(10, 48000, 1);
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for unit in [
            NormalizeUnit::Peak,
            NormalizeUnit::TruePeak,
            NormalizeUnit::Lufs,
        ] {
            assert_eq!(
                audio
                    .normalize(NormalizeOptions {
                        to: value,
                        ..options(unit)
                    })
                    .unwrap_err()
                    .code,
                ErrorCode::InvalidArgument
            );
            assert_eq!(
                audio
                    .normalize(NormalizeOptions {
                        peak_ceiling_db: value,
                        ..options(unit)
                    })
                    .unwrap_err()
                    .code,
                ErrorCode::InvalidArgument
            );
        }
    }
    for rate in [1000, 3363, 3999] {
        let audio = signal(32, rate, 1);
        assert_eq!(
            audio
                .normalize(options(NormalizeUnit::Lufs))
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
        audio.normalize(options(NormalizeUnit::TruePeak)).unwrap();
        audio.normalize(options(NormalizeUnit::Peak)).unwrap();
    }
    let extreme = Audio::new(vec![vec![f32::MAX; 100]], 48000).unwrap();
    assert_eq!(
        extreme
            .normalize(options(NormalizeUnit::Lufs))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    for unit in [NormalizeUnit::Peak, NormalizeUnit::TruePeak] {
        let out = extreme.normalize(options(unit)).unwrap();
        assert!(out.all_channels().iter().flatten().all(|s| s.is_finite()));
    }
}

#[test]
fn mastering_pcm_reports_and_exports_are_pinned_separately() {
    let mut normalized_hashes = Vec::new();
    let mut pcm_hashes = Vec::new();
    let mut reports = Vec::new();
    let mut encoded = Vec::new();
    let mut decoded = Vec::new();
    for rate in [32000, 44100, 48000] {
        let source = signal(rate as usize / 2 + 1, rate, 2);
        for unit in [NormalizeUnit::TruePeak, NormalizeUnit::Lufs] {
            let (master, report) = source
                .normalize_with_report(NormalizeOptions {
                    peak_ceiling_db: -1.0,
                    ..options(unit)
                })
                .unwrap();
            normalized_hashes.push(pcm_hash(&master));
            let boosted = master.gain_db(24.0).unwrap();
            assert!(boosted.peak() > 1.0);
            let master = boosted.limit(LimiterOptions::default()).unwrap();
            assert!(master.peak() <= analyze::from_db(-1.0) + 0.000001);
            assert_eq!(master.frames(), source.frames());
            pcm_hashes.push(pcm_hash(&master));
            reports.push(report_bits(&report));
            for format in [AudioFormat::Wav, AudioFormat::Flac, AudioFormat::Mp3] {
                let options = EncodeOptions {
                    dither: if format == AudioFormat::Mp3 {
                        Dither::None
                    } else {
                        Dither::Tpdf { seed: 7 }
                    },
                    tags: Tags {
                        title: Some("Deterministic master".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let bytes = encode_with_options(&master, format, &options).unwrap();
                encoded.push(hash(&bytes));
                decoded.push(pcm_hash(&decode(&bytes).unwrap()));
            }
        }
    }
    assert_eq!(
        normalized_hashes,
        [
            0x7dc7924f6b4602ca,
            0xf08f8f1d8b9655e3,
            0x80f83e48a6562604,
            0xda0b752ba29103cf,
            0xa571df8e3f8533eb,
            0xb724b21297059f83
        ],
        "normalized PCM: {normalized_hashes:x?}"
    );
    assert_eq!(
        pcm_hashes,
        [
            0x3458d8243d398a5b,
            0xfe79c3c847614d04,
            0x6fc15973b36f38eb,
            0x3aae2605069b06f0,
            0x99483fdfc675516,
            0x26343c17b98f7ea6
        ]
    );
    assert_eq!(
        reports,
        [
            [0xc02278235548798f, 0xc02187dcaab78671, 0x0],
            [0xc02525d352493a75, 0xc01db4595b6d8b16, 0x0],
            [0xc0228bae9c6d4a53, 0xc02174516392b5ad, 0x0],
            [0xc024a75d897e4081, 0xc01eb144ed037efe, 0x0],
            [0xc02249868d1a10fc, 0xc021b67972e5ef04, 0x0],
            [0xc02480f123cbee34, 0xc01efe1db8682398, 0x0]
        ]
    );
    assert_eq!(
        encoded,
        [
            0x9bf540d1ab6c1bd0,
            0x59184862371c8deb,
            0xc2cdbd9d1c3b2932,
            0x9b38293225b9f409,
            0x6800dd61c0a1d43c,
            0xc2cdbd9d1c3b2932,
            0xf2dae6df0e5acaf5,
            0xaf1f396e3de04795,
            0x3249751d31f1f604,
            0xacc863b724695c87,
            0x727db9a4a757c72,
            0x3249751d31f1f604,
            0x61717716aa659ce7,
            0xee841e51aeb70584,
            0x16975e39f74bf7a2,
            0xcf7b364bb3dc3f07,
            0x31bd32ec643d8a07,
            0x16975e39f74bf7a2
        ]
    );
    assert_eq!(
        decoded,
        [
            0x1261a59621191155,
            0x1261a59621191155,
            0xf25bdec12807cf91,
            0x60b77d12b6ac9ae3,
            0x60b77d12b6ac9ae3,
            0xf25bdec12807cf91,
            0xd90f9afe470baef8,
            0xd90f9afe470baef8,
            0xf4150841ceaae53a,
            0x314000db3cb9cb86,
            0x314000db3cb9cb86,
            0xf4150841ceaae53a,
            0x3e3a3e242104f2d8,
            0x3e3a3e242104f2d8,
            0x6aafb51db5db1049,
            0x6241ffab2f09aea0,
            0x6241ffab2f09aea0,
            0x6aafb51db5db1049
        ]
    );
}
