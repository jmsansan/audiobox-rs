//! Plays generated dialogue through the mixer and follows its markers.
//!
//! `cargo run --example voice_dialogue --features voice,mixer,events`
//!
//! The line is planned and prepared on the game's thread. The mixer streams it
//! without allocating; the game reads the voice's *source position* from
//! `MixerHandle::voice_status` and looks up the syllable under it, which stays
//! correct under pause, rate and pitch changes because it follows the source, not
//! the wall clock. This program renders the mixer offline in callback-sized
//! blocks; a real game hands the same `render` calls to its audio device, which
//! audiobox does not provide.
use audiobox::events::{EventBank, Sources};
use audiobox::mixer::{Fade, Mixer, MixerConfig, Play, VoiceState};
use audiobox::voice::{
    GibberishOptions, MarkerKind, PreparedUtterance, VoiceOptions, VoicePreset, VoiceProfile,
    plan_gibberish,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let text = "¡Hola! ¿Traes el mapa, viajera?";
    let profile = VoiceProfile::preset(VoicePreset::SmallCreature);
    let plan = plan_gibberish(text, &profile, &GibberishOptions::default())?;
    let voice = PreparedUtterance::new(
        &profile,
        &plan,
        &VoiceOptions {
            sample_rate: 48_000,
            seed: 3,
            ..VoiceOptions::default()
        },
    )?;

    println!("Marker timeline (source frames at 48 kHz):");
    for m in voice.markers() {
        println!(
            "  {:<8?} {:>6}..{:<6} {:?}",
            m.kind,
            m.frames.start,
            m.frames.end,
            &text[m.bytes.clone()]
        );
    }

    let (mut mixer, mut handle) = Mixer::new(MixerConfig::default())?;
    let id = handle.play(&voice, &Play::default())?;
    let mut block = vec![0.0f32; 512 * mixer.channels()];
    let mut last = None;
    let mut rendered = 0u64;
    println!("Following the mixer voice:");
    loop {
        mixer.render(&mut block);
        rendered += 512;
        while handle.poll_event().is_some() {}
        let Some(status) = handle.voice_status(id) else {
            break;
        };
        if status.state == VoiceState::Finished {
            break;
        }
        let syllable = voice.marker_at(MarkerKind::Syllable, status.position);
        let span = syllable.map(|m| m.bytes.clone());
        if span != last {
            match &span {
                Some(bytes) => println!(
                    "  output {rendered:>6}: source {:>6} says {:?}",
                    status.position,
                    &text[bytes.clone()]
                ),
                None => println!(
                    "  output {rendered:>6}: source {:>6} (between syllables)",
                    status.position
                ),
            }
            last = span;
        }
        if rendered == 512 * 40 {
            // Pausing holds the source position, so the markers hold too.
            handle.pause(id, Fade::NONE)?;
            for _ in 0..20 {
                mixer.render(&mut block);
            }
            handle.resume(id, Fade::NONE)?;
        }
    }

    // Designers can name the same line as a sound event; the bank never plans text.
    let mut sources = Sources::default();
    sources.insert_voice("greeting", &voice)?;
    let bank = EventBank::parse("event npc.greeting sources=greeting gain=0.9")?;
    let mut player = bank.prepare(&handle, &sources)?.player(7);
    player.play(&mut handle, "npc.greeting")?;
    mixer.render(&mut block);
    println!("Started npc.greeting through the event bank.");
    Ok(())
}
