use audiobox::{
    Audio, AudioFormat, CoverArt, EncodeOptions, ErrorCode, Tags, decode, encode,
    encode_with_options,
};

const PNG: &[u8] = include_bytes!("fixtures/cover.png");
const JPEG: &[u8] = include_bytes!("fixtures/cover.jpg");
fn audio() -> Audio {
    Audio::new(
        vec![(0..2304).map(|i| ((i as f32) * 0.1).sin() * 0.2).collect()],
        48_000,
    )
    .unwrap()
}
fn tags() -> Tags {
    Tags {
        title: Some("Título 日本語 🎵".into()),
        artist: Some("José".into()),
        album: Some("Álbum".into()),
        album_artist: Some("Equipo".into()),
        track: Some("3/12".into()),
        disc: Some("1/2".into()),
        year: Some("2026".into()),
        genre: Some("Chiptune".into()),
        comment: Some("Primera línea\nSegunda".into()),
        isrc: Some("ESABC2600001".into()),
        cover: Some(CoverArt::new("image/png", PNG.to_vec()).unwrap()),
    }
}
fn wav_tags() -> Tags {
    Tags {
        album_artist: None,
        disc: None,
        isrc: None,
        cover: None,
        ..tags()
    }
}
fn tag_size(bytes: &[u8]) -> usize {
    10 + bytes[6..10].iter().fold(0, |n, &b| (n << 7) | b as usize)
}
fn flac_offset(bytes: &[u8]) -> usize {
    let mut at = 4;
    loop {
        let last = bytes[at] & 128 != 0;
        let n = ((bytes[at + 1] as usize) << 16)
            | ((bytes[at + 2] as usize) << 8)
            | bytes[at + 3] as usize;
        at += 4 + n;
        if last {
            return at;
        }
    }
}
fn wav_payload(bytes: &[u8]) -> &[u8] {
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let n = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if &bytes[at..at + 4] == b"data" {
            return &bytes[at + 8..at + 8 + n];
        }
        at += 8 + n + n % 2;
    }
    panic!("no data")
}

