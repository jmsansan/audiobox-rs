use audiobox::stream::{Decoder, DecoderOptions};
use audiobox::*;

/// Every stream the tests decode: fixtures and longer encodes, by name.
fn inputs() -> Vec<(&'static str, Vec<u8>)> {
    let tone = |frames: usize, rate: f32, hz: f32| -> Vec<f32> {
        (0..frames)
            .map(|i| {
                let t = i as f32 / rate;
                0.5 * (std::f32::consts::TAU * hz * t).sin() * (0.6 + 0.4 * (t * 2.0).sin())
            })
            .collect()
    };
    let stereo = Audio::new(
        vec![tone(30_000, 44_100.0, 440.0), tone(30_000, 44_100.0, 660.0)],
        44_100,
    )
    .unwrap();
    let mono = Audio::new(vec![tone(25_001, 48_000.0, 300.0)], 48_000).unwrap();
    let wav = |audio: &Audio, sample_format| {
        encode_with_options(
            audio,
            AudioFormat::Wav,
            &EncodeOptions {
                sample_format,
                ..Default::default()
            },
        )
        .unwrap()
    };
    vec![
        (
            "typescript.wav",
            include_bytes!("fixtures/typescript.wav").to_vec(),
        ),
        (
            "typescript.flac",
            include_bytes!("fixtures/typescript.flac").to_vec(),
        ),
        (
            "typescript.mp3",
            include_bytes!("fixtures/typescript.mp3").to_vec(),
        ),
        (
            "lame-stereo.mp3",
            include_bytes!("fixtures/lame-stereo.mp3").to_vec(),
        ),
        (
            "lame-mpeg2.mp3",
            include_bytes!("fixtures/lame-mpeg2.mp3").to_vec(),
        ),
        (
            "lame-mpeg25.mp3",
            include_bytes!("fixtures/lame-mpeg25.mp3").to_vec(),
        ),
        (
            "ima-stereo.wav",
            include_bytes!("fixtures/ima-stereo.wav").to_vec(),
        ),
        ("s16.wav", wav(&stereo, SampleFormat::S16)),
        ("f32.wav", wav(&mono, SampleFormat::F32)),
        ("u8.wav", wav(&mono, SampleFormat::U8)),
        ("stereo.flac", encode(&stereo, AudioFormat::Flac).unwrap()),
        (
            "mono24.flac",
            encode_with_options(
                &mono,
                AudioFormat::Flac,
                &EncodeOptions {
                    sample_format: SampleFormat::S24,
                    ..Default::default()
                },
            )
            .unwrap(),
        ),
        ("stereo.mp3", encode(&stereo, AudioFormat::Mp3).unwrap()),
        (
            "mono.mp3",
            encode_with_options(
                &mono,
                AudioFormat::Mp3,
                &EncodeOptions {
                    bitrate: 64,
                    ..Default::default()
                },
            )
            .unwrap(),
        ),
    ]
}

fn interleave(audio: &Audio, from: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for i in from..audio.frames() {
        for c in audio.all_channels() {
            out.push(c[i]);
        }
    }
    out
}

/// Reads everything left, `chunk` frames at a time.
fn read_all<B: AsRef<[u8]>>(decoder: &mut Decoder<B>, chunk: usize) -> Vec<f32> {
    let mut out = Vec::new();
    let mut buffer = vec![0.0; chunk * decoder.channels()];
    loop {
        let n = decoder.read(&mut buffer).unwrap();
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&buffer[..n * decoder.channels()]);
    }
}

