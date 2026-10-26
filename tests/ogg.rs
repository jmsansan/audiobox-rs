use audiobox::{stream::Decoder, *};
struct Case {
    name: &'static str,
    bytes: &'static [u8],
    reference: &'static [u8],
}
macro_rules! case {
    ($name:literal) => {
        Case {
            name: $name,
            bytes: include_bytes!(concat!("fixtures/ogg/", $name, ".ogg")),
            reference: include_bytes!(concat!("fixtures/ogg/", $name, ".f32")),
        }
    };
}
const VORBIS: &[Case] = &[
    case!("vorbis-floor1-short-shared"),
    case!("vorbis-residue2-eop-stereo"),
    case!("vorbis-lookup-work"),
    case!("vorbis-coupling-forward"),
    case!("vorbis-residue-eop"),
    case!("vorbis-eop-mono"),
    case!("vorbis-floor-eop"),
    case!("vorbis-floor0"),
    case!("vorbis-floor0-even"),
    case!("vorbis-floor0-vector"),
    case!("vorbis-mono-8000"),
    case!("vorbis-mono-16000"),
    case!("vorbis-mono-22050"),
    case!("vorbis-mono-32000"),
    case!("vorbis-mono-44100"),
    case!("vorbis-mono-48000"),
    case!("vorbis-mono-96000"),
    case!("vorbis-stereo-q-1"),
    case!("vorbis-stereo-q0"),
    case!("vorbis-stereo-q4"),
    case!("vorbis-stereo-q10"),
    case!("vorbis-3ch"),
    case!("vorbis-4ch"),
    case!("vorbis-5ch"),
    case!("vorbis-6ch"),
    case!("vorbis-7ch"),
    case!("vorbis-8ch"),
    case!("vorbis-short-1"),
    case!("vorbis-short-31"),
    case!("vorbis-short-64"),
    case!("vorbis-short-127"),
    case!("vorbis-short-128"),
    case!("vorbis-short-129"),
    case!("vorbis-short-255"),
    case!("vorbis-short-1025"),
];
fn shape(c: &Case) -> (u32, usize, usize) {
    let row = include_str!("fixtures/ogg/vorbis.csv")
        .lines()
        .find(|s| s.split(',').next() == Some(c.name))
        .unwrap();
    let mut v = row.split(',').skip(1);
    (
        v.next().unwrap().parse().unwrap(),
        v.next().unwrap().parse().unwrap(),
        v.next().unwrap().parse().unwrap(),
    )
}
#[test]
fn vorbis_matches_independent_pcm_and_container_sample_counts() {
    for c in VORBIS {
        assert_eq!(sniff(c.bytes), Some(AudioFormat::Vorbis));
        let audio = decode(c.bytes).unwrap_or_else(|e| panic!("{}: {e}", c.name));
        let (rate, channels, frames) = shape(c);
        assert_eq!(
            (audio.sample_rate(), audio.channels(), audio.frames()),
            (rate, channels, frames),
            "{}",
            c.name
        );
        assert_eq!(
            audio.info().tags.title.as_deref(),
            Some("audiobox synthetic fixture")
        );
        let samples = audio.to_interleaved();
        assert_eq!(
            samples.len() * 4,
            c.reference.len(),
            "{} reference length",
            c.name
        );
        let mut max_error = 0.0f32;
        for (&actual, bytes) in samples.iter().zip(c.reference.chunks_exact(4)) {
            max_error =
                max_error.max((actual - f32::from_le_bytes(bytes.try_into().unwrap())).abs());
        }
        eprintln!("{}: {frames} frames; max error={max_error}", c.name);
        assert!(max_error < 0.00001, "{}: max error {max_error}", c.name);
    }
}

