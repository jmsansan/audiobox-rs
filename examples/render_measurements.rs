//! Numerical measurements only; the caller owns review thresholds and decisions.
use audiobox::analyze::MeasurementBoundary;
use audiobox::{Audio, Result, read_audio_file};

fn main() -> Result<()> {
    let audio = if let Some(path) = std::env::args().nth(1) {
        read_audio_file(path)?
    } else {
        Audio::new(
            vec![
                vec![0.0, 0.0, 0.5, -0.25, 0.25, 0.0, 0.0, 0.0],
                vec![0.0, 0.0, 0.25, -0.125, 0.125, 0.0, 0.0, 0.0],
            ],
            48000,
        )?
    };
    println!(
        "{} frames, {} channels, {} Hz",
        audio.frames(),
        audio.channels(),
        audio.sample_rate()
    );
    println!("DC means (sample amplitude): {:?}", audio.dc_means()?);
    println!(
        "Silence (threshold 0.001 linear): {:?}",
        audio.threshold_silence(0.001)?
    );
    if audio.channels() == 2 {
        println!(
            "Stereo (power in squared sample amplitude): {:?}",
            audio.stereo_measurements()?
        );
    }
    let mut boundaries = vec![MeasurementBoundary::LoopSeam];
    if audio.frames() >= 2 {
        boundaries.push(MeasurementBoundary::Frame(audio.frames() / 2));
    }
    println!(
        "Explicit joins (signed sample steps): {:?}",
        audio.measure_boundaries(&boundaries)?
    );
    Ok(())
}
