//! One physical voice, three retained clocks; the distant source returns in sync.
//! cargo run --example virtual_voices --features mixer -- target/virtual-voices.wav
use audiobox::mixer::*;
use audiobox::*;
use std::sync::Arc;

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "virtual-voices.wav".into());
    let rate = 48_000;
    let source_rate = 24_000;
    // Integer-phase triangle: repeatable without host trigonometry.
    let tone = Arc::new(Audio::new(
        vec![
            (0..source_rate)
                .map(|i| {
                    let phase = (i % 120) as f32 / 120.0;
                    (1.0 - 4.0 * (phase - 0.5).abs()) * 0.2
                })
                .collect(),
        ],
        source_rate,
    )?);
    let (mut mixer, mut handle) = Mixer::new(MixerConfig {
        max_voices: 1,
        virtual_voices: Some(VirtualVoices {
            capacity: 3,
            ..Default::default()
        }),
        ..Default::default()
    })?;
    let ordinary = handle.play(
        tone.clone(),
        &Play {
            looping: Loop::Whole,
            ..Default::default()
        },
    )?;
    let mut emitter = Emitter {
        position: [0.0, 0.0, -200.0],
        attenuation: Attenuation {
            max_distance: 20.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let distant = handle.play(
        tone,
        &Play {
            emitter: Some(emitter),
            priority: 200,
            looping: Loop::Whole,
            resampling: ResamplingQuality::Linear,
            ..Default::default()
        },
    )?;
    emitter.position = [0.0, 0.0, -1.0];
    handle
        .at(rate as u64)
        .set_emitter(distant, emitter, Fade::linear(0.02))?;
    handle
        .at(2 * rate as u64)
        .set_gain(distant, 0.0, Fade::linear(0.02))?;
    let mut output = vec![0.0; 3 * rate as usize * 2];
    for block in output.chunks_mut(256 * 2) {
        mixer.render(block);
        while handle.poll_event().is_some() {}
    }
    eprintln!(
        "physical={}, virtual={}, clocks={:?}/{:?}",
        mixer.physical_voices(),
        mixer.virtual_voice_count(),
        handle.voice_status(ordinary),
        handle.voice_status(distant)
    );
    let audio = Audio::from_interleaved(&output, 2, rate)?;
    write_audio_file(&path, &audio, &EncodeOptions::default())?;
    Ok(())
}
