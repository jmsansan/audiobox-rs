//! Native file streaming matches memory decoding with bounded resident buffers.
#![cfg(all(feature = "file-stream", not(target_family = "wasm")))]
use audiobox::stream::{FileStream, FileStreamOptions};
use audiobox::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new(bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "audiobox-disk-{}-{}.bin",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn inputs() -> Vec<Vec<u8>> {
    let audio = Audio::new(
        vec![
            (0..20001).map(|i| (i as f32 * 0.05).sin() * 0.3).collect(),
            (0..20001).map(|i| (i as f32 * 0.08).sin() * 0.2).collect(),
        ],
        44100,
    )
    .unwrap();
    let mut inputs = vec![
        include_bytes!("fixtures/typescript.wav").to_vec(),
        include_bytes!("fixtures/typescript.flac").to_vec(),
        include_bytes!("fixtures/typescript.mp3").to_vec(),
        include_bytes!("fixtures/lame-stereo.mp3").to_vec(),
        include_bytes!("fixtures/lame-mpeg2.mp3").to_vec(),
        include_bytes!("fixtures/lame-mpeg25.mp3").to_vec(),
        include_bytes!("fixtures/ima-stereo.wav").to_vec(),
    ];
    for format in [AudioFormat::Wav, AudioFormat::Flac, AudioFormat::Mp3] {
        inputs.push(encode(&audio, format).unwrap());
    }
    inputs
}
fn interleaved(audio: &Audio) -> Vec<f32> {
    (0..audio.frames())
        .flat_map(|i| (0..audio.channels()).map(move |c| audio.channel_data(c).unwrap()[i]))
        .collect()
}
fn read_all(stream: &mut FileStream, size: usize) -> Vec<f32> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buffer = vec![0.0; size * stream.channels()];
    let mut out = Vec::new();
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buffer[..n * stream.channels()]),
            Err(e) if e.code == ErrorCode::Underrun => {
                assert!(Instant::now() < deadline, "reader stalled");
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) => panic!("{e:?}"),
        }
    }
    out
}
fn options() -> FileStreamOptions {
    FileStreamOptions {
        buffer_frames: 509,
        encoded_buffer_bytes: 65536,
        decoder: audiobox::stream::DecoderOptions {
            chunk_frames: 137,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn all_codecs_match_whole_decode_and_seeks_discard_old_queued_frames() {
    for bytes in inputs() {
        let expected = decode(&bytes).unwrap();
        let data = interleaved(&expected);
        let file = Temp::new(&bytes);
        let (mut stream, handle) = FileStream::with_options(&file.0, &options()).unwrap();
        assert_eq!(
            (stream.sample_rate(), stream.channels()),
            (expected.sample_rate(), expected.channels())
        );
        assert_eq!(read_all(&mut stream, 173), data);
        assert_eq!(stream.position(), expected.frames() as u64);
        for target in [
            expected.frames() / 3,
            expected.frames() - 3,
            0,
            expected.frames(),
            17,
        ] {
            stream.seek(4).unwrap(); // superseded request with old audio still in flight
            stream.seek(target as u64).unwrap();
            assert_eq!(
                read_all(&mut stream, 131),
                data[target * expected.channels()..]
            );
        }
        assert!(handle.status().buffered_frames <= 509);
    }
}

#[test]
fn small_encoded_windows_cross_pcm_and_flac_frame_boundaries() {
    let audio = Audio::new(
        vec![(0..60000).map(|i| (i % 97) as f32 / 100.0 - 0.5).collect()],
        8000,
    )
    .unwrap();
    for format in [AudioFormat::Wav, AudioFormat::Flac] {
        let mut bytes = encode(&audio, format).unwrap();
        if format == AudioFormat::Flac {
            // Unknown maximum encoded frame size: bounded refill must handle splits.
            bytes[15..18].fill(0);
        }
        let file = Temp::new(&bytes);
        let opts = FileStreamOptions {
            encoded_buffer_bytes: 16384,
            buffer_frames: 1024,
            ..options()
        };
        let (mut stream, _handle) = FileStream::with_options(&file.0, &opts).unwrap();
        assert_eq!(
            read_all(&mut stream, 1031),
            interleaved(&decode(&bytes).unwrap())
        );
    }
}

#[test]
fn empty_end_rewind_cancel_and_partial_destination_semantics() {
    let audio = Audio::new(vec![vec![0.25; 37], vec![-0.5; 37]], 8000).unwrap();
    let file = Temp::new(&encode(&audio, AudioFormat::Wav).unwrap());
    let (mut stream, handle) = FileStream::open(&file.0).unwrap();
    assert_eq!(stream.read(&mut [99.0]).unwrap(), 0);
    let mut out = [99.0; 81];
    assert_eq!(stream.read(&mut out).unwrap(), 37);
    assert_eq!(&out[74..], &[99.0; 7]);
    assert_eq!(stream.read(&mut out).unwrap(), 0);
    stream.rewind().unwrap();
    assert_eq!(read_all(&mut stream, 100), interleaved(&audio));
    handle.cancel();
    assert_eq!(stream.read(&mut out).unwrap_err().code, ErrorCode::Io);
    assert!(handle.status().cancelled);
    let empty =
        Temp::new(&encode(&Audio::new(vec![vec![]], 8000).unwrap(), AudioFormat::Wav).unwrap());
    let (mut stream, _handle) = FileStream::open(&empty.0).unwrap();
    assert_eq!(stream.read(&mut [0.0; 10]).unwrap(), 0);
}

#[test]
fn input_pcm_codec_header_and_declared_output_budgets_are_checked() {
    let file = Temp::new(
        &encode(
            &Audio::new(vec![vec![0.0; 10000]], 8000).unwrap(),
            AudioFormat::Wav,
        )
        .unwrap(),
    );
    let error =
        |opts: FileStreamOptions| FileStream::with_options(&file.0, &opts).err().unwrap().code;
    assert_eq!(
        error(FileStreamOptions {
            buffer_frames: 0,
            ..options()
        }),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        error(FileStreamOptions {
            max_file_bytes: 100,
            ..options()
        }),
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        error(FileStreamOptions {
            max_pcm_bytes: 1,
            ..options()
        }),
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        error(FileStreamOptions {
            max_decoder_bytes: 1,
            ..options()
        }),
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        error(FileStreamOptions {
            max_header_chunks: 1,
            ..options()
        }),
        ErrorCode::LimitExceeded
    );
    let opts = FileStreamOptions {
        decoder: audiobox::stream::DecoderOptions {
            limits: Limits {
                max_frames: 10,
                ..Default::default()
            },
            ..Default::default()
        },
        ..options()
    };
    assert_eq!(error(opts), ErrorCode::LimitExceeded);
    assert_eq!(
        FileStream::open(file.0.with_extension("missing"))
            .err()
            .unwrap()
            .code,
        ErrorCode::Io
    );
}

#[test]
fn large_sparse_wav_can_seek_without_loading_its_payload() {
    use std::io::{Seek, SeekFrom, Write};
    let mut bytes = encode(&Audio::new(vec![vec![]], 8000).unwrap(), AudioFormat::Wav).unwrap();
    let frames = 4_000_000usize;
    let size = frames * 2;
    let container = (bytes.len() - 8 + size) as u32;
    bytes[4..8].copy_from_slice(&container.to_le_bytes());
    let data = bytes.windows(4).position(|b| b == b"data").unwrap() + 4;
    bytes[data..data + 4].copy_from_slice(&(size as u32).to_le_bytes());
    let file = Temp::new(&bytes);
    let mut disk = std::fs::OpenOptions::new()
        .write(true)
        .open(&file.0)
        .unwrap();
    disk.set_len((bytes.len() + size) as u64).unwrap();
    disk.seek(SeekFrom::End(-2)).unwrap();
    disk.write_all(&16384i16.to_le_bytes()).unwrap();
    drop(disk);
    let (mut stream, _handle) = FileStream::with_options(
        &file.0,
        &FileStreamOptions {
            encoded_buffer_bytes: 4096,
            buffer_frames: 32,
            ..options()
        },
    )
    .unwrap();
    stream.seek((frames - 1) as u64).unwrap();
    assert_eq!(read_all(&mut stream, 7), vec![0.5]);
}

#[test]
fn malformed_headers_and_a_late_decode_error_are_reported_without_losing_prior_pcm() {
    for bytes in [
        vec![],
        b"RIFF\xff\xff\xff\xffWAVE".to_vec(),
        b"fLaC\x80\0\0\x22".to_vec(),
        b"ID3\0\0\0\x7f\x7f\x7f\x7f".to_vec(),
    ] {
        let file = Temp::new(&bytes);
        assert!(FileStream::open(&file.0).is_err());
    }
    let audio = Audio::new(vec![vec![0.25; 3000]], 8000).unwrap();
    let mut bytes = encode_with_options(
        &audio,
        AudioFormat::Wav,
        &EncodeOptions {
            sample_format: SampleFormat::F32,
            ..Default::default()
        },
    )
    .unwrap();
    let payload = bytes.windows(4).position(|b| b == b"data").unwrap() + 8;
    bytes[payload + 1200 * 4..payload + 1201 * 4].copy_from_slice(&f32::NAN.to_le_bytes());
    let file = Temp::new(&bytes);
    let opts = FileStreamOptions {
        buffer_frames: 101,
        decoder: audiobox::stream::DecoderOptions {
            chunk_frames: 100,
            ..Default::default()
        },
        ..options()
    };
    let (mut stream, _handle) = FileStream::with_options(&file.0, &opts).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut buffer = [0.0; 173];
    let mut count = 0;
    loop {
        match stream.read(&mut buffer) {
            Ok(n) => {
                assert!(n > 0);
                assert!(buffer[..n].iter().all(|&v| v == 0.25));
                count += n;
            }
            Err(e) if e.code == ErrorCode::Underrun => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) => {
                assert_eq!(e.code, ErrorCode::DecodeError);
                break;
            }
        }
    }
    assert_eq!(count, 1200);
    assert_eq!(
        stream.read(&mut buffer).unwrap_err().code,
        ErrorCode::DecodeError
    );
    stream.seek(200).unwrap();
    // Seeking clears the sticky error and replays valid source data.
    loop {
        match stream.read(&mut buffer) {
            Ok(n) => {
                assert!(n > 0);
                assert!(buffer[..n].iter().all(|&v| v == 0.25));
                break;
            }
            Err(e) if e.code == ErrorCode::Underrun => std::thread::sleep(Duration::from_millis(1)),
            Err(e) => panic!("{e:?}"),
        }
    }
}

#[test]
fn unknown_length_flac_delivers_the_valid_prefix_before_a_sticky_output_limit() {
    let mut bytes = encode(
        &Audio::new(vec![vec![0.25; 10000]], 8000).unwrap(),
        AudioFormat::Flac,
    )
    .unwrap();
    bytes[21] &= 0xf0;
    bytes[22..26].fill(0);
    let file = Temp::new(&bytes);
    let opts = FileStreamOptions {
        decoder: audiobox::stream::DecoderOptions {
            limits: Limits {
                max_frames: 5000,
                ..Default::default()
            },
            chunk_frames: 137,
            ..Default::default()
        },
        ..options()
    };
    let (mut source, _handle) = FileStream::with_options(&file.0, &opts).unwrap();
    assert_eq!(source.frames(), None);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut buffer = [0.0; 311];
    let mut count = 0;
    loop {
        match source.read(&mut buffer) {
            Ok(n) => {
                assert!(n > 0);
                assert!(buffer[..n].iter().all(|&v| v == 0.25));
                count += n;
            }
            Err(e) if e.code == ErrorCode::Underrun => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) => {
                assert_eq!(e.code, ErrorCode::LimitExceeded);
                break;
            }
        }
    }
    assert_eq!(count, 5000);
    assert_eq!(
        source.read(&mut buffer).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
}

#[cfg(feature = "mixer")]
#[test]
fn mixer_plays_disk_and_worker_seeks_whole_and_region_loops() {
    use audiobox::mixer::*;
    let audio = Audio::new(vec![(0..1000).map(|i| i as f32 / 2000.0).collect()], 8000).unwrap();
    let file = Temp::new(&encode(&audio, AudioFormat::Wav).unwrap());
    for looping in [
        Loop::Off,
        Loop::Whole,
        Loop::Region {
            start: 200,
            end: 700,
        },
    ] {
        let (source, handle) = FileStream::open(&file.0).unwrap();
        let (mut mixer, mut control) = Mixer::new(MixerConfig {
            sample_rate: 8000,
            channels: 1,
            ..Default::default()
        })
        .unwrap();
        control
            .play(
                Sound::stream(source),
                &Play {
                    looping,
                    ..Default::default()
                },
            )
            .unwrap();
        let mut out = Vec::new();
        let mut buffer = [0.0; 100];
        for _ in 0..25 {
            mixer.render(&mut buffer);
            out.extend_from_slice(&buffer);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            &out[..650],
            &interleaved(&decode(&std::fs::read(&file.0).unwrap()).unwrap())[..650]
        );
        if looping == Loop::Off {
            assert!(out[1100..].iter().all(|&v| v == 0.0));
        } else {
            assert!(out[1500..].iter().any(|&v| v > 0.05));
        }
        while control.poll_event().is_some() {}
        assert!(!handle.status().cancelled || looping == Loop::Off);
    }
}