fn same(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

#[test]
fn chunks_equal_whole_decode() {
    for (name, bytes) in inputs() {
        let whole = decode(&bytes).unwrap();
        let expected = interleave(&whole, 0);
        for chunk in [7, 1152, 10_000] {
            let mut decoder = Decoder::new(bytes.as_slice()).unwrap();
            assert_eq!(decoder.sample_rate(), whole.sample_rate(), "{name}");
            assert_eq!(decoder.channels(), whole.channels(), "{name}");
            let got = read_all(&mut decoder, chunk);
            assert!(
                same(&got, &expected),
                "{name}, chunks of {chunk}: {} vs {} samples",
                got.len(),
                expected.len()
            );
            assert_eq!(decoder.position(), whole.frames() as u64);
            if let Some(frames) = decoder.frames() {
                assert_eq!(frames, whole.frames() as u64, "{name}");
            }
        }
        // next_chunk gives bounded chunks with the same samples.
        let options = DecoderOptions {
            chunk_frames: 777,
            ..Default::default()
        };
        let mut decoder = Decoder::with_options(&bytes, &options).unwrap();
        let mut got = Vec::new();
        while let Some(chunk) = decoder.next_chunk().unwrap() {
            assert!(chunk.frames() <= 777 && chunk.frames() > 0);
            got.extend(interleave(&chunk, 0));
        }
        assert!(same(&got, &expected), "{name} via next_chunk");
    }
}

#[test]
fn seeking_matches_whole_decode() {
    for (name, bytes) in inputs() {
        let whole = decode(&bytes).unwrap();
        let frames = whole.frames();
        let mut decoder = Decoder::new(bytes.clone()).unwrap();
        let mut positions = vec![
            0,
            1,
            575,
            576,
            1151,
            1152,
            1153,
            frames / 3,
            frames / 2,
            frames - 1,
            frames,
            frames + 10,
        ];
        // Many positions across the stream, visited out of order.
        positions.extend((0..23).map(|i| (i * 7919 * 13) % frames.max(1)));
        for (i, &p) in positions.iter().enumerate() {
            let p = p.min(frames + 10);
            decoder.seek(p as u64).unwrap();
            assert_eq!(decoder.position(), p.min(frames) as u64, "{name} seek {p}");
            let mut expected = interleave(&whole, p.min(frames));
            // Read to the end after the fixed positions, and a window after the others.
            let got = if i < 12 {
                read_all(&mut decoder, 1000)
            } else {
                expected.truncate(3000 * whole.channels());
                let mut window = vec![0.0; 3000 * whole.channels()];
                let n = decoder.read(&mut window).unwrap();
                window.truncate(n * whole.channels());
                window
            };
            assert!(
                same(&got, &expected),
                "{name}: seek to {p} of {frames} gave {} samples, expected {}",
                got.len(),
                expected.len()
            );
        }
        // A short read, then a seek forward and backward within and across blocks.
        decoder.rewind().unwrap();
        let mut buffer = vec![0.0; 300 * whole.channels()];
        decoder.read(&mut buffer).unwrap();
        for p in [310, 250, 5000.min(frames), 20] {
            decoder.seek(p as u64).unwrap();
            let n = decoder.read(&mut buffer).unwrap();
            let expected = interleave(&whole, p);
            assert!(
                same(
                    &buffer[..n * whole.channels()],
                    &expected[..n * whole.channels()]
                ),
                "{name} short seek {p}"
            );
        }
        decoder.rewind().unwrap();
        assert!(
            same(&read_all(&mut decoder, 4096), &interleave(&whole, 0)),
            "{name} after rewind"
        );
    }
}

#[test]
fn flac_seek_table_is_used() {
    // Our encoder writes no SEEKTABLE: insert one with a point per 4096-sample frame.
    let tone: Vec<f32> = (0..50_000).map(|i| (i as f32 * 0.01).sin() * 0.4).collect();
    let plain = encode(&Audio::new(vec![tone], 22_050).unwrap(), AudioFormat::Flac).unwrap();
    let mut decoder = Decoder::new(plain.as_slice()).unwrap();
    // Find the frames' offsets by decoding.
    let first_frame = {
        let mut pos = 4;
        loop {
            let last = plain[pos] & 128 != 0;
            let n =
                u32::from_be_bytes([0, plain[pos + 1], plain[pos + 2], plain[pos + 3]]) as usize;
            pos += 4 + n;
            if last {
                break pos;
            }
        }
    };
    let mut offsets = Vec::new();
    for pos in first_frame..plain.len() - 1 {
        if plain[pos] == 0xff && plain[pos + 1] == 0xf8 {
            offsets.push(pos - first_frame);
        }
    }
    let mut table = Vec::new();
    for (i, &offset) in offsets.iter().enumerate().take(13) {
        table.extend_from_slice(&((i * 4096) as u64).to_be_bytes());
        table.extend_from_slice(&(offset as u64).to_be_bytes());
        table.extend_from_slice(&4096u16.to_be_bytes());
    }
    table.extend_from_slice(&u64::MAX.to_be_bytes()); // A placeholder point.
    table.extend_from_slice(&[0; 10]);
    // STREAMINFO stays first; the table becomes the last metadata block.
    let mut with_table = plain[..first_frame].to_vec();
    let mut pos = 4;
    loop {
        let last = with_table[pos] & 128 != 0;
        let n = u32::from_be_bytes([
            0,
            with_table[pos + 1],
            with_table[pos + 2],
            with_table[pos + 3],
        ]) as usize;
        if last {
            with_table[pos] &= 127;
            break;
        }
        pos += 4 + n;
    }
    with_table.push(128 | 3);
    with_table.extend_from_slice(&(table.len() as u32).to_be_bytes()[1..]);
    with_table.extend_from_slice(&table);
    with_table.extend_from_slice(&plain[first_frame..]);
    let whole = decode(&with_table).unwrap();
    assert!(same(
        whole.channel_data(0).unwrap(),
        decode(&plain).unwrap().channel_data(0).unwrap()
    ));
    // With a frame before the last seek point corrupted, only the table skips it.
    let corrupt = |bytes: &[u8], header: usize| {
        let mut bytes = bytes.to_vec();
        bytes[header + offsets[2] + 40] ^= 0x55;
        bytes
    };
    let mut skipping = Decoder::new(corrupt(
        &with_table,
        with_table.len() - plain.len() + first_frame,
    ))
    .unwrap();
    skipping.seek(49_200).unwrap();
    assert!(same(
        &read_all(&mut skipping, 999),
        &interleave(&whole, 49_200)
    ));
    let mut blind = Decoder::new(corrupt(&plain, first_frame)).unwrap();
    assert_eq!(blind.seek(49_200).unwrap_err().code, ErrorCode::DecodeError);

    let mut tabled = Decoder::new(with_table.as_slice()).unwrap();
    for p in [45_000, 9000, 4096, 30_001, 0] {
        tabled.seek(p).unwrap();
        decoder.seek(p).unwrap();
        let expected = interleave(&whole, p as usize);
        assert!(
            same(&read_all(&mut tabled, 999), &expected),
            "seek table, {p}"
        );
        assert!(
            same(&read_all(&mut decoder, 999), &expected),
            "no seek table, {p}"
        );
    }
}

#[test]
fn malformed_input_gives_errors() {
    let unsupported = include_bytes!("fixtures/typescript.aiff");
    assert_eq!(
        Decoder::new(unsupported.as_slice()).unwrap_err().code,
        ErrorCode::UnsupportedFormat
    );
    assert_eq!(
        Decoder::new([0u8; 64].as_slice()).unwrap_err().code,
        ErrorCode::UnsupportedFormat
    );
    let zero = DecoderOptions {
        chunk_frames: 0,
        ..Default::default()
    };
    assert!(
        Decoder::with_options(include_bytes!("fixtures/typescript.wav").as_slice(), &zero).is_err()
    );

    let decode_all = |bytes: &[u8]| -> Result<Vec<f32>> {
        let mut decoder = Decoder::new(bytes)?;
        let mut out = Vec::new();
        let mut buffer = vec![0.0; 333 * decoder.channels()];
        loop {
            let n = decoder.read(&mut buffer)?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buffer[..n * decoder.channels()]);
        }
    };
    for (name, bytes) in inputs() {
        let whole = decode(&bytes).map(|a| interleave(&a, 0));
        // Cut anywhere, including inside a frame or block: the same result as decode,
        // or an error, and never a panic.
        let step = (bytes.len() / 24).max(1);
        for cut in (0..bytes.len()).step_by(step).chain([bytes.len() - 1]) {
            let part = &bytes[..cut];
            let streamed = decode_all(part);
            match (decode(part), &streamed) {
                (Ok(a), Ok(b)) => assert!(same(&interleave(&a, 0), b), "{name} cut at {cut}"),
                (Err(_), Err(e)) => assert!(matches!(
                    e.code,
                    ErrorCode::DecodeError
                        | ErrorCode::UnsupportedFormat
                        | ErrorCode::LimitExceeded
                )),
                (a, b) => panic!(
                    "{name} cut at {cut}: decode {:?}, stream {:?}",
                    a.map(|a| a.frames()),
                    b.as_ref().map(Vec::len)
                ),
            }
        }
        // Corrupt bytes: errors are crate errors, and a failed decoder keeps failing.
        let mut seed = 0x1234_5678_9abc_def1u64 ^ bytes.len() as u64;
        for _ in 0..12 {
            let mut broken = bytes.clone();
            for _ in 0..4 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let at = (seed % broken.len() as u64) as usize;
                broken[at] ^= (seed >> 40) as u8 | 1;
            }
            if let Ok(mut decoder) = Decoder::new(broken.as_slice()) {
                let _ = decoder.seek(decoder.frames().unwrap_or(1000) / 2);
                let mut buffer = vec![0.0; 4096];
                for _ in 0..8 {
                    if decoder.read(&mut buffer).is_err() {
                        assert!(decoder.read(&mut buffer).is_err());
                        break;
                    }
                }
                decoder.rewind().unwrap();
                let _ = decoder.read(&mut buffer);
            }
        }
        assert!(whole.is_ok());
    }
}

