//! Fractional tempo, an authored tempo change and a held note across it.
use audiobox::synth::*;
use audiobox::{Audio, EncodeOptions, Result, write_audio_file};

fn main() -> Result<()> {
    let rate = 48000;
    let map = TempoMap::new(
        rate,
        48,
        &[
            TempoChange {
                tick: 0,
                bpm: 126.375,
            },
            TempoChange {
                tick: 4 * 48,
                bpm: 111.125,
            },
        ],
    )?;
    let end = map.frame_at(8 * 48)?;
    let mut melody = Vec::new();
    for (quarter, midi) in [60, 64, 67, 72, 71, 67, 64, 60].into_iter().enumerate() {
        melody.push(FrameNote::new(
            map.frame_at(quarter as u64 * 48)?,
            map.frame_at(quarter as u64 * 48 + 36)?,
            Pitch::new(midi)?,
            0,
        ));
    }
    let score = FrameScore {
        instruments: vec![Instrument {
            waveform: Waveform::Sawtooth,
            envelope: Envelope {
                attack_ms: 10,
                release_ms: 180,
                ..Default::default()
            },
            synthwave: Some(Synthwave {
                voices: 4,
                unison: 3,
                detune: 8,
                spread: 60,
                amplitude_lfo: Some(Lfo {
                    rate: LfoRate::Beats {
                        numerator: 2,
                        denominator: 1,
                    },
                    depth: 30,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }],
        tracks: vec![melody, vec![FrameNote::new(0, end, Pitch::new(48)?, 0)]],
        end_frame: end,
    };
    let mut synth = Synth::from_frames_stereo(
        &score,
        &map,
        &RenderOptions {
            gain: 0.15,
            ..Default::default()
        },
    )?;
    let mut samples = vec![0.0; synth.max_frames().unwrap() as usize * 2];
    let frames = synth.fill(&mut samples);
    let audio = Audio::from_interleaved(&samples[..frames * 2], 2, rate)?;
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/prepared-synth.wav".into());
    write_audio_file(&path, &audio, &EncodeOptions::default())?;
    println!(
        "{path}: {end} body frames, {} release frames, {rate} Hz",
        frames as u64 - end
    );
    Ok(())
}
