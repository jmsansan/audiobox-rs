use audiobox::*;
fn source(frames: usize, rate: u32, channels: usize) -> Audio {
    Audio::new(
        (0..channels)
            .map(|c| {
                (0..frames)
                    .map(|i| {
                        (0.65
                            * (std::f64::consts::TAU * (c + 1) as f64 * 440.0 * i as f64
                                / rate as f64)
                                .sin()) as f32
                    })
                    .collect()
            })
            .collect(),
        rate,
    )
    .unwrap()
}
fn diff(a: &Audio, b: &Audio) -> f32 {
    assert_eq!(a.frames(), b.frames());
    assert_eq!(a.channels(), b.channels());
    a.all_channels()
        .iter()
        .flatten()
        .zip(b.all_channels().iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max)
}
#[test]
fn lossless_container_round_trips() {
    let a = source(9001, 44100, 2);
    for format in [
        AudioFormat::Wav,
        AudioFormat::Aiff,
        AudioFormat::Au,
        AudioFormat::Caf,
        AudioFormat::Flac,
    ] {
        for sample_format in [SampleFormat::S16, SampleFormat::S24, SampleFormat::S32] {
            let bytes = encode_with_options(
                &a,
                format,
                &EncodeOptions {
                    sample_format,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(sniff(&bytes), Some(format));
            let b = decode(&bytes).unwrap();
            assert_eq!(b.sample_rate(), 44100);
            assert_eq!(b.info().format, Some(format));
            assert!(
                diff(&a, &b)
                    <= if sample_format == SampleFormat::S16 {
                        0.000016
                    } else {
                        0.0000002
                    },
                "{format:?} {sample_format:?}"
            );
        }
    }
}
#[test]
fn float_and_g711_formats() {
    let a = source(1000, 8000, 1);
    for format in [
        AudioFormat::Wav,
        AudioFormat::Aiff,
        AudioFormat::Au,
        AudioFormat::Caf,
    ] {
        for sample_format in [
            SampleFormat::F32,
            SampleFormat::F64,
            SampleFormat::Alaw,
            SampleFormat::Ulaw,
        ] {
            let bytes = encode_with_options(
                &a,
                format,
                &EncodeOptions {
                    sample_format,
                    ..Default::default()
                },
            )
            .unwrap();
            let b = decode(&bytes).unwrap();
            assert!(
                diff(&a, &b)
                    <= if matches!(sample_format, SampleFormat::F32 | SampleFormat::F64) {
                        0.0
                    } else {
                        0.02
                    },
                "{format:?} {sample_format:?}"
            );
        }
    }
    for code in 0..=255 {
        assert_eq!(pcm::encode_alaw(pcm::decode_alaw(code)), code);
    }
}
#[test]
fn flac_compression_and_checksums() {
    let a = source(16384, 48000, 2);
    let bytes = encode(&a, AudioFormat::Flac).unwrap();
    let raw = encode(&a, AudioFormat::Wav).unwrap();
    assert!(
        bytes.len() < raw.len() * 3 / 4,
        "{} vs {}",
        bytes.len(),
        raw.len()
    );
    let mut broken = bytes.clone();
    let n = broken.len();
    broken[n - 3] ^= 1;
    assert_eq!(decode(&broken).unwrap_err().code, ErrorCode::DecodeError);
    let mut broken = bytes;
    broken[26] ^= 1;
    assert_eq!(decode(&broken).unwrap_err().code, ErrorCode::DecodeError);
}
#[test]
fn mp3_gapless_quality_and_all_encoding_rates() {
    for rate in [32000, 44100, 48000] {
        for channels in [1, 2] {
            let a = source(12001, rate, channels);
            let bytes = encode(&a, AudioFormat::Mp3).unwrap();
            let b = decode(&bytes).unwrap();
            assert_eq!(b.frames(), a.frames());
            assert_eq!(b.channels(), channels);
            let info = mp3::probe_mp3(&bytes).unwrap();
            assert!((info.duration - a.duration()).abs() < 1e-9);
            let start = 1000;
            let end = 10000;
            let mut error = 0.0;
            let mut energy = 0.0;
            for c in 0..channels {
                for i in start..end {
                    let x = a.channel_data(c).unwrap()[i] as f64;
                    let y = b.channel_data(c).unwrap()[i] as f64;
                    energy += x * x;
                    error += (x - y).powi(2);
                }
            }
            let snr = 10.0 * (energy / error).log10();
            assert!(snr > 25.0, "rate {rate}, channels {channels}, SNR {snr}");
        }
    }
    for n in [0, 1, 100, 528, 1152, 2304] {
        let a = source(n, 44100, 1);
        let bytes = encode(&a, AudioFormat::Mp3).unwrap();
        assert_eq!(decode(&bytes).unwrap().frames(), n);
    }
}
#[test]
fn empty_files_limits_and_metadata() {
    let empty = source(0, 48000, 1);
    for format in [
        AudioFormat::Wav,
        AudioFormat::Aiff,
        AudioFormat::Au,
        AudioFormat::Caf,
        AudioFormat::Flac,
    ] {
        assert_eq!(
            decode(&encode(&empty, format).unwrap()).unwrap().frames(),
            0
        );
    }
    let a = source(5000, 48000, 1);
    let bytes = encode(&a, AudioFormat::Flac).unwrap();
    let options = DecodeOptions {
        limits: Limits {
            max_decoded_bytes: 100,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        decode_with_options(&bytes, &options).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    let mut tags = AudioMetadata::new();
    tags.insert("title".into(), "Prueba ñ".into());
    for format in [AudioFormat::Wav, AudioFormat::Aiff, AudioFormat::Flac] {
        let bytes = encode_with_options(
            &a,
            format,
            &EncodeOptions {
                metadata: tags.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        let b = decode(&bytes).unwrap();
        assert!(b.info().metadata.values().any(|s| s == "Prueba ñ"));
    }
}
#[test]
fn hostile_inputs_never_panic() {
    let formats = [
        AudioFormat::Wav,
        AudioFormat::Aiff,
        AudioFormat::Au,
        AudioFormat::Caf,
        AudioFormat::Flac,
        AudioFormat::Mp3,
    ];
    let a = source(1000, 44100, 1);
    let limits = Limits {
        max_decoded_bytes: 1024 * 1024,
        max_frames: 10000,
        max_channels: 8,
        ..Default::default()
    };
    for f in formats {
        let valid = encode(&a, f).unwrap();
        for n in (0..valid.len()).step_by((valid.len() / 70).max(1)) {
            assert!(
                std::panic::catch_unwind(|| decode_with_options(
                    &valid[..n],
                    &DecodeOptions {
                        format: Some(f),
                        limits: limits.clone()
                    }
                ))
                .is_ok(),
                "{f:?} truncation {n}"
            );
        }
        for i in 0..valid.len().min(200) {
            let mut broken = valid.clone();
            broken[i] ^= 0xff;
            assert!(
                std::panic::catch_unwind(|| decode_with_options(
                    &broken,
                    &DecodeOptions {
                        format: Some(f),
                        limits: limits.clone()
                    }
                ))
                .is_ok(),
                "{f:?} mutation {i}"
            );
        }
    }
    let mut state = 0x12345678u32;
    for n in 0..512 {
        let mut bytes = vec![0; n];
        for b in &mut bytes {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *b = state as u8;
        }
        for f in formats {
            assert!(
                std::panic::catch_unwind(|| decode_with_options(
                    &bytes,
                    &DecodeOptions {
                        format: Some(f),
                        limits: limits.clone()
                    }
                ))
                .is_ok()
            );
        }
    }
}
#[test]
fn streaming_handles_every_byte_boundary() {
    let a = source(401, 48000, 2);
    let raw = pcm::encode_pcm(&a, SampleFormat::S24, Endian::Big).unwrap();
    let mut d =
        stream::PcmDecoder::new(SampleFormat::S24, Endian::Big, 2, 48000, Limits::default())
            .unwrap();
    let mut out = vec![Vec::new(); 2];
    for byte in &raw {
        let part = d.push(&[*byte]).unwrap();
        for (c, data) in out.iter_mut().enumerate() {
            data.extend_from_slice(part.channel_data(c).unwrap());
        }
    }
    d.finish().unwrap();
    assert!(diff(&a, &Audio::new(out, 48000).unwrap()) < 0.0000002);
    let mut d = stream::PcmDecoder::new(
        SampleFormat::S16,
        Endian::Little,
        2,
        48000,
        Limits::default(),
    )
    .unwrap();
    d.push(&[0]).unwrap();
    assert!(d.finish().is_err());
}

#[test]
fn streaming_resampling_is_independent_of_chunk_boundaries() {
    let a = source(2049, 48000, 2);
    for rate in [16000, 44100, 96000] {
        let expected = a.resample(rate).unwrap();
        for size in [1, 137, 2048] {
            let mut resampler = stream::Resampler::new(48000, rate, 2, Limits::default()).unwrap();
            let mut data = vec![Vec::new(); 2];
            for start in (0..a.frames()).step_by(size) {
                let part = a
                    .cut(
                        TimePosition::Frames(start as i64),
                        TimePosition::Frames((start + size).min(a.frames()) as i64),
                    )
                    .unwrap();
                let output = resampler.push(&part).unwrap();
                for (c, dst) in data.iter_mut().enumerate() {
                    dst.extend_from_slice(output.channel_data(c).unwrap());
                }
            }
            let output = resampler.finish().unwrap();
            for (c, dst) in data.iter_mut().enumerate() {
                dst.extend_from_slice(output.channel_data(c).unwrap());
            }
            assert_eq!(
                diff(&expected, &Audio::new(data, rate).unwrap()),
                0.0,
                "rate {rate}, chunk {size}"
            );
        }
    }
}

#[test]
fn decodes_files_from_the_original_typescript_library() {
    let pcm = decode(include_bytes!("fixtures/typescript.wav")).unwrap();
    for bytes in [
        include_bytes!("fixtures/typescript.aiff").as_slice(),
        include_bytes!("fixtures/typescript.caf").as_slice(),
        include_bytes!("fixtures/typescript.au").as_slice(),
        include_bytes!("fixtures/typescript.flac").as_slice(),
    ] {
        assert_eq!(diff(&pcm, &decode(bytes).unwrap()), 0.0);
    }
    let mp3 = decode(include_bytes!("fixtures/typescript.mp3")).unwrap();
    assert_eq!(mp3.frames(), pcm.frames());
    assert_eq!(mp3.channels(), 2);
    let mut energy = 0.0;
    let mut noise = 0.0;
    for c in 0..2 {
        for i in 700..3300 {
            let a = pcm.channel_data(c).unwrap()[i] as f64;
            let b = mp3.channel_data(c).unwrap()[i] as f64;
            energy += a * a;
            noise += (a - b).powi(2);
        }
    }
    assert!(10.0 * (energy / noise).log10() > 30.0);
}