#[test]
fn reads_whole_frames_only() {
    let bytes = include_bytes!("fixtures/typescript.wav");
    let whole = decode(bytes).unwrap();
    let mut decoder = Decoder::new(bytes.as_slice()).unwrap();
    let mut buffer = [9.0f32; 7];
    assert_eq!(decoder.read(&mut buffer).unwrap(), 3);
    assert_eq!(buffer[6], 9.0);
    assert!(same(&buffer[..6], &interleave(&whole, 0)[..6]));
    assert_eq!(decoder.read(&mut buffer[..1]).unwrap(), 0);
    assert_eq!(decoder.position(), 3);
    assert_eq!(decoder.format(), AudioFormat::Wav);
    let shared: std::sync::Arc<[u8]> = bytes.to_vec().into();
    let decoder = Decoder::new(shared.clone()).unwrap();
    assert_eq!(decoder.into_inner().len(), shared.len());
}

#[test]
fn limits_apply_when_opening() {
    let bytes = include_bytes!("fixtures/lame-stereo.mp3");
    let short = DecoderOptions {
        limits: Limits {
            max_duration_seconds: 1.0,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        Decoder::with_options(bytes.as_slice(), &short)
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    let small = DecoderOptions {
        limits: Limits {
            max_decoded_bytes: 1000,
            ..Default::default()
        },
        ..Default::default()
    };
    let flac = include_bytes!("fixtures/typescript.flac");
    assert_eq!(
        Decoder::with_options(flac.as_slice(), &small)
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
}