#[test]
fn all_supported_fields_and_cover_round_trip_without_changing_the_audio_payload() {
    let audio = audio();
    for format in [AudioFormat::Wav, AudioFormat::Flac, AudioFormat::Mp3] {
        let tags = if format == AudioFormat::Wav {
            wav_tags()
        } else {
            tags()
        };
        let plain = encode(&audio, format).unwrap();
        let tagged = encode_with_options(
            &audio,
            format,
            &EncodeOptions {
                tags: tags.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        let decoded = decode(&tagged).unwrap();
        assert_eq!(decoded.info().tags, tags, "{format:?}");
        assert_eq!(
            decoded.all_channels(),
            decode(&plain).unwrap().all_channels(),
            "{format:?}"
        );
        match format {
            AudioFormat::Mp3 => {
                assert_eq!(&tagged[..6], b"ID3\x04\0\0");
                assert_eq!(&tagged[tag_size(&tagged)..], plain);
            }
            AudioFormat::Flac => assert_eq!(
                &tagged[flac_offset(&tagged)..],
                &plain[flac_offset(&plain)..]
            ),
            AudioFormat::Wav => assert_eq!(wav_payload(&tagged), wav_payload(&plain)),
            _ => unreachable!(),
        }
    }
}

#[test]
fn native_keys_and_custom_fields_remain_available() {
    for (format, native_title) in [
        (AudioFormat::Wav, "INAM"),
        (AudioFormat::Flac, "title"),
        (AudioFormat::Mp3, "title"),
    ] {
        let native = if format == AudioFormat::Wav {
            [
                ("INAM".into(), "Native".into()),
                ("ICOP".into(), "Original".into()),
            ]
            .into()
        } else {
            [
                ("title".into(), "Native".into()),
                ("custom_field".into(), "Valor español".into()),
            ]
            .into()
        };
        let bytes = encode_with_options(
            &audio(),
            format,
            &EncodeOptions {
                metadata: native,
                ..Default::default()
            },
        )
        .unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.info().metadata[native_title], "Native");
        assert_eq!(decoded.info().tags.title.as_deref(), Some("Native"));
        if format != AudioFormat::Wav {
            assert_eq!(decoded.info().metadata["custom_field"], "Valor español");
        }
    }
    let bytes = encode_with_options(
        &audio(),
        AudioFormat::Flac,
        &EncodeOptions {
            tags: tags(),
            ..Default::default()
        },
    )
    .unwrap();
    let out = decode(&bytes).unwrap();
    assert_eq!(out.info().metadata["albumartist"], "Equipo");
    assert_eq!(out.info().metadata["tracknumber"], "3/12");
    assert_eq!(out.info().metadata["discnumber"], "1/2");
    assert_eq!(out.info().metadata["date"], "2026");
}

#[test]
fn legacy_native_map_keys_keep_their_original_spelling_and_meaning() {
    for format in [AudioFormat::Flac, AudioFormat::Wav] {
        let metadata: audiobox::AudioMetadata = if format == AudioFormat::Flac {
            [
                ("year".into(), "2026".into()),
                ("track".into(), "3".into()),
                ("album_artist".into(), "Team".into()),
            ]
            .into()
        } else {
            [
                ("year".into(), "2026".into()),
                ("date".into(), "literal custom key".into()),
            ]
            .into()
        };
        let bytes = encode_with_options(
            &audio(),
            format,
            &EncodeOptions {
                metadata: metadata.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(decode(&bytes).unwrap().info().metadata, metadata);
    }
}

#[test]
fn jpeg_and_png_front_cover_bytes_round_trip_exactly() {
    for (mime, data) in [("image/png", PNG.to_vec()), ("image/jpeg", JPEG.to_vec())] {
        for format in [AudioFormat::Flac, AudioFormat::Mp3] {
            let cover = CoverArt::new(mime, data.clone()).unwrap();
            let tags = Tags {
                cover: Some(cover.clone()),
                ..Default::default()
            };
            let bytes = encode_with_options(
                &audio(),
                format,
                &EncodeOptions {
                    tags,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(decode(&bytes).unwrap().info().tags.cover, Some(cover));
        }
    }
}

#[test]
fn tagged_incremental_decoding_and_seeking_match_whole_decode() {
    for format in [AudioFormat::Wav, AudioFormat::Flac, AudioFormat::Mp3] {
        let tags = if format == AudioFormat::Wav {
            wav_tags()
        } else {
            tags()
        };
        let bytes = encode_with_options(
            &audio(),
            format,
            &EncodeOptions {
                tags,
                ..Default::default()
            },
        )
        .unwrap();
        let expected = decode(&bytes).unwrap().to_interleaved();
        let mut decoder = audiobox::stream::Decoder::new(bytes).unwrap();
        let mut actual = vec![];
        let mut buffer = [0.0; 37];
        loop {
            let n = decoder.read(&mut buffer).unwrap();
            if n == 0 {
                break;
            }
            actual.extend_from_slice(&buffer[..n]);
        }
        assert_eq!(actual, expected);
        decoder.seek(100).unwrap();
        let n = decoder.read(&mut buffer).unwrap();
        assert_eq!(&buffer[..n], &expected[100..100 + n]);
    }
}

#[test]
fn unsupported_fields_conflicting_aliases_nul_and_invalid_covers_are_errors() {
    for tags in [
        Tags {
            album_artist: Some("x".into()),
            ..Default::default()
        },
        Tags {
            disc: Some("1".into()),
            ..Default::default()
        },
        Tags {
            isrc: Some("x".into()),
            ..Default::default()
        },
        Tags {
            cover: tags().cover,
            ..Default::default()
        },
    ] {
        assert_eq!(
            encode_with_options(
                &audio(),
                AudioFormat::Wav,
                &EncodeOptions {
                    tags,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
            ErrorCode::EncodeError
        );
    }
    for format in [
        AudioFormat::Aiff,
        AudioFormat::Caf,
        AudioFormat::Au,
        AudioFormat::Raw,
    ] {
        assert_eq!(
            encode_with_options(
                &audio(),
                format,
                &EncodeOptions {
                    tags: tags(),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
            ErrorCode::EncodeError
        );
    }
    for format in [AudioFormat::Wav, AudioFormat::Flac, AudioFormat::Mp3] {
        let options = EncodeOptions {
            tags: Tags {
                title: Some("typed".into()),
                ..Default::default()
            },
            metadata: [("TITLE".into(), "map".into())].into(),
            ..Default::default()
        };
        assert_eq!(
            encode_with_options(&audio(), format, &options)
                .unwrap_err()
                .code,
            ErrorCode::EncodeError
        );
        let options = EncodeOptions {
            tags: Tags {
                title: Some("a\0b".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            encode_with_options(&audio(), format, &options)
                .unwrap_err()
                .code,
            ErrorCode::EncodeError
        );
    }
    assert!(CoverArt::new("image/gif", PNG.to_vec()).is_err());
    assert!(CoverArt::new("image/jpeg", PNG.to_vec()).is_err());
}

#[test]
fn metadata_byte_and_field_limits_are_enforced_on_write_and_read() {
    let options = EncodeOptions {
        tags: Tags {
            title: Some("x".repeat(audiobox::metadata::MAX_METADATA_BYTES + 1)),
            ..Default::default()
        },
        ..Default::default()
    };
    for format in [AudioFormat::Wav, AudioFormat::Flac, AudioFormat::Mp3] {
        assert_eq!(
            encode_with_options(&audio(), format, &options)
                .unwrap_err()
                .code,
            ErrorCode::LimitExceeded
        );
    }
    let options = EncodeOptions {
        metadata: (0..=audiobox::metadata::MAX_METADATA_FIELDS)
            .map(|n| (format!("field{n}"), "v".into()))
            .collect(),
        ..Default::default()
    };
    assert_eq!(
        encode_with_options(&audio(), AudioFormat::Flac, &options)
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    let mut id3 = b"ID3\x04\0\0\x08\0\0\0".to_vec();
    id3.extend_from_slice(&encode(&audio(), AudioFormat::Mp3).unwrap());
    assert_eq!(decode(&id3).unwrap_err().code, ErrorCode::LimitExceeded);
}

#[test]
fn mp3_frame_limit_counts_the_front_cover_in_both_directions() {
    let fields = |count| {
        (0..count)
            .map(|n| (format!("field{n}"), "v".into()))
            .collect()
    };
    let options = EncodeOptions {
        metadata: fields(audiobox::metadata::MAX_METADATA_FIELDS - 1),
        tags: Tags {
            cover: Some(CoverArt::new("image/png", PNG.to_vec()).unwrap()),
            ..Default::default()
        },
        ..Default::default()
    };
    let bytes = encode_with_options(&audio(), AudioFormat::Mp3, &options).unwrap();
    let decoded = decode(&bytes).unwrap();
    assert_eq!(
        decoded.info().metadata.len(),
        audiobox::metadata::MAX_METADATA_FIELDS - 1
    );
    assert_eq!(decoded.info().tags.cover, options.tags.cover);
    let options = EncodeOptions {
        metadata: fields(audiobox::metadata::MAX_METADATA_FIELDS),
        ..options
    };
    assert_eq!(
        encode_with_options(&audio(), AudioFormat::Mp3, &options)
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        audiobox::mp3::encode_mp3(&audio(), &options)
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
}

#[test]
fn truncated_and_corrupted_metadata_never_panics_and_returns_errors() {
    let bytes = encode_with_options(
        &audio(),
        AudioFormat::Mp3,
        &EncodeOptions {
            tags: tags(),
            ..Default::default()
        },
    )
    .unwrap();
    for end in 3..tag_size(&bytes) {
        assert!(decode(&bytes[..end]).is_err(), "truncated at {end}");
    }
    let mut corrupt = bytes.clone();
    corrupt[6] = 128;
    assert_eq!(decode(&corrupt).unwrap_err().code, ErrorCode::DecodeError);
    let mut corrupt = bytes.clone();
    corrupt[14] = 128;
    assert_eq!(decode(&corrupt).unwrap_err().code, ErrorCode::DecodeError);
    // Find an UTF-8 text payload and invalidate the first continuation sequence.
    let mut corrupt = bytes;
    let at = corrupt
        .windows(7)
        .position(|s| s == "Título".as_bytes())
        .unwrap();
    corrupt[at + 1] = 255;
    assert_eq!(decode(&corrupt).unwrap_err().code, ErrorCode::DecodeError);
}

#[cfg(feature = "assets")]
#[test]
fn asset_cache_budget_includes_structured_strings_and_cover_storage() {
    use audiobox::assets::{AssetKey, AudioCache};
    use std::sync::Arc;
    let mut small = AudioCache::new(usize::MAX);
    small
        .insert(AssetKey::Name("a".into()), Arc::new(audio()))
        .unwrap();
    let cost = small.used_bytes();
    let bytes = encode_with_options(
        &audio(),
        AudioFormat::Flac,
        &EncodeOptions {
            tags: tags(),
            ..Default::default()
        },
    )
    .unwrap();
    let mut limited = AudioCache::new(cost);
    assert_eq!(
        limited
            .insert(
                AssetKey::Name("a".into()),
                Arc::new(decode(&bytes).unwrap())
            )
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert!(limited.is_empty());
}
