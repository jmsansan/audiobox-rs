//! Synthetic intersample-peak control, exported with both offline limiters.
//! cargo run --example true_peak_limiter -- target/true-peak-limiter
use audiobox::{
    Audio, AudioFormat, EncodeOptions, LimiterOptions, SampleFormat, TruePeakLimiterOptions,
    encode_with_options,
};
use std::path::PathBuf;

fn main() -> audiobox::Result<()> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/true-peak-limiter"));
    std::fs::create_dir_all(&directory)?;
    let samples: Vec<f32> = (0..96_000)
        .map(|i| {
            let level = if (i / 12_000) % 2 == 0 { 1.2 } else { 0.2 };
            if i % 4 < 2 { level } else { -level }
        })
        .collect();
    let source = Audio::new(
        vec![samples.iter().map(|s| s * 0.5).collect(), samples],
        48_000,
    )?;
    let sample_limited = source.limit(LimiterOptions::default())?;
    let true_peak_limited = source.limit_true_peak(TruePeakLimiterOptions::default())?;
    for (name, audio) in [
        ("source", &source),
        ("sample-limited", &sample_limited),
        ("true-peak-limited", &true_peak_limited),
    ] {
        let path = directory.join(format!("{name}.wav"));
        std::fs::write(
            &path,
            encode_with_options(
                audio,
                AudioFormat::Wav,
                &EncodeOptions {
                    sample_format: SampleFormat::F32,
                    ..Default::default()
                },
            )?,
        )?;
        println!(
            "{}: {} frames, sample {:.3} dBFS, existing 4x estimator {:.3} dBTP",
            path.display(),
            audio.frames(),
            audio.peak_db(),
            audio.true_peak_db()
        );
    }
    Ok(())
}
