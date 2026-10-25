//! Writes one keyboard-like phrase on a stepped wavetable twice: on the default single
//! voice, where each note restarts the table and cuts the last release, and with
//! `voices=4`, where each note gets a fresh voice and earlier notes ring out.
//!
//! ```text
//! cargo run --example keyboard_voices --features synth -- target/keyboard-voices
//! ```
use audiobox::synth::{RenderOptions, Score};
use audiobox::{EncodeOptions, write_audio_file};
use std::path::PathBuf;

const PHRASE: &str = "q8 o4 l8 [c e g >c< b g e d]2 c2";

fn main() -> audiobox::Result<()> {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "keyboard-voices".into()),
    );
    std::fs::create_dir_all(&directory)?;
    for (name, voices) in [("single-voice", ""), ("voices-4", "voices=4")] {
        let score: Score = format!(
            "tempo 96
instrument keys wavetable=89acdeefffeedca97653211000112356 adsr=6,420,28,360 volume=13 {voices}
channel keys = @keys {PHRASE}"
        )
        .parse()?;
        let audio = score.render_with(
            48_000,
            &RenderOptions {
                gain: 0.5,
                ..Default::default()
            },
        )?;
        let path = directory.join(format!("{name}.wav"));
        write_audio_file(&path, &audio, &EncodeOptions::default())?;
        println!("{}: {} frames", path.display(), audio.frames());
    }
    Ok(())
}
