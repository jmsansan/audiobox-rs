use audiobox::*;

const DITHER: Dither = Dither::Tpdf {
    seed: 0x1234_5678_9abc_def0,
};
const INTEGER: [SampleFormat; 5] = [
    SampleFormat::U8,
    SampleFormat::S8,
    SampleFormat::S16,
    SampleFormat::S24,
    SampleFormat::S32,
];

// Rational arithmetic only: fixture samples themselves are identical across hosts.
fn signal(frames: usize, channels: usize) -> Audio {
    Audio::new(
        (0..channels)
            .map(|c| {
                (0..frames)
                    .map(|i| ((i * 7919 + c * 104729) % 65536) as f32 / 32768.0 - 1.0)
                    .collect()
            })
            .collect(),
        48_000,
    )
    .unwrap()
}

fn raw(audio: &Audio, format: SampleFormat, endian: Endian, dither: Dither) -> Vec<u8> {
    pcm::encode_pcm_with_dither(audio, format, endian, dither).unwrap()
}

fn integers(bytes: &[u8], format: SampleFormat, endian: Endian) -> Vec<i64> {
    bytes
        .chunks_exact(format.bytes())
        .map(|b| {
            let mut value = 0i64;
            for i in 0..b.len() {
                value |= (b[if endian == Endian::Little {
                    i
                } else {
                    b.len() - i - 1
                }] as i64)
                    << (8 * i);
            }
            if format == SampleFormat::U8 {
                value - 128
            } else {
                let shift = 64 - format.bits();
                value << shift >> shift
            }
        })
        .collect()
}

fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

#[test]
fn disabled_dither_keeps_the_legacy_integer_quantizer_and_float_bits() {
    let audio = Audio::new(
        vec![vec![
            -f32::MAX,
            -2.0,
            -1.0,
            -0.5,
            -1.5 / 32768.0,
            -0.5 / 32768.0,
            -0.0,
            0.0,
            0.5 / 32768.0,
            1.5 / 32768.0,
            0.5,
            1.0,
            2.0,
            f32::MAX,
        ]],
        48_000,
    )
    .unwrap();
    for format in INTEGER {
        let scale = (1u64 << (format.bits() - 1)) as f64;
        let expected: Vec<i64> = audio
            .channel_data(0)
            .unwrap()
            .iter()
            .map(|&s| (s as f64 * scale).round().clamp(-scale, scale - 1.0) as i64)
            .collect();
        for endian in [Endian::Little, Endian::Big] {
            let bytes = raw(&audio, format, endian, Dither::None);
            assert_eq!(integers(&bytes, format, endian), expected);
            assert_eq!(bytes, pcm::encode_pcm(&audio, format, endian).unwrap());
        }
    }
    for format in [
        SampleFormat::F32,
        SampleFormat::F64,
        SampleFormat::Alaw,
        SampleFormat::Ulaw,
    ] {
        for endian in [Endian::Little, Endian::Big] {
            let bytes = raw(&audio, format, endian, Dither::None);
            let expected: Vec<u8> = audio
                .channel_data(0)
                .unwrap()
                .iter()
                .flat_map(|&s| {
                    let value = match format {
                        SampleFormat::F32 => s.to_bits() as u64,
                        SampleFormat::F64 => (s as f64).to_bits(),
                        SampleFormat::Alaw => pcm::encode_alaw(
                            (s as f64 * 32768.0).round().clamp(-32768.0, 32767.0) as i16,
                        ) as u64,
                        SampleFormat::Ulaw => pcm::encode_ulaw(
                            (s as f64 * 32768.0).round().clamp(-32768.0, 32767.0) as i16,
                        ) as u64,
                        _ => unreachable!(),
                    };
                    (0..format.bytes()).map(move |i| {
                        let shift = if endian == Endian::Little {
                            i
                        } else {
                            format.bytes() - i - 1
                        };
                        (value >> (8 * shift)) as u8
                    })
                })
                .collect();
            assert_eq!(bytes, expected);
        }
    }
}

