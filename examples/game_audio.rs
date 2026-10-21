//! A game's audio with the mixer: looping chiptune music on a music bus, sound
//! effects on an effects bus, and the mixer rendering on its own thread. Without an
//! audio device, the rendered buffers are written to a WAV file.
//!
//! ```sh
//! cargo run --example game_audio --features mixer,sfx -- game.wav
//! ```
use std::sync::Arc;
use std::thread;

use audiobox::mixer::{Fade, Mixer, MixerConfig, MixerEvent, Play, Variation};
use audiobox::sfx::{Effect, Preset};
use audiobox::synth::{RenderOptions, Score, Synth};
use audiobox::{Audio, EncodeOptions, write_audio_file};

const RATE: u32 = 48_000;
const SONG: &str = "
tempo 150
instrument lead wave=pulse duty=25 adsr=4,80,60,60 vibrato=12,170,200
instrument bass wave=triangle adsr=2,0,100,25
channel lead = @lead o5 l8 q7 L [c e g e >c< g e g]2 [d f a f >d< a f a]2
channel bass = @bass o2 l4 L [c c g g]2 [d d a a]2
";

fn main() -> audiobox::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "game.wav".into());
    let (mut mixer, mut audio) = Mixer::new(MixerConfig {
        sample_rate: RATE,
        channels: 2,
        max_voices: 16,
        buses: vec!["music".into(), "effects".into()],
        ..MixerConfig::default()
    })?;
    let music = audio.bus("music").unwrap();
    let effects = audio.bus("effects").unwrap();

    // Loading: effects are rendered once and shared by every voice that plays them.
    let jump = Arc::new(Effect::preset(Preset::Jump, 7).render(RATE)?);
    let coin = Arc::new(Effect::preset(Preset::Pickup, 3).render(RATE)?);
    let hit = Arc::new(Effect::preset(Preset::Hit, 2).render(RATE)?);
    let score: Score = SONG.parse()?;
    let options = RenderOptions {
        loops: None,
        ..RenderOptions::default()
    };
    let song = Synth::new(&score, RATE, &options)?;

    // The level starts: music fades in at high priority so effects never take its place.
    audio.play(
        song,
        &Play {
            bus: music,
            priority: 255,
            fade_in: Fade::linear(1.0),
            ..Play::default()
        },
    )?;
    // Game events, scheduled at exact frames as the game loop would send them.
    let second = RATE as u64;
    let at = |seconds: f64| (seconds * second as f64) as u64;
    for (time, sound, pan) in [
        (1.0, &jump, 0.0),
        (1.6, &coin, 0.4),
        (2.2, &coin, 0.5),
        (2.8, &jump, -0.2),
        (3.5, &hit, -0.6),
    ] {
        audio.at(at(time)).play(
            sound.clone(),
            &Play {
                bus: effects,
                pan,
                ..Play::default()
            },
        )?;
    }
    // Footsteps: one sound, varied in pitch and level on every step.
    let footstep = Arc::new(Effect::preset(Preset::Hit, 11).render(RATE)?);
    let step = Play {
        bus: effects,
        gain: 0.6,
        variation: Variation {
            pitch: 1.5,
            gain_db: 2.0,
        },
        ..Play::default()
    };
    for i in 0..8 {
        audio
            .at(at(0.4 + i as f64 * 0.3))
            .play(footstep.clone(), &step)?;
    }
    // A pause menu ducks the music, then the level ends with a fade-out.
    audio
        .at(at(4.0))
        .set_bus_gain(music, 0.3, Fade::linear(0.2))?;
    audio
        .at(at(5.0))
        .set_bus_gain(music, 1.0, Fade::linear(0.2))?;
    audio.at(at(6.0)).stop_all(Fade::linear(1.0))?;

    // The audio thread: an audio callback would call `render` for each device buffer.
    let device = thread::spawn(move || {
        let mut recording = Vec::new();
        let mut buffer = vec![0.0f32; 2 * 512];
        while mixer.frame() < 7 * second {
            mixer.render(&mut buffer);
            recording.extend_from_slice(&buffer);
        }
        recording
    });
    let recording = device.join().expect("audio thread");

    // Once per game frame, the game polls events, which also frees finished voices.
    while let Some(event) = audio.poll_event() {
        if let MixerEvent::Stolen(voice) | MixerEvent::Rejected(voice) = event {
            println!("voice {voice:?} gave way to another");
        }
    }
    let output = Audio::from_interleaved(&recording, 2, RATE)?;
    write_audio_file(&path, &output, &EncodeOptions::default())?;
    println!("Wrote {path}: {:.1} s", output.duration());
    Ok(())
}
