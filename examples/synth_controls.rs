//! Held-note expression/cutoff automation without retriggering the synthesizer.
use audiobox::synth::*;
use audiobox::{Audio, EncodeOptions, Result, write_audio_file};
fn main() -> Result<()> {
    let map = TempoMap::new(
        48000,
        48,
        &[
            TempoChange {
                tick: 0,
                bpm: 123.456,
            },
            TempoChange {
                tick: 192,
                bpm: 98.765,
            },
        ],
    )?;
    let end = map.frame_at(384)?;
    let score = FrameScore {
        instruments: vec![Instrument {
            waveform: Waveform::Sawtooth,
            envelope: Envelope {
                attack_ms: 100,
                release_ms: 250,
                ..Default::default()
            },
            synthwave: Some(Synthwave {
                voices: 1,
                unison: 3,
                detune: 8,
                spread: 70,
                filter: Some(VoiceFilter {
                    cutoff_hz: 400,
                    ..Default::default()
                }),
                cutoff_lfo: Some(Lfo {
                    rate: LfoRate::Beats {
                        numerator: 2,
                        denominator: 1,
                    },
                    depth: 300,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }],
        tracks: vec![vec![FrameNote::new(0, end, Pitch::new(48)?, 0)]],
        end_frame: end,
    };
    let mut synth = Synth::from_frames_stereo(
        &score,
        &map,
        &RenderOptions {
            gain: 0.1,
            ..Default::default()
        },
    )?;
    let target = ControlTarget::Instrument {
        track: 0,
        instrument: 0,
    };
    let ramp = |start, end, p, value, curve| -> Result<ControlEvent> {
        Ok(ControlEvent {
            ramp_frames: map.frame_at(end)? - map.frame_at(start)?,
            curve,
            ..ControlEvent::new(map.frame_at(start)?, target, p, value)
        })
    };
    synth.prepare_controls(
        &[
            ControlEvent::new(0, target, ControlParameter::Expression, 0.25),
            ramp(
                48,
                336,
                ControlParameter::CutoffHz,
                6000.0,
                ControlCurve::Exponential,
            )?,
            ramp(
                48,
                336,
                ControlParameter::Expression,
                0.8,
                ControlCurve::Linear,
            )?,
            ramp(96, 288, ControlParameter::Pan, 0.7, ControlCurve::Linear)?,
        ],
        8,
    )?;
    let mut pcm = vec![0.0; synth.max_frames().unwrap() as usize * 2];
    let frames = synth.fill(&mut pcm);
    let audio = Audio::from_interleaved(&pcm[..frames * 2], 2, map.sample_rate())?;
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/synth-controls.wav".into());
    write_audio_file(&path, &audio, &EncodeOptions::default())?;
    while let Some(report) = synth.poll_control_event() {
        println!(
            "control {} at frame {}: {:?}",
            report.id.0, report.frame, report.status
        );
    }
    println!(
        "{path}: {end} body frames, {} release frames",
        frames as u64 - end
    );
    Ok(())
}
