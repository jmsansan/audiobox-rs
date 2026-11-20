//! Writes the procedural voice listening fixtures and a report.
//!
//! `cargo run --release --example voices --features voice [OUT_DIR]` writes WAVs to
//! `target/voices` by default: five held vowels, the robot waveforms with question
//! and statement endings, the five characters speaking the same line, the same
//! line rigid and expressive for one character, smooth and saw sources on the same
//! vowels, and an articulated phrase. Each sound has a raw file and a copy scaled
//! to RMS 0.05 (`-rms.wav`) for level-matched comparison, a `.voice` file with the
//! exact expanded profile, and a line in `report.csv` with its seed, rate, length,
//! peak, RMS, DC and the applied matching gain. The output is unintelligible
//! procedural sound by design.
use audiobox::synth::{Duty, NoiseMode, Waveform};
use audiobox::voice::{
    Articulation, Delivery, GibberishOptions, PreparedUtterance, UtterancePlan, VocalEvent,
    VoiceMode, VoiceOptions, VoicePreset, VoiceProfile, VoiceSource, Vowel, plan_gibberish,
};
use audiobox::{Audio, AudioFormat, encode};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const LINE: &str = "¿Qué tal, viajero? ¡Vamos!";
const RATE: u32 = 48_000;
const SEED: u64 = 7;
const TARGET_RMS: f64 = 0.05;

struct Out {
    dir: PathBuf,
    report: String,
}
impl Out {
    fn write(&mut self, name: &str, voice: &PreparedUtterance) -> audiobox::Result<()> {
        let audio = voice.render()?;
        self.save(name, &audio, voice.profile(), voice.seed())
    }
    fn save(
        &mut self,
        name: &str,
        audio: &Audio,
        profile: &VoiceProfile,
        seed: u64,
    ) -> audiobox::Result<()> {
        let samples = audio.channel_data(0).unwrap_or(&[]);
        let n = samples.len().max(1) as f64;
        let rms = (samples.iter().map(|&s| s as f64 * s as f64).sum::<f64>() / n).sqrt();
        let dc = samples.iter().map(|&s| s as f64).sum::<f64>() / n;
        let peak = samples.iter().fold(0.0f64, |m, &s| m.max((s as f64).abs()));
        let gain = if rms > 0.0 { TARGET_RMS / rms } else { 1.0 };
        let path = |suffix: &str| self.dir.join(format!("{name}{suffix}"));
        std::fs::write(path(".wav"), encode(audio, AudioFormat::Wav)?)?;
        std::fs::write(
            path("-rms.wav"),
            encode(&audio.gain(gain)?, AudioFormat::Wav)?,
        )?;
        std::fs::write(path(".voice"), profile.to_string())?;
        let _ = writeln!(
            self.report,
            "{name},{seed},{},{},{peak:.4},{rms:.4},{dc:.6},{gain:.3}",
            audio.sample_rate(),
            audio.frames()
        );
        println!(
            "{name:<34} {:>6} ms  peak {peak:.3}  rms {rms:.4}  dc {dc:+.5}",
            audio.frames() * 1000 / audio.sample_rate() as usize
        );
        Ok(())
    }
}

fn options() -> VoiceOptions {
    VoiceOptions {
        sample_rate: RATE,
        seed: SEED,
        ..VoiceOptions::default()
    }
}

