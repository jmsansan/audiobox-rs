//! Six sections of the same synthetic pad: dry and each colour effect in isolation.
//! Writes WAV without a device; phase pulses use no platform transcendental math.
use audiobox::mixer::effects::{
    Bitcrusher, Chorus, Effect, Flanger, ModulationRate, Phaser, Saturation,
};
use audiobox::mixer::{BusId, Fade, MasterLimiter, Mixer, MixerConfig, Play};
use audiobox::{Audio, EncodeOptions, Result, write_audio_file};
use std::sync::Arc;

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "colour-effects.wav".into());
    let rate = 48_000u32;
    let frames = rate as usize * 3;
    let voice = |offset: usize| {
        (0..frames)
            .map(|n| {
                let pulse = |period: usize, level: f32| {
                    if (n + offset) % period < period / 2 {
                        level
                    } else {
                        -level
                    }
                };
                pulse(218, 0.11) + pulse(173, 0.07) + pulse(145, 0.05)
            })
            .collect()
    };
    let pad = Arc::new(Audio::new(vec![voice(0), voice(23)], rate)?);
    let sections = [
        ("dry", None),
        (
            "chorus",
            Some(Effect::Chorus(Chorus {
                rate: ModulationRate::Beats {
                    beats: 2.0,
                    bpm: 120.0,
                },
                mix: 0.5,
                ..Default::default()
            })),
        ),
        ("phaser", Some(Effect::Phaser(Phaser::default()))),
        ("flanger", Some(Effect::Flanger(Flanger::default()))),
        (
            "bitcrusher",
            Some(Effect::Bitcrusher(Bitcrusher {
                bits: 5.0,
                rate_ratio: 0.2,
                mix: 1.0,
            })),
        ),
        (
            "saturation",
            Some(Effect::Saturation(Saturation {
                drive_db: 18.0,
                output_db: -6.0,
                mix: 1.0,
            })),
        ),
    ];
    let mut left = Vec::with_capacity(frames * sections.len());
    let mut right = Vec::with_capacity(frames * sections.len());
    let mut out = vec![0.0; frames * 2];
    for (index, (name, effect)) in sections.into_iter().enumerate() {
        let (mut mixer, mut handle) = Mixer::new(MixerConfig {
            sample_rate: rate,
            limiter: Some(MasterLimiter::default()),
            ..Default::default()
        })?;
        if let Some(effect) = effect {
            handle.set_effects(BusId::MASTER, &[effect])?;
        }
        handle.play(
            pad.clone(),
            &Play {
                fade_in: Fade::linear(0.05),
                ..Default::default()
            },
        )?;
        handle.at(frames as u64 - rate as u64 / 10).set_bus_gain(
            BusId::MASTER,
            0.0,
            Fade::linear(0.1),
        )?;
        mixer.render(&mut out);
        for pair in out.chunks_exact(2) {
            left.push(pair[0]);
            right.push(pair[1]);
        }
        while handle.poll_event().is_some() {}
        println!("{}..{} seconds: {name}", index * 3, (index + 1) * 3);
    }
    write_audio_file(
        path,
        &Audio::new(vec![left, right], rate)?,
        &EncodeOptions::default(),
    )?;
    Ok(())
}
