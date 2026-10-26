//! Exact fixture comparisons, not a promise for arbitrary MP3 input.
use audiobox::*;
use std::fmt::Write;

const HEADER: &str = "case,rate,channels,bitrate,frames,signal,level_bits,source_fnv1a64,mp3_bytes,mp3_fnv1a64,decoded_frames,decoded_fnv1a64";
// Adjacent positive f32 scales found by find_quantizer_boundaries below.
// Each pair changes encoded audio with otherwise identical settings/metadata.
const BOUNDARIES: [[u32; 2]; 6] = [
    [0x3e80_081b, 0x3e80_081c],
    [0x3e80_0c05, 0x3e80_0c06],
    [0x3e80_0199, 0x3e80_019a],
    [0x3e80_09d7, 0x3e80_09d8],
    [0x3e80_0abe, 0x3e80_0abf],
    [0x3e80_10fa, 0x3e80_10fb],
];

#[derive(Clone, Copy)]
struct Case {
    rate: u32,
    channels: usize,
    bitrate: u32,
    frames: usize,
    signal: &'static str,
    level: u32,
}
impl Case {
    fn id(self) -> String {
        format!(
            "{}-{}-{}-{}-{}-{:08x}",
            self.signal, self.rate, self.channels, self.bitrate, self.frames, self.level
        )
    }
    fn source(self) -> Audio {
        let scale = f32::from_bits(self.level);
        Audio::new(
            (0..self.channels)
                .map(|c| {
                    let mut seed = 0x2545_f491u32 ^ c as u32;
                    (0..self.frames)
                        .map(|i| {
                            let sample = match self.signal {
                                "silence" => 0.0,
                                "edge" => {
                                    if i == 0 {
                                        0.5 / (c + 1) as f32
                                    } else if i == self.frames - 1 || i % 576 == 575 {
                                        -0.25 / (c + 1) as f32
                                    } else {
                                        0.0
                                    }
                                }
                                _ => {
                                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                                    let noise =
                                        ((seed >> 9) as i32 - (1 << 22)) as f32 / (1 << 23) as f32;
                                    let triangle = ((i + 13 * c) % 64) as i32 - 32;
                                    noise * 0.5 + triangle as f32 / 128.0
                                }
                            };
                            sample * scale
                        })
                        .collect()
                })
                .collect(),
            self.rate,
        )
        .unwrap()
    }
    fn options(self) -> EncodeOptions {
        let mut options = EncodeOptions {
            bitrate: self.bitrate,
            ..Default::default()
        };
        if self.signal == "tagged" {
            options.tags = Tags {
                title: Some("MP3 contract / original fixture".into()),
                artist: Some("audiobox".into()),
                album: Some("0.5 validation".into()),
                ..Default::default()
            };
            options.tags.cover = Some(
                CoverArt::new("image/png", include_bytes!("fixtures/cover.png").to_vec()).unwrap(),
            );
        }
        options
    }
    fn encode(self) -> Vec<u8> {
        encode_with_options(&self.source(), AudioFormat::Mp3, &self.options()).unwrap()
    }
}
fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for rate in [32000, 44100, 48000] {
        for channels in [1, 2] {
            for bitrate in [32, 128, 320] {
                let base = Case {
                    rate,
                    channels,
                    bitrate,
                    frames: 2305,
                    signal: "seeded",
                    level: 0.5f32.to_bits(),
                };
                for frames in [0, 1, 575, 576, 1151, 1152, 1153, 2305] {
                    cases.push(Case {
                        frames,
                        signal: "edge",
                        ..base
                    });
                }
                cases.push(Case {
                    frames: 3457,
                    ..base
                });
                for level in [0x3380_0000, 0x0d80_0000] {
                    // 2^-24 and 2^-100
                    cases.push(Case {
                        signal: "quiet",
                        level,
                        ..base
                    });
                }
                cases.push(Case {
                    signal: "silence",
                    ..base
                });
                cases.push(Case {
                    signal: "tagged",
                    ..base
                });
            }
            let index = (if rate == 32000 {
                0
            } else if rate == 44100 {
                2
            } else {
                4
            }) + channels
                - 1;
            for level in BOUNDARIES[index] {
                cases.push(Case {
                    rate,
                    channels,
                    bitrate: 128,
                    frames: 2305,
                    signal: "boundary",
                    level,
                });
            }
        }
    }
    cases
}
fn hash(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}
fn pcm_hash(audio: &Audio) -> u64 {
    // Channel-major f32 bits, little endian; no quantization or host byte order.
    hash(
        audio
            .all_channels()
            .iter()
            .flatten()
            .flat_map(|s| s.to_bits().to_le_bytes()),
    )
}
fn collect() -> Vec<String> {
    cases()
        .into_iter()
        .map(|case| {
            let source = case.source();
            let encoded = encode_with_options(&source, AudioFormat::Mp3, &case.options())
                .unwrap_or_else(|e| panic!("{}: {e}", case.id()));
            let decoded = decode(&encoded).unwrap();
            let info = mp3::probe_mp3(&encoded).unwrap();
            assert_eq!(
                (decoded.sample_rate(), decoded.channels(), decoded.frames()),
                (case.rate, case.channels, case.frames),
                "{} decoded shape",
                case.id()
            );
            assert_eq!(
                (
                    info.sample_rate,
                    info.channels,
                    info.frames * 1152 - info.encoder_delay - info.encoder_padding
                ),
                (case.rate, case.channels, case.frames),
                "{} gapless probe",
                case.id()
            );
            format!(
                "{},{},{},{},{},{},{:08x},{:016x},{},{:016x},{},{:016x}",
                case.id(),
                case.rate,
                case.channels,
                case.bitrate,
                case.frames,
                case.signal,
                case.level,
                pcm_hash(&source),
                encoded.len(),
                hash(encoded),
                decoded.frames(),
                pcm_hash(&decoded)
            )
        })
        .collect()
}
fn report(rows: &[String]) {
    if let Some(path) = std::env::var_os("AUDIOBOX_MP3_CONTRACT_REPORT") {
        let path = std::path::Path::new(&path);
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut text = format!("{HEADER}\n");
        for row in rows {
            writeln!(text, "{row}").unwrap();
        }
        std::fs::write(path, text).unwrap();
        std::fs::write(
            path.with_extension("txt"),
            format!(
                "crate={}\narch={}\nos={}\ndebug_assertions={}\ncases={}\n",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::ARCH,
                std::env::consts::OS,
                cfg!(debug_assertions),
                rows.len()
            ),
        )
        .unwrap();
    }
}

