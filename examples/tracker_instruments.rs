//! Four opt-in tracker voices, also exported separately for comparison.
use audiobox::synth::{RenderOptions, Score};
use audiobox::{EncodeOptions, Result, write_audio_file};
use std::path::PathBuf;

fn main() -> Result<()> {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "tracker-instruments".into()),
    );
    std::fs::create_dir_all(&directory)?;
    let definitions = "tempo 150
instrument table wavetable=0123579bdeffffedb9753210000012357 adsr=3,40,70,20 macro-volume=15,12,10,8 macro-arpeggio=0,12,7 macro-arpeggio-step=ticks,3 macro-arpeggio-loop=0
instrument pulse wave=pulse adsr=2,30,70,20 quantize=ntsc-pulse macro-duty=0,1,2,1 macro-duty-loop=0 macro-pitch=0,-8,8 macro-pitch-loop=0
instrument noise wave=noise noise=short noise-period=5 macro-volume=15,9,3,0
instrument pcm sample=000204080c0f0c080402000306090c0f09 sample-bits=4 sample-rate=4000 sample-loop=on adsr=0,15,60,20 quantize=volume";
    let parts = [
        ("table", "@table o5 l8 [c e g >c<]2"),
        ("pulse", "@pulse o3 l4 [c g]2"),
        ("noise", "@noise l16 [c r c r c c r r]2"),
        ("pcm", "@pcm o3 l8 [c!100 r g!80 r]2"),
    ];
    let mut combined = definitions.to_owned();
    for (name, commands) in parts {
        let channel = format!("\nchannel {name}={commands}");
        combined.push_str(&channel);
        save(
            &directory.join(format!("{name}.wav")),
            &format!("{definitions}{channel}"),
            0.5,
        )?;
    }
    save(&directory.join("combined.wav"), &combined, 0.2)
}
fn save(path: &std::path::Path, text: &str, gain: f32) -> Result<()> {
    let score: Score = text.parse()?;
    let audio = score.render_with(
        48_000,
        &RenderOptions {
            gain,
            ..Default::default()
        },
    )?;
    write_audio_file(path, &audio, &EncodeOptions::default())?;
    println!("{}: {} frames", path.display(), audio.frames());
    Ok(())
}
