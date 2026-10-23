//! Named, seeded sound events. Run with `--features events,sfx`.
use audiobox::events::{EventBank, Placement, Sources, Trigger};
use audiobox::mixer::{Mixer, MixerConfig};
use audiobox::sfx::{Effect, Preset};
use audiobox::{Audio, EncodeOptions, write_audio_file};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/sound-events.wav".into());
    let rate = 48_000;
    let (mut mixer, mut handle) = Mixer::new(MixerConfig {
        sample_rate: rate,
        buses: vec!["effects".into()],
        ..Default::default()
    })?;
    let mut sources = Sources::default();
    for (name, seed) in [("step-a", 11), ("step-b", 22), ("step-c", 33)] {
        sources.insert_effect(name, &Effect::preset(Preset::Jump, seed), rate)?;
    }
    let text = "
# The game only needs to know the event name.
event footstep sources=step-a,step-b,step-c choice=shuffle gain=0.2,0.35 pitch=-3,2 bus=effects max-instances=2 min-gap=0.1 policy=oldest
";
    let prepared = EventBank::parse(text)?.prepare(&handle, &sources)?;
    let mut player = prepared.player(123);
    for i in 0..8 {
        player.play_with(
            &mut handle,
            "footstep",
            Trigger {
                frame: Some(i * u64::from(rate) / 3),
                placement: Some(Placement::Position([i as f32 / 4.0 - 1.0, 0.0])),
            },
        )?;
    }
    let mut samples = vec![0.0; rate as usize * 4 * 2];
    for block in samples.chunks_mut(512 * 2) {
        mixer.render(block);
        while handle.poll_event().is_some() {}
    }
    let left = samples.iter().step_by(2).copied().collect();
    let right = samples.iter().skip(1).step_by(2).copied().collect();
    write_audio_file(
        &path,
        &Audio::new(vec![left, right], rate)?,
        &EncodeOptions::default(),
    )?;
    println!("Wrote {path}");
    Ok(())
}
