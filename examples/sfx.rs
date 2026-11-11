//! Writes a few sound effects to WAV files.
//!
//! ```sh
//! cargo run --example sfx --features sfx -- out_dir
//! ```
use audiobox::sfx::{Effect, FmEffect, FmPreset, Preset};
use audiobox::{EncodeOptions, write_audio_file};
use std::path::PathBuf;

/// An effect written by hand, as it could be kept in a game's repository.
const COIN: &str = "
wave=pulse duty=50
frequency=987.8     ; B5
sustain=0.05 punch=0.5 decay=0.3
arpeggio=5 arpeggio-time=0.07   ; up a fourth to E6
volume=0.5
";

fn main() -> audiobox::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "sfx".into()));
    std::fs::create_dir_all(&dir)?;
    let mut effects = vec![("coin".to_string(), COIN.parse::<Effect>()?)];
    for (kind, seed) in [
        (Preset::Laser, 3),
        (Preset::Explosion, 1),
        (Preset::PowerUp, 4),
        (Preset::Hit, 2),
        (Preset::Jump, 7),
        (Preset::Blip, 5),
    ] {
        effects.push((
            format!("{kind:?}-{seed}").to_lowercase(),
            Effect::preset(kind, seed),
        ));
    }
    for (name, effect) in effects {
        let path = dir.join(format!("{name}.wav"));
        write_audio_file(&path, &effect.render(44_100)?, &EncodeOptions::default())?;
        println!("{}: {:.2} s\n{effect}", path.display(), effect.duration());
    }
    // FM effects: one note of an FM patch, drawn from a seed or written by hand.
    for (kind, seed) in [
        (FmPreset::Laser, 2),
        (FmPreset::MetalHit, 5),
        (FmPreset::Chime, 1),
        (FmPreset::Alarm, 3),
    ] {
        let effect = FmEffect::preset(kind, seed);
        let path = dir.join(format!("fm-{}-{seed}.wav", effect.patch.name));
        let audio = effect.render(44_100)?;
        write_audio_file(&path, &audio, &EncodeOptions::default())?;
        let seconds = audio.frames() as f64 / 44_100.0;
        println!("{}: {seconds:.2} s\n{effect}", path.display());
    }
    Ok(())
}