#[test]
fn mp3_export_matrix_compares_source_then_encoded_then_decoded() {
    let actual = collect();
    report(&actual); // Preserve actual evidence even if a following comparison fails.
    let mut lines = include_str!("fixtures/mp3-contract.csv").lines();
    assert_eq!(lines.next(), Some(HEADER));
    let expected: Vec<_> = lines
        .map(|line| line.split(',').collect::<Vec<_>>())
        .collect();
    let actual: Vec<_> = actual
        .iter()
        .map(|line| line.split(',').collect::<Vec<_>>())
        .collect();
    assert_eq!(actual.len(), 246);
    assert_eq!(actual.len(), expected.len());
    for row in expected.iter().chain(&actual) {
        assert_eq!(row.len(), 12);
    }
    // Stage ordering prevents blaming codec math for changed fixture PCM/options.
    for (actual, expected) in actual.iter().zip(&expected) {
        assert_eq!(
            &actual[..8],
            &expected[..8],
            "source PCM/options changed: {}",
            actual[0]
        );
    }
    for (actual, expected) in actual.iter().zip(&expected) {
        assert_eq!(
            &actual[8..10],
            &expected[8..10],
            "encoded MP3 differs: {}",
            actual[0]
        );
    }
    for (actual, expected) in actual.iter().zip(&expected) {
        assert_eq!(
            &actual[10..],
            &expected[10..],
            "decoded PCM differs: {}",
            actual[0]
        );
    }
    for pair in actual
        .iter()
        .filter(|r| r[5] == "boundary")
        .collect::<Vec<_>>()
        .chunks_exact(2)
    {
        let low = u32::from_str_radix(pair[0][6], 16).unwrap();
        let high = u32::from_str_radix(pair[1][6], 16).unwrap();
        assert_eq!(
            high,
            low + 1,
            "boundary scales must be adjacent positive f32s"
        );
        assert_ne!(
            pair[0][9], pair[1][9],
            "boundary pair must change MP3 bytes"
        );
    }
}

#[test]
#[ignore = "manual baseline/report generation; review differences before replacing fixtures"]
fn emit_export_matrix() {
    assert!(
        std::env::var_os("AUDIOBOX_MP3_CONTRACT_REPORT").is_some(),
        "set AUDIOBOX_MP3_CONTRACT_REPORT"
    );
    report(&collect());
}

#[test]
#[ignore = "manual search for adjacent f32 scales with differing encoded quantization"]
fn find_quantizer_boundaries() {
    for rate in [32000, 44100, 48000] {
        for channels in [1, 2] {
            let case = Case {
                rate,
                channels,
                bitrate: 128,
                frames: 2305,
                signal: "boundary",
                level: 0,
            };
            let mut low = 0.25f32.to_bits();
            let mut high = 0.5f32.to_bits();
            let reference = Case { level: low, ..case }.encode();
            assert_ne!(
                reference,
                Case {
                    level: high,
                    ..case
                }
                .encode()
            );
            while high - low > 1 {
                let mid = low + (high - low) / 2;
                if (Case { level: mid, ..case }).encode() == reference {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            assert_ne!(
                Case { level: low, ..case }.encode(),
                Case {
                    level: high,
                    ..case
                }
                .encode()
            );
            println!("{rate}/{channels}: [0x{low:08x}, 0x{high:08x}],");
        }
    }
}

#[cfg(not(target_family = "wasm"))]
#[test]
#[ignore = "manual individual source/MP3/decode reproducer"]
fn dump_export_case() {
    let id = std::env::var("AUDIOBOX_MP3_CONTRACT_CASE")
        .expect("set AUDIOBOX_MP3_CONTRACT_CASE to a CSV case ID");
    let case = cases()
        .into_iter()
        .find(|c| c.id() == id)
        .expect("unknown CSV case ID");
    let dir = std::env::var_os("AUDIOBOX_MP3_CONTRACT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target/mp3-contract-case"));
    std::fs::create_dir_all(&dir).unwrap();
    let source = case.source();
    let encoded = case.encode();
    let decoded = decode(&encoded).unwrap();
    write_audio_file(
        dir.join("source.wav"),
        &source,
        &EncodeOptions {
            sample_format: SampleFormat::F32,
            ..Default::default()
        },
    )
    .unwrap();
    std::fs::write(dir.join("encoded.mp3"), encoded).unwrap();
    write_audio_file(
        dir.join("decoded.wav"),
        &decoded,
        &EncodeOptions {
            sample_format: SampleFormat::F32,
            ..Default::default()
        },
    )
    .unwrap();
    std::fs::write(
        dir.join("case.txt"),
        format!(
            "{}\nsource {:016x}\ndecoded {:016x}\n",
            case.id(),
            pcm_hash(&source),
            pcm_hash(&decoded)
        ),
    )
    .unwrap();
}
