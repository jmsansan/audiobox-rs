//! Writes explicit vocal-note fixtures: a robot melody and a three-voice vowel
//! harmony, each voice with its own seed, aligned to exact frames.
//!
//! `cargo run --release --example voice_music --features voice [OUT_DIR]`
use audiobox::synth::{Duty, Waveform};
use audiobox::voice::{
    Onset, PreparedUtterance, VocalNote, VoiceOptions, VoicePreset, VoiceProfile, Vowel,
};
use audiobox::{Audio, AudioFormat, encode};
use std::path::{Path, PathBuf};

const RATE: u32 = 48_000;

fn hz(midi: f64) -> f64 {
    440.0 * 2f64.powf((midi - 69.0) / 12.0)
}

fn main() -> audiobox::Result<()> {
    let dir = std::env::args()
        .nth(1)
        .map_or_else(|| Path::new("target").join("voices"), PathBuf::from);
    std::fs::create_dir_all(&dir)?;
    let beat = RATE as u64 / 2; // 120 BPM quarter notes.

    // A robot melody: steps, a legato run and a final glide.
    let robot = VoiceProfile::robot(Waveform::Pulse(Duty::Eighth));
    let tune = [
        (72.0, 1, false),
        (74.0, 1, false),
        (76.0, 1, true),
        (79.0, 1, true),
        (76.0, 2, false),
        (72.0, 2, false),
    ];
    let mut notes = Vec::new();
    let mut at = 0;
    for (midi, beats, legato) in tune {
        let length = beats * beat;
        let mut note = VocalNote::new(at, length - beat / 8, hz(midi), Vowel::A);
        if legato {
            note.frames = length;
            note = note.legato();
        }
        notes.push(note);
        at += length;
    }
    if let Some(last) = notes.last_mut() {
        *last = last.glide_to(hz(67.0));
    }
    let options = VoiceOptions {
        sample_rate: RATE,
        seed: 1,
        ..VoiceOptions::default()
    };
    let melody = PreparedUtterance::from_notes(&robot, &notes, &options)?.render()?;
    std::fs::write(
        dir.join("music-robot-melody.wav"),
        encode(&melody, AudioFormat::Wav)?,
    )?;

    // A vowel harmony: three independent voices, sustained chords with vowel
    // changes on tied (legato) notes.
    let chords = [
        [60.0, 64.0, 67.0],
        [60.0, 65.0, 69.0],
        [59.0, 62.0, 67.0],
        [60.0, 64.0, 67.0],
    ];
    let vowels = [Vowel::A, Vowel::O, Vowel::E, Vowel::A];
    let mut mix: Option<Audio> = None;
    for part in 0..3 {
        let profile = VoiceProfile::preset(VoicePreset::SoftRounded);
        let notes: Vec<VocalNote> = chords
            .iter()
            .zip(vowels)
            .enumerate()
            .map(|(i, (chord, vowel))| {
                let note = VocalNote::new(
                    i as u64 * 4 * beat,
                    4 * beat,
                    hz(chord[part] - 12.0 * (part == 0) as u8 as f64),
                    vowel,
                )
                .with_velocity(0.7);
                if i == 0 {
                    note.with_onset(Onset::Breath)
                } else {
                    note.legato()
                }
            })
            .collect();
        let voice = PreparedUtterance::from_notes(
            &profile,
            &notes,
            &VoiceOptions {
                sample_rate: RATE,
                seed: 100 + part as u64,
                ..VoiceOptions::default()
            },
        )?
        .render()?;
        mix = Some(match mix {
            None => voice,
            Some(m) => m.mix(&voice, 0, 1.0, true)?,
        });
    }
    let harmony = mix.expect("three voices").gain(0.6)?;
    std::fs::write(
        dir.join("music-vowel-harmony.wav"),
        encode(&harmony, AudioFormat::Wav)?,
    )?;
    println!(
        "Wrote {} ({} and {} frames)",
        dir.display(),
        melody.frames(),
        harmony.frames()
    );
    Ok(())
}
