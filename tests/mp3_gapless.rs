use audiobox::*;

fn crc(bytes: &[u8]) -> u16 {
    // Independent MSB-first 0x8005 reference with reflected input/output.
    let mut value = 0u16;
    for &byte in bytes {
        value ^= (byte.reverse_bits() as u16) << 8;
        for _ in 0..8 {
            value = (value << 1) ^ if value & 0x8000 != 0 { 0x8005 } else { 0 };
        }
    }
    value.reverse_bits()
}
fn info_offset(bytes: &[u8]) -> usize {
    if bytes.starts_with(b"ID3") {
        10 + bytes[6..10].iter().fold(0, |n, &b| (n << 7) | b as usize)
    } else {
        0
    }
}
fn source(frames: usize, rate: u32, channels: usize) -> Audio {
    Audio::new(
        (0..channels)
            .map(|c| {
                (0..frames)
                    .map(|i| {
                        let n = ((i * 13 + c * 17) % 128) as i32 - 64;
                        n as f32 / 256.0
                    })
                    .collect()
            })
            .collect(),
        rate,
    )
    .unwrap()
}

#[test]
fn gapless_extension_checksums_and_shapes_cover_short_signals_and_all_rates() {
    assert_eq!(crc(b"123456789"), 0xbb3d);
    for rate in [32000, 44100, 48000] {
        for channels in [1, 2] {
            for bitrate in [32, 320] {
                for frames in [0, 1, 48, 49, 576, 1152, 1153, 12001] {
                    let a = source(frames, rate, channels);
                    let options = EncodeOptions {
                        bitrate,
                        ..Default::default()
                    };
                    let bytes = encode_with_options(&a, AudioFormat::Mp3, &options).unwrap();
                    let offset = info_offset(&bytes);
                    let side = if channels == 1 { 17 } else { 32 };
                    let p = offset + 4 + side + 16;
                    assert_eq!(&bytes[p..p + 9], b"LAMEabox1");
                    assert_eq!(bytes[p + 9], 1);
                    assert_eq!(
                        u16::from_be_bytes(bytes[p + 34..p + 36].try_into().unwrap()),
                        crc(&bytes[offset..p + 34])
                    );
                    let tag_len = 144 * bitrate.max(64) as usize * 1000 / rate as usize;
                    assert_eq!(
                        u16::from_be_bytes(bytes[p + 32..p + 34].try_into().unwrap()),
                        crc(&bytes[offset + tag_len..])
                    );
                    assert_eq!(
                        u32::from_be_bytes(bytes[p + 28..p + 32].try_into().unwrap()) as usize,
                        bytes.len() - offset
                    );
                    let info = mp3::probe_mp3(&bytes).unwrap();
                    assert!(info.frames >= 2);
                    assert_eq!(info.encoder_delay, 528);
                    assert_eq!(
                        info.frames * 1152 - info.encoder_delay - info.encoder_padding,
                        frames
                    );
                    let decoded = decode(&bytes).unwrap();
                    assert_eq!(
                        (decoded.frames(), decoded.channels(), decoded.sample_rate()),
                        (frames, channels, rate)
                    );
                    let mut stream =
                        stream::Decoder::new(std::sync::Arc::<[u8]>::from(bytes)).unwrap();
                    let mut out = vec![0.0; decoded.frames() * channels];
                    let mut at = 0;
                    while at < out.len() {
                        let end = (at + 7 * channels).min(out.len());
                        let n = stream.read(&mut out[at..end]).unwrap();
                        assert!(n > 0);
                        at += n * channels;
                    }
                    assert_eq!(out, decoded.to_interleaved());
                    let mut end = [0.0; 2];
                    assert_eq!(stream.read(&mut end[..channels]).unwrap(), 0);
                    for frame in [0, frames / 2, frames] {
                        stream.seek(frame as u64).unwrap();
                        let mut block = vec![0.0; 7 * channels];
                        let n = stream.read(&mut block).unwrap();
                        assert_eq!(
                            &block[..n * channels],
                            &out[frame * channels..frame * channels + n * channels]
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn structured_tags_cover_and_legacy_info_identity_keep_decoding() {
    let a = source(2305, 48000, 2);
    let tags = Tags {
        title: Some("Gapless fixture".into()),
        artist: Some("audiobox".into()),
        album: Some("Original synthetic control".into()),
        track: Some("1/2".into()),
        year: Some("2026".into()),
        genre: Some("Synthetic".into()),
        isrc: Some("ZZABC2600001".into()),
        cover: Some(
            CoverArt::new("image/png", include_bytes!("fixtures/cover.png").to_vec()).unwrap(),
        ),
        ..Default::default()
    };
    let mut bytes = encode_with_options(
        &a,
        AudioFormat::Mp3,
        &EncodeOptions {
            tags: tags.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    let decoded = decode(&bytes).unwrap();
    assert_eq!(decoded.info().tags, tags);
    let offset = info_offset(&bytes);
    let p = offset + 4 + 32 + 16;
    bytes[p..p + 9].copy_from_slice(b"audiobox ");
    // Previous audiobox Info tags did not carry extension CRCs.
    bytes[p + 24..p + 36].fill(0);
    let legacy = decode(&bytes).unwrap();
    assert_eq!(legacy.all_channels(), decoded.all_channels());
    assert_eq!(legacy.info().tags, tags);
}

#[test]
fn short_framing_retains_prefix_and_supplies_the_missing_coded_tail() {
    for frames in [1, 31, 32, 47, 48] {
        let a = source(frames, 48000, 2);
        let current = encode_with_options(
            &a,
            AudioFormat::Mp3,
            &EncodeOptions {
                bitrate: 320,
                ..Default::default()
            },
        )
        .unwrap();
        let decoded = decode(&current).unwrap();
        let mut legacy = current;
        legacy.truncate(960 + 960);
        let at = 36;
        let p = at + 16;
        let size = legacy.len() as u32;
        legacy[at + 8..at + 12].copy_from_slice(&1u32.to_be_bytes());
        legacy[at + 12..at + 16].copy_from_slice(&size.to_be_bytes());
        legacy[p..p + 9].copy_from_slice(b"audiobox ");
        legacy[p + 9..p + 36].fill(0);
        let padding = 1152 - frames - 528;
        legacy[p + 21] = (528 >> 4) as u8;
        legacy[p + 22] = (padding >> 8) as u8;
        legacy[p + 23] = padding as u8;
        let old = decode(&legacy).unwrap();
        assert_eq!(old.frames(), frames);
        for (before, after) in old.all_channels().iter().zip(decoded.all_channels()) {
            assert_eq!(&before[..frames.min(31)], &after[..frames.min(31)]);
        }
        let max = old
            .all_channels()
            .iter()
            .flatten()
            .zip(decoded.all_channels().iter().flatten())
            .map(|(&x, &y)| (x - y).abs())
            .fold(0.0f32, f32::max);
        println!("{frames} frames: old/new decoded max difference {max}");
        if frames <= 31 {
            assert_eq!(max, 0.0)
        } else {
            assert!(max > 0.0 && max < 0.00001);
        }
    }
}
