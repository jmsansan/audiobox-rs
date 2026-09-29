//! Independent format verification. Set AUDIOBOX_FFMPEG_TESTS=1 to enable.
use audiobox::*;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "audiobox-rust-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn enabled() -> bool {
    if std::env::var_os("AUDIOBOX_FFMPEG_TESTS").is_none() {
        return false;
    }
    assert!(
        Command::new("ffmpeg")
            .arg("-version")
            .output()
            .expect("AUDIOBOX_FFMPEG_TESTS requires ffmpeg")
            .status
            .success()
    );
    true
}
fn run(args: &[&str]) {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn text(p: &Path) -> &str {
    p.to_str().unwrap()
}
fn raw(path: &Path) -> Vec<f32> {
    std::fs::read(path)
        .unwrap()
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}
fn tone(n: usize, rate: u32, channels: usize) -> Audio {
    Audio::new(
        (0..channels)
            .map(|c| {
                (0..n)
                    .map(|i| {
                        (0.7 * (std::f64::consts::TAU * 440.0 * (c + 1) as f64 * i as f64
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
#[test]
fn pcm_and_flac_outputs_match_ffmpeg() {
    if !enabled() {
        return;
    }
    let tmp = Temp::new();
    let source = tone(12001, 44100, 2);
    for (format, ext) in [
        (AudioFormat::Wav, "wav"),
        (AudioFormat::Aiff, "aiff"),
        (AudioFormat::Au, "au"),
        (AudioFormat::Caf, "caf"),
        (AudioFormat::Flac, "flac"),
    ] {
        for f in [
            SampleFormat::S16,
            SampleFormat::S24,
            SampleFormat::S32,
            SampleFormat::F32,
            SampleFormat::F64,
            SampleFormat::Alaw,
            SampleFormat::Ulaw,
        ] {
            if format == AudioFormat::Flac
                && !matches!(f, SampleFormat::S16 | SampleFormat::S24 | SampleFormat::S32)
            {
                continue;
            }
            let input = tmp.path(&format!("out.{ext}"));
            let output = tmp.path("reference.raw");
            let bytes = encode_with_options(
                &source,
                format,
                &EncodeOptions {
                    sample_format: f,
                    ..Default::default()
                },
            )
            .unwrap();
            std::fs::write(&input, &bytes).unwrap();
            run(&["-i", text(&input), "-f", "f32le", text(&output)]);
            let reference = raw(&output);
            let decoded = decode(&bytes).unwrap().to_interleaved();
            assert_eq!(decoded.len(), reference.len(), "{format:?} {f:?}");
            let diff = decoded
                .iter()
                .zip(reference)
                .map(|(&a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(diff < 1e-7, "{format:?} {f:?}: max difference {diff}");
        }
    }
}
#[test]
fn external_pcm_flac_adpcm_and_rf64_inputs() {
    if !enabled() {
        return;
    }
    let tmp = Temp::new();
    for (ext, codec, extra) in [
        ("wav", "pcm_s24le", vec![]),
        ("wav", "pcm_f32le", vec!["-rf64", "always"]),
        ("wav", "adpcm_ima_wav", vec![]),
        ("aiff", "pcm_s16be", vec![]),
        ("caf", "pcm_s8", vec![]),
        ("caf", "pcm_s24le", vec![]),
        ("au", "pcm_mulaw", vec![]),
        ("flac", "flac", vec![]),
    ] {
        let path = tmp.path(&format!("external.{ext}"));
        let out = tmp.path("reference.raw");
        let mut args = vec![
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=0.3",
            "-ac",
            "2",
            "-c:a",
            codec,
        ];
        args.extend(extra);
        args.push(text(&path));
        run(&args);
        run(&["-i", text(&path), "-f", "f32le", text(&out)]);
        let reference = raw(&out);
        let decoded = read_audio_file(&path).unwrap().to_interleaved();
        if codec != "adpcm_ima_wav" {
            assert_eq!(decoded.len(), reference.len(), "{ext} {codec}");
        } else {
            assert!(reference.len() >= decoded.len());
        }
        let diff = decoded
            .iter()
            .zip(reference)
            .map(|(&a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(diff < 1e-6, "{ext} {codec}: {diff}");
    }
}
#[test]
fn mp3_outputs_have_correct_gain_delay_and_quality_in_ffmpeg() {
    if !enabled() {
        return;
    }
    let tmp = Temp::new();
    for rate in [32000, 44100, 48000] {
        for channels in [1, 2] {
            let source = tone(12001, rate, channels);
            let path = tmp.path("out.mp3");
            let out = tmp.path("reference.raw");
            std::fs::write(&path, encode(&source, AudioFormat::Mp3).unwrap()).unwrap();
            run(&["-i", text(&path), "-f", "f32le", text(&out)]);
            let reference = raw(&out);
            assert!(reference.len() >= source.frames() * channels, "rate {rate}");
            let info = mp3::probe_mp3(&std::fs::read(&path).unwrap()).unwrap();
            // FFmpeg only honors priming for its encoder-name whitelist. Align the
            // reference by the tag when it reports the complete MPEG frames.
            let reference = if reference.len() == source.frames() * channels {
                reference
            } else {
                reference[(info.encoder_delay + 529) * channels
                    ..(info.encoder_delay + 529 + source.frames()) * channels]
                    .to_vec()
            };
            let samples = source.to_interleaved();
            let start = 1000 * channels;
            let end = 10000 * channels;
            let energy = samples[start..end]
                .iter()
                .map(|&s| (s as f64).powi(2))
                .sum::<f64>();
            let error = samples[start..end]
                .iter()
                .zip(&reference[start..end])
                .map(|(&a, &b)| ((a - b) as f64).powi(2))
                .sum::<f64>();
            let snr = 10.0 * (energy / error).log10();
            assert!(snr > 30.0, "rate {rate}, channels {channels}, SNR {snr}");
        }
    }
}
#[test]
fn external_mp3_all_rates_joint_stereo_and_vbr() {
    if !enabled() {
        return;
    }
    let tmp = Temp::new();
    for rate in [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000] {
        for channels in [1, 2] {
            let path = tmp.path("external.mp3");
            let out = tmp.path("reference.raw");
            let rate_s = rate.to_string();
            let channels_s = channels.to_string();
            let input = format!("sine=frequency=440:sample_rate={rate}:duration=0.5");
            run(&[
                "-f",
                "lavfi",
                "-i",
                &input,
                "-ac",
                &channels_s,
                "-ar",
                &rate_s,
                "-c:a",
                "libmp3lame",
                "-q:a",
                "2",
                text(&path),
            ]);
            run(&["-i", text(&path), "-f", "f32le", text(&out)]);
            let reference = raw(&out);
            let decoded = read_audio_file(&path).unwrap().to_interleaved();
            assert_eq!(
                decoded.len(),
                reference.len(),
                "rate {rate}, channels {channels}"
            );
            let start = 1000.min(decoded.len() / 4);
            let end = decoded.len() - start;
            let energy = reference[start..end]
                .iter()
                .map(|&s| (s as f64).powi(2))
                .sum::<f64>();
            let error = reference[start..end]
                .iter()
                .zip(&decoded[start..end])
                .map(|(&a, &b)| ((a - b) as f64).powi(2))
                .sum::<f64>();
            let snr = 10.0 * (energy / error).log10();
            assert!(snr > 35.0, "rate {rate}, channels {channels}, SNR {snr}");
        }
    }
}
