//! An orbit then a moving-source flyby, in each surround layout and its stereo fold.
use audiobox::mixer::*;
use audiobox::{Audio, EncodeOptions, Result, write_audio_file};
use std::path::PathBuf;
use std::sync::Arc;
const RATE: u32 = 48000;
fn main() -> Result<()> {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/spatial-audio".into()),
    );
    std::fs::create_dir_all(&directory)?;
    let tone = (0..RATE)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / RATE as f64;
            (0.25 * (220.0 * t).sin() + 0.08 * (440.0 * t).sin()) as f32
        })
        .collect();
    let source = Arc::new(Audio::new(vec![tone], RATE)?);
    for (name, layout) in [
        ("quad", OutputLayout::Quad),
        ("5.1", OutputLayout::Surround51),
        ("7.1", OutputLayout::Surround71),
    ] {
        let (mut mixer, mut control) = Mixer::new(MixerConfig {
            channels: layout.channels(),
            limiter: Some(MasterLimiter {
                ceiling_db: -3.0,
                lookahead_ms: 2.0,
                ..Default::default()
            }),
            ..Default::default()
        })?;
        let attenuation = Attenuation {
            min_distance: 5.0,
            max_distance: 100.0,
            ..Default::default()
        };
        let id = control.play(
            source.clone(),
            &Play {
                looping: Loop::Whole,
                emitter: Some(Emitter {
                    position: [0.0, 0.0, -15.0],
                    attenuation,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )?;
        for step in 1..160 {
            let time = step as f64 * 0.05;
            let (position, velocity) = if time < 4.0 {
                let angle = time * std::f64::consts::TAU / 4.0;
                let speed = 15.0 * std::f64::consts::TAU / 4.0;
                (
                    [15.0 * angle.sin() as f32, 0.0, -15.0 * angle.cos() as f32],
                    [
                        speed as f32 * angle.cos() as f32,
                        0.0,
                        speed as f32 * angle.sin() as f32,
                    ],
                )
            } else {
                (
                    [-40.0 + 20.0 * (time - 4.0) as f32, 2.0, -10.0],
                    [20.0, 0.0, 0.0],
                )
            };
            control
                .at((time * RATE as f64).round() as u64)
                .set_emitter(
                    id,
                    Emitter {
                        position,
                        velocity,
                        attenuation,
                    },
                    Fade::linear(0.05),
                )?;
        }
        control.at(8 * RATE as u64).stop(id, Fade::linear(0.03))?;
        let ch = layout.channels();
        let mut samples = vec![0.0; ((RATE as usize * 81) / 10) * ch];
        for part in samples.chunks_mut(137 * ch) {
            mixer.render(part);
        }
        let data = (0..ch)
            .map(|c| samples.iter().skip(c).step_by(ch).copied().collect())
            .collect();
        let audio = Audio::new(data, RATE)?;
        write_audio_file(
            directory.join(format!("{name}.wav")),
            &audio,
            &EncodeOptions {
                wav_channel_mask: Some(layout.channel_mask()),
                ..Default::default()
            },
        )?;
        let mut stereo = vec![0.0; samples.len() / ch * 2];
        layout.downmix_stereo(&samples, &mut stereo)?;
        let audio = Audio::new(
            (0..2)
                .map(|c| stereo.iter().skip(c).step_by(2).copied().collect())
                .collect(),
            RATE,
        )?;
        // The source has headroom for the stereo fold; a general mix should be
        // limited after folding if a guaranteed stereo ceiling is required.
        write_audio_file(
            directory.join(format!("{name}-stereo.wav")),
            &audio,
            &EncodeOptions::default(),
        )?;
        println!(
            "{name}: {} frames, {} channels, mask {:#x}",
            samples.len() / ch,
            ch,
            layout.channel_mask()
        );
    }
    Ok(())
}