fn checksum(page: &mut [u8]) {
    page[22..26].fill(0);
    let mut crc = 0u32;
    for &b in page.iter() {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = (crc << 1) ^ if crc & 0x80000000 != 0 { 0x04c11db7 } else { 0 };
        }
    }
    page[22..26].copy_from_slice(&crc.to_le_bytes());
}
fn repair(bytes: &mut [u8]) {
    let mut at = 0;
    while at < bytes.len() {
        let segs = bytes[at + 26] as usize;
        let end = at
            + 27
            + segs
            + bytes[at + 27..at + 27 + segs]
                .iter()
                .map(|&n| n as usize)
                .sum::<usize>();
        checksum(&mut bytes[at..end]);
        at = end;
    }
}
fn packets(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut current = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let segs = bytes[at + 26] as usize;
        let mut body = at + 27 + segs;
        for &n in &bytes[at + 27..body] {
            current.extend_from_slice(&bytes[body..body + n as usize]);
            body += n as usize;
            if n < 255 {
                out.push(std::mem::take(&mut current));
            }
        }
        at = body;
    }
    out
}
fn repage(packets: &[Vec<u8>], granule: u64, laces_per_page: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut seq = 0u32;
    for (i, packet) in packets.iter().enumerate() {
        let mut laces = vec![255u8; packet.len() / 255];
        laces.push((packet.len() % 255) as u8);
        let mut body = 0;
        for (chunk, laces) in laces.chunks(laces_per_page).enumerate() {
            let complete = *laces.last().unwrap() < 255;
            let eos = i + 1 == packets.len() && complete;
            let gp = if eos {
                granule as i64
            } else if i < 3 && complete {
                0
            } else {
                -1
            };
            let mut page = Vec::from(*b"OggS");
            page.extend_from_slice(&[
                0,
                (if seq == 0 { 2 } else { 0 })
                    | (if chunk > 0 { 1 } else { 0 })
                    | (if eos { 4 } else { 0 }),
            ]);
            page.extend_from_slice(&gp.to_le_bytes());
            page.extend_from_slice(&31u32.to_le_bytes());
            page.extend_from_slice(&seq.to_le_bytes());
            page.extend_from_slice(&[0; 4]);
            page.push(laces.len() as u8);
            page.extend_from_slice(laces);
            let size = laces.iter().map(|&n| n as usize).sum::<usize>();
            page.extend_from_slice(&packet[body..body + size]);
            body += size;
            checksum(&mut page);
            out.extend(page);
            seq += 1;
        }
    }
    out
}
#[test]
fn vorbis_continued_packets_and_zero_laces_preserve_pcm() {
    let c = VORBIS
        .iter()
        .find(|c| c.name == "vorbis-stereo-q4")
        .unwrap();
    let (_, _, frames) = shape(c);
    let expected = decode(c.bytes).unwrap().to_interleaved();
    let mut p = packets(c.bytes);
    for length in [234, 255, 65514, 65535, 80000] {
        let value = format!("long={}", "x".repeat(length));
        let mut comment = Vec::from(*b"\x03vorbis");
        comment.extend_from_slice(&0u32.to_le_bytes());
        comment.extend_from_slice(&1u32.to_le_bytes());
        comment.extend_from_slice(&(value.len() as u32).to_le_bytes());
        comment.extend_from_slice(value.as_bytes());
        comment.push(1);
        if length == 255 {
            comment.extend_from_slice(&[0; 17]);
        }
        p[1] = comment;
        let bytes = repage(&p, frames as u64, 1);
        let audio = decode(&bytes).unwrap();
        assert_eq!(audio.to_interleaved(), expected);
        assert_eq!(audio.info().metadata["long"].len(), length);
        let mut d = Decoder::new(bytes).unwrap();
        d.seek(357).unwrap();
        let mut block = [0.0; 14];
        assert_eq!(d.read(&mut block).unwrap(), 7);
        assert_eq!(&block, &expected[714..728]);
    }
}

