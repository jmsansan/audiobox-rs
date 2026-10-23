//! Bass, pad and lead rendered in stereo. Run with `--features synth`.
use audiobox::synth::{RenderOptions, Score};
use audiobox::{EncodeOptions, write_audio_file};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/synthwave.wav".into());
    let score=Score::parse("tempo 110
instrument bass wave=saw voices=1 glide=80 legato=on adsr=5,120,60,40 filter=lowpass cutoff=500 resonance=160 filter-env=2400 filter-adsr=0,200,0,40
instrument pad wave=saw unison=5 detune=18 spread=90 voices=6 adsr=120,150,70,300 filter=lowpass cutoff=1800 resonance=90 keytrack=40 lfo-cutoff=beats,4,1,600
instrument lead wave=pulse voices=1 glide=60 legato=on adsr=5,80,80,100 lfo-pitch=5000,12 lfo-pan=beats,2,1,30
channel bass=@bass o2 l8 [c c >c< g]4
channel pad=@pad o4 {c e g}1 {f a >c}1
channel lead=@lead o5 l8 [c e g >c< g e d g]2")?;
    let audio = score.render_stereo_with(
        48000,
        &RenderOptions {
            gain: 0.12,
            ..Default::default()
        },
    )?;
    write_audio_file(&path, &audio, &EncodeOptions::default())?;
    println!("Wrote {path}: {} stereo frames", audio.frames());
    Ok(())
}