fn main() -> audiobox::Result<()> {
    let dir = std::env::args()
        .nth(1)
        .map_or_else(|| Path::new("target").join("voices"), PathBuf::from);
    std::fs::create_dir_all(&dir)?;
    let mut out = Out {
        dir,
        report: "name,seed,rate,frames,peak,rms,dc,rms_gain\n".into(),
    };

    // Five held vowels, smooth and saw sources at the same pitch and timing.
    for (name, source) in [("smooth", VoiceSource::Smooth), ("saw", VoiceSource::Saw)] {
        let profile = VoiceProfile {
            mode: VoiceMode::Formant(source),
            vibrato: 0.0,
            jitter: 0.0,
            shimmer: 0.0,
            ..VoiceProfile::default()
        };
        let mut events = Vec::new();
        for vowel in Vowel::ALL {
            events.push(VocalEvent::vowel(vowel, 600_000, 155.0));
            events.push(VocalEvent::rest(150_000));
        }
        events.pop();
        let voice = PreparedUtterance::new(&profile, &UtterancePlan::new(events), &options())?;
        out.write(&format!("vowels-{name}"), &voice)?;
    }

    // An articulated phrase: breath, fricative and closure gestures between vowels.
    let profile = VoiceProfile::default();
    let g = |a, v, us| VocalEvent::gesture(a, v, us);
    let events = vec![
        g(Articulation::Breath, Vowel::A, 60_000),
        VocalEvent::vowel(Vowel::A, 180_000, 200.0).glide_to(210.0),
        g(Articulation::Fricative, Vowel::I, 90_000),
        VocalEvent::vowel(Vowel::I, 160_000, 215.0),
        g(Articulation::Closure, Vowel::O, 40_000),
        g(Articulation::Fricative, Vowel::O, 15_000),
        VocalEvent::vowel(Vowel::O, 200_000, 205.0).glide_to(180.0),
        VocalEvent::rest(120_000),
        g(Articulation::Breath, Vowel::U, 70_000),
        VocalEvent::vowel(Vowel::U, 260_000, 190.0).glide_to(150.0),
    ];
    let voice = PreparedUtterance::new(&profile, &UtterancePlan::new(events), &options())?;
    out.write("articulated", &voice)?;

    // Robot waveforms, each with a rising question and a falling statement.
    let waves = [
        ("pulse", Waveform::Pulse(Duty::Quarter)),
        ("triangle", Waveform::Triangle),
        ("saw", Waveform::Sawtooth),
        ("noise-short", Waveform::Noise(NoiseMode::Short)),
        ("noise-long", Waveform::Noise(NoiseMode::Long)),
    ];
    for (name, wave) in waves {
        let profile = VoiceProfile::robot(wave);
        for (ending, text) in [
            ("question", "Beep boop bip?"),
            ("statement", "Beep boop bip."),
        ] {
            let plan = plan_gibberish(text, &profile, &GibberishOptions::default())?;
            let voice = PreparedUtterance::new(&profile, &plan, &options())?;
            out.write(&format!("robot-{name}-{ending}"), &voice)?;
        }
    }

    // The five characters speak the same line with the same delivery seed.
    let mut all = Vec::new();
    let mut same = Vec::new();
    for preset in VoicePreset::ALL {
        let profile = VoiceProfile::preset(preset);
        let plan = plan_gibberish(LINE, &profile, &GibberishOptions::default())?;
        let voice = PreparedUtterance::new(&profile, &plan, &options())?;
        out.write(&format!("character-{preset}"), &voice)?;
        all.push(voice.render()?);
        // The same authored pitch for everyone, without pitch modulation, isolates
        // timbre from pitch.
        let fixed = VoiceProfile {
            pitch: 155.0,
            vibrato: 0.0,
            jitter: 0.0,
            ..profile
        };
        let plan = plan_gibberish(LINE, &fixed, &GibberishOptions::default())?;
        same.push(PreparedUtterance::new(&fixed, &plan, &options())?.render()?);
    }
    for (name, list) in [("all-characters", all), ("all-same-pitch", same)] {
        let gap = Audio::new(vec![vec![0.0; RATE as usize / 2]], RATE)?;
        let mut joined = list[0].clone();
        for audio in &list[1..] {
            joined = joined.concat(&gap)?.concat(audio)?;
        }
        out.save(name, &joined, &VoiceProfile::default(), SEED)?;
    }

    // Rigid and expressive delivery of the same line with one character.
    for preset in [VoicePreset::Robot, VoicePreset::SoftRounded] {
        let profile = VoiceProfile::preset(preset);
        for delivery in [Delivery::Rigid, Delivery::Expressive] {
            let options_g = GibberishOptions {
                delivery,
                ..GibberishOptions::default()
            };
            let plan = plan_gibberish(LINE, &profile, &options_g)?;
            let voice = PreparedUtterance::new(&profile, &plan, &options())?;
            out.write(
                &format!("delivery-{preset}-{delivery:?}").to_lowercase(),
                &voice,
            )?;
        }
    }

    std::fs::write(out.dir.join("report.csv"), &out.report)?;
    println!("Wrote {}", out.dir.display());
    Ok(())
}