#[test]
fn vorbis_empty_stream_and_initial_granule_offsets_are_explicit() {
    let c = VORBIS.iter().find(|c| c.name == "vorbis-floor0").unwrap();
    let p = packets(c.bytes);
    let empty = repage(&p[..3], 0, 255);
    assert_eq!(decode(&empty).unwrap().frames(), 0);
    let expected = decode(c.bytes).unwrap().to_interleaved();
    for origin in [12345i64, -16] {
        let mut bytes = repage(&p, 96, 255);
        let mut at = 0;
        let mut page = 0;
        while at < bytes.len() {
            let segs = bytes[at + 26] as usize;
            let end = at
                + 27
                + segs
                + bytes[at + 27..at + 27 + segs]
                    .iter()
                    .map(|&n| n as usize)
                    .sum::<usize>();
            if page >= 3 {
                let nominal = (page - 3) as i64 * 32;
                let granule = (nominal + origin).max(-1);
                bytes[at + 6..at + 14].copy_from_slice(&granule.to_le_bytes());
            }
            page += 1;
            at = end;
        }
        repair(&mut bytes);
        let decoded = decode(&bytes).unwrap().to_interleaved();
        let skip = (-origin).max(0) as usize;
        assert_eq!(decoded, expected[skip..]);
        let mut d = Decoder::new(bytes).unwrap();
        d.seek(31).unwrap();
        let mut out = [0.0; 7];
        d.read(&mut out).unwrap();
        assert_eq!(&out, &expected[skip + 31..skip + 38]);
    }
}
#[test]
fn ogg_truncation_crc_sequence_headers_and_setup_budgets_fail_cleanly() {
    let c = VORBIS
        .iter()
        .find(|c| c.name == "vorbis-stereo-q4")
        .unwrap();
    for end in (0..c.bytes.len()).step_by(7).chain([c.bytes.len() - 1]) {
        assert!(decode(&c.bytes[..end]).is_err(), "truncated {end}");
    }
    for offset in [0, 4, 5, 14, 18, 22, 26, c.bytes.len() - 1] {
        let mut b = c.bytes.to_vec();
        b[offset] ^= 1;
        assert!(decode(&b).is_err());
    }
    let mut p = packets(c.bytes);
    for changes in [
        (11, vec![0xff, 0xff]),
        (13, vec![0xff, 0xff, 0xff]),
        (11, vec![0, 0]),
    ] {
        let original = p[2].clone();
        p[2][changes.0..changes.0 + changes.1.len()].copy_from_slice(&changes.1);
        assert!(decode(&repage(&p, 9637, 255)).is_err());
        p[2] = original;
    }
    let mut b = c.bytes.to_vec();
    b[18] = 8;
    repair(&mut b);
    assert!(decode(&b).is_err());
    let mut b = c.bytes.to_vec();
    b[28 + 11] = 0;
    repair(&mut b);
    assert!(decode(&b).is_err());
    let mut b = c.bytes.to_vec();
    b.extend_from_slice(c.bytes);
    assert!(decode(&b).is_err());
    for limits in [
        Limits {
            max_frames: 9636,
            ..Default::default()
        },
        Limits {
            max_channels: 1,
            ..Default::default()
        },
        Limits {
            max_decoded_bytes: 1024,
            ..Default::default()
        },
        Limits {
            max_sample_rate: 32000,
            ..Default::default()
        },
    ] {
        assert_eq!(
            decode_with_options(
                c.bytes,
                &DecodeOptions {
                    limits: limits.clone(),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
            ErrorCode::LimitExceeded
        );
        assert!(
            Decoder::with_options(
                c.bytes,
                &audiobox::stream::DecoderOptions {
                    limits,
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}
#[test]
fn repaired_crc_mutations_never_panic_or_return_nonfinite_pcm() {
    let mut seed = 37u32;
    for c in VORBIS.iter().filter(|c| !c.name.contains("short")) {
        for _ in 0..24 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let mut b = c.bytes.to_vec();
            // Change a body byte only, preserving page/lacing structure and CRCs.
            let mut at = 0;
            let mut bodies = Vec::new();
            while at < b.len() {
                let segs = b[at + 26] as usize;
                let body = at + 27 + segs;
                let end = body + b[at + 27..body].iter().map(|&x| x as usize).sum::<usize>();
                if end > body {
                    bodies.push((body, end));
                }
                at = end;
            }
            let (from, to) = bodies[(seed as usize) % bodies.len()];
            let index = from + ((seed >> 8) as usize) % (to - from);
            b[index] ^= 1 << ((seed >> 24) % 8);
            repair(&mut b);
            let result = std::panic::catch_unwind(|| {
                decode_with_options(
                    &b,
                    &DecodeOptions {
                        limits: Limits {
                            max_decoded_bytes: 8 * 1024 * 1024,
                            max_frames: 100000,
                            max_channels: 8,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
            });
            match result {
                Ok(Ok(a)) => assert!(a.all_channels().iter().flatten().all(|x| x.is_finite())),
                Ok(Err(_)) => (),
                Err(_) => panic!("{} mutation at {index}", c.name),
            }
        }
    }
}
#[test]
fn vorbis_incremental_and_replayed_seek_are_bit_exact() {
    for c in VORBIS {
        let expected = decode(c.bytes).unwrap().to_interleaved();
        let (_, channels, frames) = shape(c);
        for block in [1, 7, 127, 4096] {
            let mut d = Decoder::new(c.bytes).unwrap();
            let mut scratch = vec![0.0; block * channels];
            let mut actual = Vec::new();
            loop {
                let n = d.read(&mut scratch).unwrap();
                if n == 0 {
                    break;
                }
                actual.extend_from_slice(&scratch[..n * channels]);
            }
            assert_eq!(actual, expected, "{} block {block}", c.name);
            for at in [
                0,
                1,
                31,
                127,
                frames / 2,
                frames.saturating_sub(1),
                frames,
                frames + 1,
            ] {
                d.seek(at as u64).unwrap();
                let at = at.min(frames);
                let n = d.read(&mut scratch).unwrap();
                assert_eq!(
                    &scratch[..n * channels],
                    &expected[at * channels..(at + n) * channels],
                    "{} seek {at}",
                    c.name
                );
            }
            d.rewind().unwrap();
            let n = d.read(&mut scratch).unwrap();
            assert_eq!(&scratch[..n * channels], &expected[..n * channels]);
        }
    }
}
