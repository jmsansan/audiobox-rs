//! Synthetic stems, a beat-aligned cue and a bar-aligned transition to a recording.
//! No audio device: writes an eight-second WAV and reports application frames.
//! cargo run --example adaptive_music --features music,synth -- music.wav
use audiobox::mixer::{BusId, Fade, Mixer, MixerConfig, MixerEvent};
use audiobox::music::{Layer, Music, Quantize, TimeSignature, Timing, Track, Transport};
use audiobox::synth::{RenderOptions, Score, Synth};
use audiobox::{
    Audio, AudioFormat, EncodeOptions, FadeCurve, encode, stream::Decoder, write_audio_file,
};

const RATE: u32 = 48_000;
fn score(channel: &str) -> audiobox::Result<Score> {
    format!("tempo 120\ninstrument p wave=pulse duty=25 adsr=4,50,60,40\ninstrument b wave=triangle adsr=4,0,90,40\nchannel part = {channel}").parse()
}

fn main() -> audiobox::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "music.wav".into());
    let (mut mixer, mut handle) = Mixer::new(MixerConfig {
        sample_rate: RATE,
        channels: 2,
        max_voices: 16,
        ..Default::default()
    })?;
    let transport = Transport::new(RATE, 120.0, TimeSignature::default(), 0)?;
    let music = Music::new(&mut handle, transport, BusId::MASTER)?;
    let looping = RenderOptions {
        loops: None,
        ..Default::default()
    };
    let bass = Synth::new(&score("@b o3 l4 L [c g c g]2")?, RATE, &looping)?;
    let melody = Synth::new(&score("@p o5 l8 L [c e g e >c< g e g]2")?, RATE, &looping)?;
    let track = Track {
        layers: vec![
            Layer {
                gain: 0.3,
                ..Layer::new(bass)
            },
            Layer {
                gain: 0.2,
                intensity: [0.2, 0.8],
                ..Layer::new(melody)
            },
        ],
        looping: Default::default(),
    };
    music.transition(&mut handle, track, 0.0, Timing::NOW, Fade::linear(0.1))?;
    music.set_intensity(
        &mut handle,
        0.8,
        Timing {
            not_before: RATE as u64 * 2,
            quantize: Quantize::Beat,
        },
        Fade::linear(0.25),
    )?;
    let cue = Synth::new(
        &score("@p o6 l16 c e g >c")?,
        RATE,
        &RenderOptions::default(),
    )?;
    music.cue(
        &mut handle,
        cue,
        0.3,
        Timing {
            not_before: RATE as u64,
            quantize: Quantize::Beat,
        },
    )?;
    // This recording is synthetic demo material, encoded then streamed as FLAC.
    let recording = score("@b o3 l4 [f c f c]2")?.render(RATE)?;
    let bytes = encode(&recording, AudioFormat::Flac)?;
    let next = Track {
        layers: vec![Layer {
            gain: 0.3,
            ..Layer::new(Decoder::new(bytes)?)
        }],
        looping: audiobox::mixer::Loop::Whole,
    };
    music.transition(
        &mut handle,
        next,
        0.0,
        Timing {
            not_before: RATE as u64 * 3,
            quantize: Quantize::Bar,
        },
        Fade::new(0.5, FadeCurve::EqualPower),
    )?;
    music.stop(
        &mut handle,
        Timing {
            not_before: RATE as u64 * 7,
            quantize: Quantize::Immediate,
        },
        Fade::linear(0.5),
    )?;
    let mut samples = vec![0.0; RATE as usize * 8 * 2];
    for chunk in samples.chunks_mut(2 * 512) {
        mixer.render(chunk);
        while let Some(event) = handle.poll_event() {
            match event {
                MixerEvent::MusicApplied(id, frame) => {
                    println!("{id:?} applied at output frame {frame}")
                }
                MixerEvent::MusicRejected(id, code) => eprintln!("{id:?} rejected: {code:?}"),
                _ => {}
            }
        }
    }
    write_audio_file(
        &path,
        &Audio::from_interleaved(&samples, 2, RATE)?,
        &EncodeOptions::default(),
    )?;
    println!(
        "Wrote {path}. A host supplies the audio device; the game polls events on its control thread."
    );
    Ok(())
}
