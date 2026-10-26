//! Two tracks, one group insert and one shared wet delay return.
use audiobox::mixer::effects::{Delay, Effect, EffectTime};
use audiobox::mixer::*;
use audiobox::{Audio, EncodeOptions, Result, write_audio_file};

fn main() -> Result<()> {
    let rate = 48000;
    let (mut mixer, mut handle) = Mixer::new(MixerConfig {
        sample_rate: rate,
        channels: 2,
        buses: vec!["lead".into(), "bass".into(), "group".into(), "echo".into()],
        ..Default::default()
    })?;
    let lead = handle.bus("lead").unwrap();
    let bass = handle.bus("bass").unwrap();
    let group = handle.bus("group").unwrap();
    let echo = handle.bus("echo").unwrap();
    let edit = handle.set_routing(&[
        BusRouting {
            output: Some(group),
            sends: vec![BusSend {
                destination: echo,
                gain: 0.35,
                position: SendPosition::PreFader,
            }],
            pan: -0.3,
            ..BusRouting::new(lead)
        },
        BusRouting {
            output: Some(group),
            sends: vec![BusSend {
                destination: echo,
                gain: 0.15,
                position: SendPosition::PostFader,
            }],
            pan: 0.3,
            ..BusRouting::new(bass)
        },
    ])?;
    handle.set_effects(group, &[Effect::LowPass { cutoff_hz: 4000.0 }])?;
    handle.set_effects(
        echo,
        &[Effect::Delay(Delay {
            time: EffectTime::Seconds(0.25),
            max_seconds: 0.5,
            feedback: 0.3,
            mix: 1.0,
        })],
    )?;
    handle.set_bus_gain(group, 0.7, Fade::NONE)?;
    for (bus, period) in [(lead, 180), (bass, 580)] {
        let data: Vec<_> = (0..rate as usize * 2)
            .map(|i| {
                let envelope = 1.0 - (i % (rate as usize / 4)) as f32 / (rate as f32 / 4.0);
                (if i % period < period / 2 { 0.1 } else { -0.1 }) * envelope
            })
            .collect();
        handle.play(
            Audio::new(vec![data.clone(), data], rate)?,
            &Play {
                bus,
                ..Default::default()
            },
        )?;
    }
    let mut samples = vec![0.0; rate as usize * 4 * 2];
    mixer.render(&mut samples);
    let audio = Audio::from_interleaved(&samples, 2, rate)?;
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/group-sends.wav".into());
    write_audio_file(&path, &audio, &EncodeOptions::default())?;
    let mut applied = false;
    while let Some(event) = handle.poll_event() {
        if event == MixerEvent::RoutingApplied(edit, 0) {
            applied = true;
        }
    }
    println!(
        "{path}: {} stereo frames at {rate} Hz; routing edit {} applied: {applied}",
        audio.frames(),
        edit.0
    );
    Ok(())
}
