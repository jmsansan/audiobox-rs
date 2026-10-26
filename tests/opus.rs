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
            bytes: include_bytes!(concat!("fixtures/ogg/", $name, ".opus")),
            reference: include_bytes!(concat!("fixtures/ogg/", $name, ".f32")),
        }
    };
}
const CASES: &[Case] = &[
    case!("opus-silk-8000-1ch"),
    case!("opus-silk-8000-2ch"),
    case!("opus-silk-12000-1ch"),
    case!("opus-silk-12000-2ch"),
    case!("opus-silk-16000-1ch"),
    case!("opus-silk-16000-2ch"),
    case!("opus-hybrid-1ch"),
    case!("opus-hybrid-2ch"),
    case!("opus-celt-2_5ms"),
    case!("opus-celt-5ms"),
    case!("opus-celt-10ms"),
    case!("opus-celt-20ms"),
    case!("opus-celt-40ms"),
    case!("opus-celt-60ms"),
    case!("opus-celt-80ms"),
    case!("opus-celt-100ms"),
    case!("opus-celt-120ms"),
    case!("opus-3ch"),
    case!("opus-4ch"),
    case!("opus-5ch"),
    case!("opus-6ch"),
    case!("opus-7ch"),
    case!("opus-8ch"),
    case!("opus-short-1"),
    case!("opus-short-31"),
    case!("opus-short-120"),
    case!("opus-short-121"),
    case!("opus-short-312"),
    case!("opus-short-313"),
    case!("opus-short-1025"),
    case!("opus-fec"),
    case!("opus-dtx"),
    case!("opus-transitions"),
    case!("opus-gain-12"),
    case!("opus-gain--18"),
];
fn shape(c: &Case) -> (usize, usize) {
    let row = include_str!("fixtures/ogg/opus.csv")
        .lines()
        .find(|r| r.split(',').next() == Some(c.name))
        .unwrap();
    let mut cells = row.split(',').skip(2);
    (
        cells.next().unwrap().parse().unwrap(),
        cells.next().unwrap().parse().unwrap(),
    )
}
#[test]
fn silk_celt_hybrid_multistream_trim_and_gain_match_libopus() {
    let mut failures = Vec::new();
    for c in CASES {
        assert_eq!(sniff(c.bytes), Some(AudioFormat::Opus));
        let (channels, frames) = shape(c);
        let a = decode(c.bytes).unwrap_or_else(|e| panic!("{}: {e}", c.name));
        assert_eq!(
            (a.sample_rate(), a.channels(), a.frames()),
            (48000, channels, frames),
            "{}",
            c.name
        );
        let samples = a.to_interleaved();
        assert_eq!(samples.len() * 4, c.reference.len());
        let mut max = 0f32;
        let mut signal = 0f64;
        let mut error = 0f64;
        for (&x, b) in samples.iter().zip(c.reference.chunks_exact(4)) {
            let y = f32::from_le_bytes(b.try_into().unwrap());
            let d = x - y;
            max = max.max(d.abs());
            signal += (y as f64).powi(2);
            error += (d as f64).powi(2);
        }
        let snr = 10.0 * (signal / error).log10();
        eprintln!("{}: {frames} frames, max={max}, SNR={snr}", c.name);
        // Complete, aligned float output; no lag search or consumer trimming.
        // Modern libopus and the reference float DSP differ in CELT synthesis.
        if max >= 0.006 || (error != 0.0 && snr <= 50.0) {
            failures.push((c.name, max, snr));
        }
    }
    assert!(failures.is_empty(), "Oracle differences: {failures:?}");
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
fn packets(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut packet = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let n = bytes[at + 26] as usize;
        let mut body = at + 27 + n;
        for &n in &bytes[at + 27..body] {
            packet.extend_from_slice(&bytes[body..body + n as usize]);
            body += n as usize;
            if n < 255 {
                out.push(std::mem::take(&mut packet));
            }
        }
        at = body;
    }
    out
}
fn repage(packets: &[Vec<u8>], granule: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, p) in packets.iter().enumerate() {
        let mut laces = vec![255; p.len() / 255];
        laces.push((p.len() % 255) as u8);
        assert!(laces.len() <= 255);
        let last = i + 1 == packets.len();
        let gp = if last {
            granule as i64
        } else if i < 2 {
            0
        } else {
            -1
        };
        let mut page = Vec::from(*b"OggS");
        page.extend_from_slice(&[
            0,
            if i == 0 {
                2
            } else if last {
                4
            } else {
                0
            },
        ]);
        page.extend_from_slice(&gp.to_le_bytes());
        page.extend_from_slice(&17u32.to_le_bytes());
        page.extend_from_slice(&(i as u32).to_le_bytes());
        page.extend_from_slice(&[0; 4]);
        page.push(laces.len() as u8);
        page.extend(laces);
        page.extend(p);
        checksum(&mut page);
        out.extend(page);
    }
    out
}
#[test]
fn opus_headers_mapping_preskip_gain_tags_and_limits() {
    let c = CASES.iter().find(|c| c.name == "opus-celt-20ms").unwrap();
    let p = packets(c.bytes);
    let (channels, frames) = shape(c);
    let skip = u16::from_le_bytes(p[0][10..12].try_into().unwrap()) as usize;
    let expected = decode(c.bytes).unwrap().to_interleaved();
    // Family 1 can duplicate or silence outputs independently of coded channels.
    let mut mapped = p.clone();
    mapped[0][18] = 1;
    mapped[0].extend([1, 1, 1, 255]);
    let actual = decode(&repage(&mapped, (frames + skip) as u64))
        .unwrap()
        .to_interleaved();
    for (a, b) in actual.chunks_exact(2).zip(expected.chunks_exact(2)) {
        assert_eq!(a, &[b[1], 0.0]);
    }
    for extra in [0, 1, 119, 480] {
        let mut q = p.clone();
        q[0][10..12].copy_from_slice(&((skip + extra) as u16).to_le_bytes());
        assert_eq!(
            decode(&repage(&q, (frames + skip) as u64))
                .unwrap()
                .to_interleaved(),
            expected[extra * channels..]
        );
    }
    let mut q = p.clone();
    q[0][12..16].copy_from_slice(&12345u32.to_le_bytes());
    assert_eq!(
        decode(&repage(&q, (frames + skip) as u64))
            .unwrap()
            .to_interleaved(),
        expected
    );
    q[1] = b"OpusTags\0\0\0\0\x01\0\0\0\x0a\0\0\0TITLE=test\0\xff".to_vec();
    let tagged = decode(&repage(&q, (frames + skip) as u64)).unwrap();
    assert_eq!(tagged.to_interleaved(), expected);
    for (offset, value) in [(8, 16), (9, 0), (9, 9), (18, 2), (18, 1)] {
        let mut q = p.clone();
        q[0][offset] = value;
        assert!(decode(&repage(&q, (frames + skip) as u64)).is_err());
    }
    let mut q = mapped.clone();
    q[0][22] = 3;
    assert!(decode(&repage(&q, (frames + skip) as u64)).is_err());
    for limits in [
        Limits {
            max_channels: 1,
            ..Default::default()
        },
        Limits {
            max_frames: frames - 1,
            ..Default::default()
        },
        Limits {
            max_sample_rate: 44100,
            ..Default::default()
        },
        Limits {
            max_decoded_bytes: 1024,
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
    assert!(decode(&repage(&p, (skip - 1) as u64)).is_err());
    assert!(decode(&repage(&p, (frames + skip + 5760) as u64)).is_err());
    assert!(
        expected
            .iter()
            .zip(
                decode(include_bytes!("fixtures/ogg/opus-gain-12.opus"))
                    .unwrap()
                    .to_interleaved()
            )
            .all(|(&a, b)| (b - a * 10f32.powf(12.0 / 20.0)).abs() < 1e-6)
    );
}
#[test]
fn opus_truncation_and_repaired_packet_mutations_do_not_panic() {
    let mut seed = 73u32;
    for c in CASES.iter().filter(|c| !c.name.contains("short")) {
        for end in (0..c.bytes.len()).step_by(131).chain([c.bytes.len() - 1]) {
            assert!(decode(&c.bytes[..end]).is_err());
        }
        let original = packets(c.bytes);
        let (_, frames) = shape(c);
        let skip = u16::from_le_bytes(original[0][10..12].try_into().unwrap()) as usize;
        for _ in 0..96 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let mut p = original.clone();
            let i = 2 + (seed as usize) % (p.len() - 2);
            let j = ((seed >> 8) as usize) % p[i].len();
            p[i][j] ^= 1 << ((seed >> 24) % 8);
            let data = repage(&p, (frames + skip) as u64);
            let result = std::panic::catch_unwind(|| {
                decode_with_options(
                    &data,
                    &DecodeOptions {
                        limits: Limits {
                            max_frames: 100000,
                            max_decoded_bytes: 32 * 1024 * 1024,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
            });
            match result {
                Ok(Ok(a)) => assert!(a.all_channels().iter().flatten().all(|x| x.is_finite())),
                Ok(Err(_)) => (),
                Err(_) => panic!("{} packet {i} byte {j}", c.name),
            }
        }
    }
}
#[test]
fn opus_incremental_and_replayed_seek_preserve_state_exactly() {
    for c in CASES {
        let expected = decode(c.bytes).unwrap().to_interleaved();
        let (channels, frames) = shape(c);
        for block in [7, 4096] {
            let mut d = Decoder::new(c.bytes).unwrap();
            let mut buffer = vec![0.0; block * channels];
            let mut actual = Vec::new();
            loop {
                let n = d.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }
                actual.extend_from_slice(&buffer[..n * channels]);
            }
            assert_eq!(actual, expected, "{} block {block}", c.name);
            for at in [
                0,
                1,
                311,
                frames / 2,
                frames.saturating_sub(1),
                frames,
                frames + 1,
            ] {
                d.seek(at as u64).unwrap();
                let at = at.min(frames);
                let n = d.read(&mut buffer).unwrap();
                assert_eq!(
                    &buffer[..n * channels],
                    &expected[at * channels..(at + n) * channels],
                    "{} seek {at}",
                    c.name
                );
            }
            d.rewind().unwrap();
            let n = d.read(&mut buffer).unwrap();
            assert_eq!(
                &buffer[..n * channels],
                &expected[..n * channels],
                "{} rewind",
                c.name
            );
        }
    }
}

#[test]
fn opus_final_page_trim_and_positive_origin_preserve_pcm() {
    let c = CASES.iter().find(|c| c.name == "opus-celt-20ms").unwrap();
    let p = packets(c.bytes);
    let (_, frames) = shape(c);
    let skip = u16::from_le_bytes(p[0][10..12].try_into().unwrap()) as usize;
    let expected = decode(c.bytes).unwrap().to_interleaved();
    let headers = repage(&p[..2], 0);
    // Keep headers on separate non-EOS pages; all audio completes on one EOS page.
    let mut bytes = headers;
    let second = bytes[26] as usize + 27 + p[0].len();
    bytes[second + 5] = 0;
    checksum(&mut bytes[second..]);
    let mut laces = Vec::new();
    let mut body = Vec::<u8>::new();
    for packet in &p[2..] {
        laces.extend(std::iter::repeat_n(255u8, packet.len() / 255));
        laces.push((packet.len() % 255) as u8);
        body.extend_from_slice(packet);
    }
    assert!(laces.len() <= 255);
    let kept = frames - 1920;
    let mut page = Vec::from(*b"OggS");
    page.extend([0, 4]);
    page.extend(((kept + skip) as u64).to_le_bytes());
    page.extend(17u32.to_le_bytes());
    page.extend(2u32.to_le_bytes());
    page.extend([0; 4]);
    page.push(laces.len() as u8);
    page.extend(laces);
    page.extend(body);
    checksum(&mut page);
    bytes.extend(page);
    let actual = decode(&bytes).unwrap();
    assert_eq!(actual.frames(), kept);
    assert_eq!(actual.to_interleaved(), expected[..kept * 2]);
    let mut d = Decoder::new(bytes).unwrap();
    d.seek((kept - 3) as u64).unwrap();
    let mut out = [0.0; 6];
    assert_eq!(d.read(&mut out).unwrap(), 3);
    assert_eq!(out, expected[(kept - 3) * 2..kept * 2]);
    // A cropped/live stream can preserve a positive source granule origin.
    let mut bytes = repage(&p, (frames + skip) as u64);
    let mut at = 0;
    let mut index = 0;
    while at < bytes.len() {
        let n = bytes[at + 26] as usize;
        let end = at
            + 27
            + n
            + bytes[at + 27..at + 27 + n]
                .iter()
                .map(|&n| n as usize)
                .sum::<usize>();
        if index >= 2 {
            let gp = if index + 1 == p.len() {
                frames + skip
            } else {
                (index - 1) * 960
            };
            bytes[at + 6..at + 14].copy_from_slice(&((gp + 12345) as u64).to_le_bytes());
            checksum(&mut bytes[at..end]);
        }
        at = end;
        index += 1;
    }
    assert_eq!(decode(&bytes).unwrap().to_interleaved(), expected);
}
