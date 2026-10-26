#![cfg(not(target_family = "wasm"))]
use audiobox::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "audiobox-reader-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, bytes: &[u8]) -> PathBuf {
        let path = self.0.join("input");
        std::fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn exact_encoded_limits_preserve_wav_flac_and_mp3_decodes() {
    let dir = Temp::new();
    let audio = Audio::new(
        vec![
            (0..1601)
                .map(|i| if i % 4 < 2 { 0.25 } else { -0.25 })
                .collect();
            2
        ],
        48000,
    )
    .unwrap();
    for format in [AudioFormat::Wav, AudioFormat::Flac, AudioFormat::Mp3] {
        let bytes = encode(&audio, format).unwrap();
        let path = dir.write(&bytes);
        let exact = ReadAudioOptions {
            max_encoded_bytes: bytes.len(),
            ..Default::default()
        };
        for read in [
            read_audio_file(&path).unwrap(),
            read_audio_file_with_options(&path, &exact).unwrap(),
        ] {
            let expected = decode(&bytes).unwrap();
            assert_eq!(read.frames(), expected.frames());
            assert_eq!(read.sample_rate(), expected.sample_rate());
            assert_eq!(read.all_channels(), expected.all_channels());
        }
        assert_eq!(
            read_audio_file_with_options(
                &path,
                &ReadAudioOptions {
                    max_encoded_bytes: bytes.len() - 1,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
            ErrorCode::LimitExceeded
        );
        let mut decoded_limit = exact;
        decoded_limit.decode.limits.max_decoded_bytes = 4;
        assert_eq!(
            read_audio_file_with_options(&path, &decoded_limit)
                .unwrap_err()
                .code,
            ErrorCode::LimitExceeded
        );
    }
}

#[test]
fn rejects_nonregular_oversized_missing_and_invalid_input_with_stable_errors() {
    let dir = Temp::new();
    assert_eq!(
        read_audio_file(&dir.0).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        read_audio_file(dir.0.join("absent")).unwrap_err().code,
        ErrorCode::Io
    );
    for bytes in [&[][..], &b"not audio"[..]] {
        let path = dir.write(bytes);
        assert_eq!(
            read_audio_file_with_options(
                path,
                &ReadAudioOptions {
                    max_encoded_bytes: bytes.len(),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
            ErrorCode::UnsupportedFormat
        );
    }
    let path = dir.write(&[]);
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    // Extending a file without writing payload exercises the metadata rejection.
    file.set_len(1024 * 1024).unwrap();
    drop(file);
    assert_eq!(
        read_audio_file_with_options(
            &path,
            &ReadAudioOptions {
                max_encoded_bytes: 1024,
                ..Default::default()
            }
        )
        .unwrap_err()
        .code,
        ErrorCode::LimitExceeded
    );
    #[cfg(unix)]
    {
        let path = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        path.set_len(1024 * 1024 * 1024).unwrap();
        assert_eq!(
            read_audio_file(dir.0.join("input")).unwrap_err().code,
            ErrorCode::LimitExceeded
        );
        assert_eq!(
            read_audio_file("/dev/null").unwrap_err().code,
            ErrorCode::InvalidArgument
        );
    }
}

#[cfg(unix)]
#[test]
fn follows_regular_file_symlinks_without_project_containment() {
    let dir = Temp::new();
    let other = Temp::new();
    let bytes = encode(
        &Audio::new(vec![vec![0.25; 17]], 8000).unwrap(),
        AudioFormat::Wav,
    )
    .unwrap();
    let target = other.write(&bytes);
    let link = dir.0.join("linked");
    std::os::unix::fs::symlink(target, &link).unwrap();
    assert_eq!(read_audio_file(link).unwrap().frames(), 17);
}

#[cfg(unix)]
#[test]
fn rejects_direct_and_linked_fifos_in_bounded_children() {
    use std::process::Command;
    use std::time::{Duration, Instant};
    let dir = Temp::new();
    let fifo = dir.0.join("fifo");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let link = dir.0.join("link");
    std::os::unix::fs::symlink(&fifo, &link).unwrap();
    for path in [fifo, link] {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "fifo_child", "--ignored", "--nocapture"])
            .env("AUDIOBOX_FIFO_CHILD", path)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("static FIFO read blocked");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(unix)]
#[test]
#[ignore = "run by the bounded parent with a FIFO path"]
fn fifo_child() {
    let Some(path) = std::env::var_os("AUDIOBOX_FIFO_CHILD") else {
        return;
    };
    assert_eq!(
        read_audio_file(path).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
}
