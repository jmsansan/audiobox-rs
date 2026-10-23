//! A synthetic pulse bed ducked by a rhythmic key, with a filter sweep, delay and
//! reverb. Writes WAV without opening a device. No recorded samples are used.
use audiobox::mixer::effects::{Compressor, Delay, Effect, EffectTime, Reverb};
use audiobox::mixer::{BusId, Fade, MasterLimiter, Mixer, MixerConfig, Play};
use audiobox::{Audio, EncodeOptions, Result, write_audio_file};

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "bus-effects.wav".into());
    let rate = 48_000u32;
    let (mut mixer, mut handle) = Mixer::new(MixerConfig {
        sample_rate: rate,
        buses: vec!["pad".into(), "key".into()],
        limiter: Some(MasterLimiter::default()),
        ..Default::default()
    })?;
    let pad = handle.bus("pad").unwrap();
    let key = handle.bus("key").unwrap();
    handle.set_effects(
        pad,
        &[
            Effect::LowPass { cutoff_hz: 7000.0 },
            Effect::Compressor(Compressor {
                threshold_db: -24.0,
                release_ms: 200.0,
                ..Compressor::ducking(key)
            }),
            Effect::Delay(Delay {
                time: EffectTime::Beats {
                    beats: 0.5,
                    bpm: 120.0,
                },
                mix: 0.15,
                ..Default::default()
            }),
            Effect::Reverb(Reverb {
                mix: 0.15,
                ..Default::default()
            }),
        ],
    )?;
    handle.set_bus_gain(key, 0.5, Fade::NONE)?;
    // Integer phase pulses keep the authored input deterministic too.
    let bed: Vec<f32> = (0..rate as usize * 6)
        .map(|i| {
            let a = if i % 218 < 109 { 0.1 } else { -0.1 };
            let b = if i % 173 < 86 { 0.07 } else { -0.07 };
            a + b
        })
        .collect();
    handle.play(
        Audio::new(vec![bed], rate)?,
        &Play {
            bus: pad,
            ..Default::default()
        },
    )?;
    let key_frames = rate as usize / 12;
    let tick: Vec<f32> = (0..key_frames)
        .map(|i| {
            let envelope = 1.0 - i as f32 / key_frames as f32;
            if i % 240 < 120 {
                envelope * 0.8
            } else {
                -envelope * 0.8
            }
        })
        .collect();
    let tick = std::sync::Arc::new(Audio::new(vec![tick], rate)?);
    for beat in 0..12 {
        handle.at(beat * rate as u64 / 2).play(
            tick.clone(),
            &Play {
                bus: key,
                ..Default::default()
            },
        )?;
    }
    handle.at(rate as u64).set_effect(
        pad,
        0,
        Effect::LowPass { cutoff_hz: 350.0 },
        Fade::linear(1.5),
    )?;
    handle.at(rate as u64 * 3).set_effect(
        pad,
        0,
        Effect::LowPass { cutoff_hz: 7000.0 },
        Fade::linear(1.5),
    )?;
    handle
        .at(rate as u64 * 8)
        .set_bus_gain(BusId::MASTER, 0.0, Fade::linear(0.5))?;

    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut buffer = [0.0f32; 960];
    for _ in 0..900 {
        mixer.render(&mut buffer);
        for frame in buffer.chunks_exact(2) {
            left.push(frame[0]);
            right.push(frame[1]);
        }
        while handle.poll_event().is_some() {}
    }
    let audio = Audio::new(vec![left, right], rate)?;
    write_audio_file(path, &audio, &EncodeOptions::default())?;
    println!(
        "Rendered 9 seconds at {rate} Hz: filter sweep, sidechain ducking, beat delay and reverb"
    );
    Ok(())
}