#[test]
fn default_container_bytes_match_the_released_041_encoder() {
    // Recorded from an unmodified checkout at 7477cb1, independently compiled.
    let audio = signal(8201, 2);
    let mut hashes = Vec::new();
    for format in [
        AudioFormat::Raw,
        AudioFormat::Wav,
        AudioFormat::Aiff,
        AudioFormat::Au,
        AudioFormat::Caf,
        AudioFormat::Flac,
    ] {
        for sample_format in [
            SampleFormat::U8,
            SampleFormat::S8,
            SampleFormat::S16,
            SampleFormat::S24,
            SampleFormat::S32,
            SampleFormat::F32,
            SampleFormat::F64,
            SampleFormat::Alaw,
            SampleFormat::Ulaw,
        ] {
            for endian in [Endian::Little, Endian::Big] {
                if let Ok(bytes) = encode_with_options(
                    &audio,
                    format,
                    &EncodeOptions {
                        sample_format,
                        endian,
                        ..Default::default()
                    },
                ) {
                    hashes.extend_from_slice(&hash(&bytes).to_le_bytes());
                }
            }
        }
    }
    assert_eq!(hashes.len(), 98 * 8);
    assert_eq!(hash(&hashes), 0x1f09f85e63d4d2c6);
}

#[test]
fn raw_dispatch_and_container_storage_options_keep_the_same_quantized_samples() {
    let audio = signal(500, 6);
    for endian in [Endian::Little, Endian::Big] {
        let options = EncodeOptions {
            endian,
            dither: DITHER,
            ..Default::default()
        };
        let raw = raw(&audio, SampleFormat::S16, endian, DITHER);
        assert_eq!(
            encode_with_options(&audio, AudioFormat::Raw, &options).unwrap(),
            raw
        );
        let expected = pcm::decode_pcm(
            &raw,
            SampleFormat::S16,
            endian,
            6,
            48_000,
            &Limits::default(),
        )
        .unwrap();
        let caf =
            decode(&encode_with_options(&audio, AudioFormat::Caf, &options).unwrap()).unwrap();
        assert_eq!(caf.all_channels(), expected.all_channels());
        let wav = decode(
            &encode_with_options(
                &audio,
                AudioFormat::Wav,
                &EncodeOptions {
                    wav_channel_mask: Some(0x3f),
                    ..options
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(wav.all_channels(), expected.all_channels());
    }
}

#[test]
fn silence_has_the_expected_tpdf_distribution_and_independent_channels() {
    // Rounded TPDF on exact silence: P(-1, 0, 1) = (1/8, 3/4, 1/8), variance 1/4 LSB².
    let frames = 131_072;
    let audio = Audio::new(vec![vec![0.0; frames]; 2], 48_000).unwrap();
    for format in INTEGER {
        let values = integers(
            &raw(&audio, format, Endian::Little, DITHER),
            format,
            Endian::Little,
        );
        let mut channels = [Vec::new(), Vec::new()];
        for pair in values.chunks_exact(2) {
            channels[0].push(pair[0] as f64);
            channels[1].push(pair[1] as f64);
        }
        for channel in &channels {
            let mean = channel.iter().sum::<f64>() / frames as f64;
            let power = channel.iter().map(|x| x * x).sum::<f64>() / frames as f64;
            assert!(channel.iter().all(|x| x.abs() <= 1.0));
            assert!(mean.abs() < 0.01, "{format:?}: mean {mean} LSB");
            assert!((power - 0.25).abs() < 0.01, "{format:?}: variance {power}");
            for level in [-1.0, 1.0] {
                let fraction =
                    channel.iter().filter(|&&x| x == level).count() as f64 / frames as f64;
                assert!((fraction - 0.125).abs() < 0.01);
            }
        }
        let cross = channels[0]
            .iter()
            .zip(&channels[1])
            .map(|(l, r)| l * r)
            .sum::<f64>()
            / frames as f64;
        assert!(
            (cross / 0.25).abs() < 0.02,
            "{format:?}: channel correlation"
        );
    }
}

#[test]
fn low_level_tone_error_is_bounded_unbiased_and_decorrelated() {
    // A quarter-LSB tone would quantize to silence without dither.
    let frames = 131_072;
    let tone = Audio::new(
        vec![
            (0..frames)
                .map(|i| {
                    (0.25 / 32768.0 * (std::f64::consts::TAU * 997.0 * i as f64 / 48_000.0).sin())
                        as f32
                })
                .collect(),
        ],
        48_000,
    )
    .unwrap();
    let source = tone.channel_data(0).unwrap();
    let values = integers(
        &raw(&tone, SampleFormat::S16, Endian::Little, DITHER),
        SampleFormat::S16,
        Endian::Little,
    );
    let mut mean = 0.0;
    let mut error_power = 0.0;
    let mut signal_power = 0.0;
    let mut cross = 0.0;
    for (&x, &y) in source.iter().zip(&values) {
        let x = x as f64 * 32768.0;
        let error = y as f64 - x;
        assert!(error.abs() <= 1.5);
        mean += error;
        error_power += error * error;
        signal_power += x * x;
        cross += x * error;
    }
    assert!((mean / frames as f64).abs() < 0.01);
    assert!((error_power / frames as f64 - 0.25).abs() < 0.01);
    assert!((cross / (error_power * signal_power).sqrt()).abs() < 0.02);
}

#[test]
fn saturation_clamps_after_noise_without_wrapping_at_any_bit_depth() {
    let audio = Audio::new(
        vec![vec![-f32::MAX, -2.0, -1.0, 1.0, 2.0, f32::MAX]],
        48_000,
    )
    .unwrap();
    for format in INTEGER {
        let scale = 1i64 << (format.bits() - 1);
        for endian in [Endian::Little, Endian::Big] {
            let values = integers(&raw(&audio, format, endian, DITHER), format, endian);
            assert_eq!(values[0], -scale);
            assert_eq!(values[1], -scale);
            assert!((-scale..=-scale + 1).contains(&values[2]));
            assert_eq!(values[3], scale - 1);
            assert_eq!(values[4], scale - 1);
            assert_eq!(values[5], scale - 1);
        }
    }
}

#[test]
fn containers_apply_dither_once_and_flac_hashes_the_quantized_pcm() {
    // Cross FLAC's 4096-frame block boundary, with correlated stereo input and tags.
    for frames in [0, 1, 17, 4095, 4096, 4097, 8201] {
        for channels in [1, 2] {
            let audio = signal(frames, channels);
            for sample_format in [
                SampleFormat::U8,
                SampleFormat::S16,
                SampleFormat::S24,
                SampleFormat::S32,
            ] {
                let options = EncodeOptions {
                    sample_format,
                    dither: DITHER,
                    ..Default::default()
                };
                let raw = raw(&audio, sample_format, Endian::Little, DITHER);
                let expected = pcm::decode_pcm(
                    &raw,
                    sample_format,
                    Endian::Little,
                    channels,
                    48_000,
                    &Limits::default(),
                )
                .unwrap();
                for format in [
                    AudioFormat::Wav,
                    AudioFormat::Aiff,
                    AudioFormat::Caf,
                    AudioFormat::Au,
                    AudioFormat::Flac,
                ] {
                    let container_options = EncodeOptions {
                        sample_format: if format == AudioFormat::Caf
                            && sample_format == SampleFormat::U8
                        {
                            SampleFormat::S8
                        } else {
                            sample_format
                        },
                        ..options.clone()
                    };
                    let bytes = encode_with_options(&audio, format, &container_options).unwrap();
                    let actual = decode(&bytes).unwrap();
                    assert_eq!(
                        actual.all_channels(),
                        expected.all_channels(),
                        "{format:?} {sample_format:?} {frames} {channels}"
                    );
                    assert_eq!(actual.frames(), frames);
                }
            }
        }
    }
    let audio = signal(8201, 2);
    for compression_level in [0, 1, 8] {
        let options = EncodeOptions {
            compression_level,
            dither: DITHER,
            tags: Tags {
                title: Some("Dither".into()),
                cover: Some(
                    CoverArt::new("image/png", include_bytes!("fixtures/cover.png").to_vec())
                        .unwrap(),
                ),
                ..Default::default()
            },
            ..Default::default()
        };
        let actual =
            decode(&encode_with_options(&audio, AudioFormat::Flac, &options).unwrap()).unwrap();
        let expected = decode(
            &encode_with_options(
                &audio,
                AudioFormat::Wav,
                &EncodeOptions {
                    dither: DITHER,
                    ..Default::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(actual.all_channels(), expected.all_channels());
        assert_eq!(actual.info().tags, options.tags);
    }
}

#[test]
fn full_width_flac_uses_bounded_rice_costs_on_32_bit_hosts_too() {
    let audio = signal(8201, 2);
    for compression_level in [0, 5, 8] {
        let options = EncodeOptions {
            sample_format: SampleFormat::S32,
            compression_level,
            ..Default::default()
        };
        let bytes = encode_with_options(&audio, AudioFormat::Flac, &options).unwrap();
        // The verbatim candidate bounds every block, even when Rice's unary
        // candidate would need more than 2^32 bits.
        assert!(bytes.len() <= audio.frames() * audio.channels() * 4 + 256);
        assert_eq!(decode(&bytes).unwrap().all_channels(), audio.all_channels());
    }
}

#[test]
fn stream_chunks_empty_chunks_and_rejected_shapes_preserve_the_noise_sequence() {
    let audio = signal(8201, 2);
    for format in INTEGER {
        for endian in [Endian::Little, Endian::Big] {
            let expected = raw(&audio, format, endian, DITHER);
            for chunk in [1, 7, 256, 4096, 8201] {
                let mut encoder = stream::PcmEncoder::with_dither(format, endian, DITHER).unwrap();
                let mut actual = Vec::new();
                let empty = signal(0, 2);
                assert!(encoder.push(&empty).unwrap().is_empty());
                for start in (0..audio.frames()).step_by(chunk) {
                    assert!(encoder.push(&signal(0, 1)).is_err());
                    let bad_rate = Audio::new(vec![vec![0.0]; 2], 44_100).unwrap();
                    assert!(encoder.push(&bad_rate).is_err());
                    let end = (start + chunk).min(audio.frames());
                    let part = audio
                        .cut(
                            TimePosition::Frames(start as i64),
                            TimePosition::Frames(end as i64),
                        )
                        .unwrap();
                    actual.extend(encoder.push(&part).unwrap());
                    assert!(encoder.push(&empty).unwrap().is_empty());
                }
                assert_eq!(actual, expected, "{format:?} {endian:?} chunk={chunk}");
            }
        }
    }
}

#[test]
fn unsupported_dither_combinations_are_explicit_errors() {
    let audio = signal(0, 2);
    for sample_format in [
        SampleFormat::F32,
        SampleFormat::F64,
        SampleFormat::Alaw,
        SampleFormat::Ulaw,
    ] {
        for format in [
            AudioFormat::Wav,
            AudioFormat::Aiff,
            AudioFormat::Caf,
            AudioFormat::Au,
            AudioFormat::Flac,
            AudioFormat::Raw,
            AudioFormat::Mp3,
        ] {
            let options = EncodeOptions {
                sample_format,
                dither: DITHER,
                ..Default::default()
            };
            assert_eq!(
                encode_with_options(&audio, format, &options)
                    .unwrap_err()
                    .code,
                ErrorCode::InvalidArgument
            );
        }
        assert_eq!(
            pcm::encode_pcm_with_dither(&audio, sample_format, Endian::Little, DITHER)
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            stream::PcmEncoder::with_dither(sample_format, Endian::Little, DITHER)
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
    }
    for sample_format in INTEGER {
        assert_eq!(
            encode_with_options(
                &audio,
                AudioFormat::Mp3,
                &EncodeOptions {
                    sample_format,
                    dither: DITHER,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
            ErrorCode::InvalidArgument
        );
    }
    for (format, sample_format) in [
        (AudioFormat::Caf, SampleFormat::U8),
        (AudioFormat::Wav, SampleFormat::S8),
    ] {
        assert!(
            encode_with_options(
                &audio,
                format,
                &EncodeOptions {
                    sample_format,
                    dither: DITHER,
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}

#[test]
fn seeds_and_channel_streams_are_local_to_each_export() {
    let audio = signal(8201, 2);
    for seed in [0, 1, u64::MAX] {
        let dither = Dither::Tpdf { seed };
        let expected = raw(&audio, SampleFormat::S16, Endian::Little, dither);
        let unrelated = raw(
            &signal(300, 4),
            SampleFormat::U8,
            Endian::Big,
            Dither::Tpdf {
                seed: seed.wrapping_add(1),
            },
        );
        assert!(!unrelated.is_empty());
        assert_eq!(
            raw(&audio, SampleFormat::S16, Endian::Little, dither),
            expected
        );
        // Channel zero never depends on the presence of later channels.
        let mono = Audio::new(vec![audio.channel_data(0).unwrap().to_vec()], 48_000).unwrap();
        let mono = raw(&mono, SampleFormat::S16, Endian::Little, dither);
        let left: Vec<u8> = expected
            .chunks_exact(4)
            .flat_map(|b| b[..2].iter().copied())
            .collect();
        assert_eq!(mono, left);
        assert_ne!(
            expected,
            raw(
                &audio,
                SampleFormat::S16,
                Endian::Little,
                Dither::Tpdf {
                    seed: seed.wrapping_add(1)
                }
            )
        );
    }
}

#[test]
fn dithered_pcm_and_container_bytes_are_pinned_across_hosts() {
    let audio = signal(8201, 2);
    let mut hashes = Vec::new();
    for sample_format in INTEGER {
        for endian in [Endian::Little, Endian::Big] {
            hashes.push(hash(&raw(&audio, sample_format, endian, DITHER)));
        }
    }
    for format in [AudioFormat::Wav, AudioFormat::Flac] {
        for sample_format in [
            SampleFormat::U8,
            SampleFormat::S16,
            SampleFormat::S24,
            SampleFormat::S32,
        ] {
            hashes.push(hash(
                &encode_with_options(
                    &audio,
                    format,
                    &EncodeOptions {
                        sample_format,
                        dither: DITHER,
                        ..Default::default()
                    },
                )
                .unwrap(),
            ));
        }
    }
    assert_eq!(
        hashes,
        [
            0xdbcbc53d3ae0d7bf,
            0xdbcbc53d3ae0d7bf,
            0x9b152b7f0798fcbf,
            0x9b152b7f0798fcbf,
            0x0f60aace065aefaf,
            0x50624ac417e7ddab,
            0xb8cbbecb62c0557a,
            0x1dcf3534b333d0ca,
            0x216012dae9d2d480,
            0xc49ed97fb73149da,
            0xcd99387b42322c46,
            0x4040aeaf7eefb9e4,
            0x8e915254bcf92a54,
            0x905e4ffcf805f63a,
            0x77568ca3aa0257ef,
            0x7b81710ae4e3f133,
            0x50d4e9a9508238d9,
            0x50a79203dbe36f26,
        ],
        "dithered bytes changed: {hashes:x?}"
    );
}
