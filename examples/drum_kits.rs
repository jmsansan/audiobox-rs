//! The same synthesized drum pattern in two kits. No recorded samples or device.
use audiobox::synth::{RenderOptions, Score};
use audiobox::{EncodeOptions, Result, write_audio_file};
use std::path::PathBuf;

fn main() -> Result<()> {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "drum-kits".into()),
    );
    std::fs::create_dir_all(&directory)?;
    for kit in ["chiptune", "electronic"] {
        let text = format!(
            "tempo 120
instrument k drum=kick kit={kit}
instrument s drum=snare kit={kit} drum-seed=123
instrument h drum=closed-hat kit={kit}
instrument o drum=open-hat kit={kit}
instrument t drum=tom kit={kit}
instrument c drum=clap kit={kit}
instrument x drum=crash kit={kit}
channel kick = @k l8 [c!127 r c!80 r c!110 c!70 r c!100]2
channel snare = @s [r4 c4!100]4
channel hats = l8 [@h c!90 c!50 c!80 @o c!80 @h c!90 c!50 c!80 r]2
channel tom = @t r1 r2 c8 d8 e8 g8
channel clap = @c [r2 c4!90 r4]2
channel crash = @x c1!90 r1"
        );
        let score: Score = text.parse()?;
        let audio = score.render_with(
            48_000,
            &RenderOptions {
                gain: 0.2,
                ..Default::default()
            },
        )?;
        let path = directory.join(format!("{kit}.wav"));
        write_audio_file(&path, &audio, &EncodeOptions::default())?;
        println!(
            "{}: {} frames at 48000 Hz, synthesized {kit} kit",
            path.display(),
            audio.frames()
        );
    }
    Ok(())
}
