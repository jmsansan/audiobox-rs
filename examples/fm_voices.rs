//! Four FM patches in a short piece, rendered in stereo. Run with `--features synth`.
//!
//! The patches are starting points written for this example: an electric piano from
//! three operator pairs, an inharmonic bell, a brass whose modulator rises after the
//! carrier, and a bass that layers two stacks an octave apart.
use audiobox::synth::{RenderOptions, Score};
use audiobox::{EncodeOptions, write_audio_file};

const SONG: &str = "tempo 96
; Three pairs (algorithm 5): a short bright tine over two detuned bodies.
fm piano algorithm=5 feedback=5 voices=6 volume=9
  op 1 ratio=1            rates=96,25,25,67 levels=99,75,0,0
  op 2 ratio=14 level=58  rates=95,50,35,78 levels=99,75,0,0
  op 3 ratio=1 detune=-5  rates=95,20,20,50 levels=99,95,0,0
  op 4 ratio=1 level=82   rates=95,29,20,50 levels=99,95,0,0
  op 5 ratio=1 detune=5   rates=95,20,20,50 levels=99,95,0,0
  op 6 ratio=1 level=76   rates=95,29,20,50 levels=99,95,0,0
end
; A 3.5 ratio gives inharmonic partials; the modulator dies first, leaving a sine.
fm bell algorithm=1 voices=4 volume=8 pan=35
  op 1 ratio=1               rates=95,28,25,40 levels=99,0,0,0
  op 2 ratio=3,50 level=80   rates=95,38,30,45 levels=99,0,0,0
end
; Feedback on the modulator (algorithm 2), which swells in after the carrier.
fm brass algorithm=2 feedback=6 voices=3 volume=8 pan=-30 unison=2 unison-detune=6 spread=40
  op 1 ratio=1            rates=62,40,30,55 levels=99,92,88,0
  op 2 ratio=1 level=77   rates=46,45,30,55 levels=99,85,80,0
end
; Two stacks (algorithm 3): 3>2>1 for the bite, 6>5>4 an octave below for the body.
fm bass algorithm=3 feedback=4 voices=1 glide=35 legato=on volume=11
  op 1 ratio=1            rates=99,45,35,65 levels=99,80,60,0
  op 2 ratio=1 level=80   rates=99,55,40,65 levels=99,60,40,0
  op 3 ratio=2 level=68   rates=99,60,45,65 levels=99,40,0,0
  op 4 ratio=0.5          rates=99,40,30,65 levels=99,90,80,0
  op 5 ratio=0.5 level=72 rates=99,50,35,65 levels=99,70,50,0
  op 6 ratio=1 level=60   rates=99,60,40,65 levels=99,50,0,0
end
channel piano=@piano o4 l2 [{c e g b}]2 [{a >c< e g}]2 [{f a >c e<}]2 [{g b >d< f}]2
channel bell=@bell o6 l8 r4 e g b4 r8 r4 c e a4 r8 r4 c f a4 r8 r4 d g b4 r8
channel brass=@brass o4 l1 r r {a >c<} {b >d<}
channel bass=@bass o2 l8 [c c >c< c]2 <[a a >a< a]2 [f f >f< f]2 [g g >g< g]2";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/fm_voices.wav".into());
    let score = Score::parse(SONG)?;
    let audio = score.render_stereo_with(
        48000,
        &RenderOptions {
            gain: 0.2,
            ..Default::default()
        },
    )?;
    write_audio_file(&path, &audio, &EncodeOptions::default())?;
    println!("Wrote {path}: {} stereo frames", audio.frames());
    Ok(())
}
