//! One-shot sound effects from FM patches.
use super::{MAX_DURATION, ParseError};
use crate::synth::{
    ControlEvent, ControlParameter, ControlTarget, Fm, FmEnvelope, FmFrequency, FmLfo, FmLfoWave,
    FmOperator, FmPatch, FrameNote, FrameScore, Instrument, Pitch, RenderOptions, Synth,
    TempoChange, TempoMap,
};
use crate::{Audio, Error, ErrorCode, Result};
use std::fmt;

/// One note of an FM patch as a sound effect: a pitch, a velocity, how long the key
/// is held and an optional pitch slide. It renders deterministically into [`Audio`],
/// so it plays through the mixer and sound events like any [`super::Effect`].
///
/// ```
/// use audiobox::sfx::FmEffect;
/// let chime: FmEffect = "note=84 gate=0.1 volume=0.6
/// fm chime algorithm=1
///   op 1 ratio=1 rates=99,40,40,45 levels=99,0,0,0
///   op 2 ratio=3,50 level=75 rates=99,50,50,50 levels=99,0,0,0
/// end".parse()?;
/// let audio = chime.render(44_100)?;
/// assert!(audio.frames() > 4_410);
/// assert_eq!(chime.to_string().parse::<FmEffect>()?, chime);
/// # Ok::<(), audiobox::Error>(())
/// ```
///
/// The text form is one optional line of `key=value` pairs (`note`, `velocity`,
/// `gate`, `slide`, `volume`) followed by an `fm` block as documented in
/// `docs/fm-voice.md`.
#[derive(Clone, Debug, PartialEq)]
pub struct FmEffect {
    pub patch: FmPatch,
    /// MIDI note, 0..=127. 60 is middle C.
    pub note: u8,
    /// 0..=127. It reaches the sound through each operator's `velocity` sensitivity.
    pub velocity: u8,
    /// Seconds the key is held, 0.001..=10. The patch's release follows it.
    pub gate: f64,
    /// Pitch slide over the gate, in semitones per second. `slide * gate` must stay
    /// within two octaves either way. The pitch holds where it ends for the release.
    /// Fixed-frequency operators do not slide.
    pub slide: f64,
    /// Linear output level, 0..=1.
    pub volume: f64,
}

/// Kinds of FM effect that [`FmEffect::preset`] draws from a seed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FmPreset {
    /// A shot: a bright tone whose pitch and brightness fall fast.
    Laser,
    /// An impact on metal: inharmonic fixed-frequency partials that die quickly.
    MetalHit,
    /// A bell-like note that rings out.
    Chime,
    /// A sustained two-tone warning.
    Alarm,
}
impl FmPreset {
    pub const ALL: [FmPreset; 4] = [Self::Laser, Self::MetalHit, Self::Chime, Self::Alarm];
}

impl FmEffect {
    /// One note of `patch` at middle C, full velocity, held for 0.2 seconds.
    pub fn new(patch: FmPatch) -> Self {
        Self {
            patch,
            note: 60,
            velocity: 127,
            gate: 0.2,
            slide: 0.0,
            volume: 0.5,
        }
    }
    /// Draws an effect of the given kind from a seed. The same kind and seed always
    /// give the same effect.
    pub fn preset(kind: FmPreset, seed: u64) -> Self {
        generate(kind, seed)
    }
    /// Parses the text form, with the line and column of a fault.
    pub fn parse(text: &str) -> std::result::Result<Self, ParseError> {
        parse(text)
    }
    pub fn validate(&self) -> Result<()> {
        self.patch.validate()?;
        if self.note > 127 || self.velocity > 127 {
            return Err(Error::invalid("note and velocity must be 0..=127"));
        }
        if !(0.001..=MAX_DURATION).contains(&self.gate) {
            return Err(Error::invalid(format!(
                "gate must be 0.001..={MAX_DURATION} seconds"
            )));
        }
        if !(0.0..=1.0).contains(&self.volume) {
            return Err(Error::invalid("volume must be 0..=1"));
        }
        if !self.slide.is_finite() || (self.slide * self.gate).abs() > 24.0 {
            return Err(Error::invalid(
                "slide * gate must stay within 24 semitones either way",
            ));
        }
        Ok(())
    }
    /// Renders mono audio: the gate, then the patch's release, cut with a short fade
    /// if it would pass [`MAX_DURATION`].
    pub fn render(&self, sample_rate: u32) -> Result<Audio> {
        self.render_channels(sample_rate, 1)
    }
    /// Renders stereo, keeping the patch's pan and unison spread.
    pub fn render_stereo(&self, sample_rate: u32) -> Result<Audio> {
        self.render_channels(sample_rate, 2)
    }
    fn render_channels(&self, rate: u32, channels: usize) -> Result<Audio> {
        self.validate()?;
        let map = TempoMap::new(
            rate,
            48,
            &[TempoChange {
                tick: 0,
                bpm: 120.0,
            }],
        )?;
        let gate = ((self.gate * rate as f64).round() as u64).max(1);
        let mut note = FrameNote::new(0, gate, Pitch::new(self.note)?, 0);
        note.velocity = self.velocity;
        let score = FrameScore {
            instruments: vec![Instrument {
                volume: self.patch.volume,
                ..Instrument::fm(self.patch.voice)
            }],
            tracks: vec![vec![note]],
            end_frame: gate,
        };
        let options = RenderOptions {
            gain: self.volume as f32,
            ..RenderOptions::default()
        };
        let mut synth = if channels == 2 {
            Synth::from_frames_stereo(&score, &map, &options)?
        } else {
            Synth::from_frames(&score, &map, &options)?
        };
        if self.slide != 0.0 && gate > 1 {
            let target = ControlTarget::Instrument {
                track: 0,
                instrument: 0,
            };
            let slide = ControlEvent {
                ramp_frames: gate - 1,
                ..ControlEvent::new(
                    0,
                    target,
                    ControlParameter::PitchCents,
                    self.slide * self.gate * 100.0,
                )
            };
            synth.prepare_controls(&[slide], 1)?;
        }
        let limit = (MAX_DURATION * rate as f64) as u64;
        let frames = synth.max_frames().unwrap_or(limit).min(limit) as usize;
        let mut samples = vec![0.0f32; frames * channels];
        let written = synth.fill(&mut samples);
        samples.truncate(written * channels);
        if written == frames && frames as u64 == limit {
            // The release was cut: fade the last 5 ms.
            let fade = (rate as usize / 200).clamp(1, written);
            for i in 0..fade {
                let gain = (fade - 1 - i) as f32 / fade as f32;
                for sample in &mut samples[(written - fade + i) * channels..][..channels] {
                    *sample *= gain;
                }
            }
        }
        let planar = (0..channels)
            .map(|c| samples.iter().skip(c).step_by(channels).copied().collect())
            .collect();
        Audio::new(planar, rate)
    }
}

impl fmt::Display for FmEffect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "note={} velocity={} gate={} slide={} volume={}",
            self.note, self.velocity, self.gate, self.slide, self.volume
        )?;
        write!(f, "{}", self.patch)
    }
}
impl std::str::FromStr for FmEffect {
    type Err = Error;
    fn from_str(text: &str) -> Result<Self> {
        Ok(Self::parse(text)?)
    }
}

fn parse(text: &str) -> std::result::Result<FmEffect, ParseError> {
    let error = |line: usize, column: usize, code, message: String| ParseError {
        line,
        column,
        code,
        message,
    };
    let (mut note, mut velocity, mut gate, mut slide, mut volume) = (60, 127, 0.2, 0.0, 0.5);
    let mut seen: Vec<&str> = Vec::new();
    let mut header = 0;
    for (index, raw) in text.lines().enumerate() {
        let content = raw.split(';').next().unwrap_or("");
        if content.split_whitespace().next() == Some("fm") {
            break;
        }
        header = index + 1;
        let mut column = 1;
        for word in content.split(' ') {
            let start = column;
            column += word.chars().count() + 1;
            let word = word.trim();
            if word.is_empty() {
                continue;
            }
            let at = |code, message: String| error(index + 1, start, code, message);
            let (key, value) = word.split_once('=').ok_or_else(|| {
                at(
                    ErrorCode::Syntax,
                    format!("Expected key=value or an fm block, found {word}"),
                )
            })?;
            if seen.contains(&key) {
                return Err(at(ErrorCode::Syntax, format!("{key} is set twice")));
            }
            seen.push(key);
            let value_at = start + key.chars().count() + 1;
            let number = |min: f64, max: f64| {
                let parsed: f64 = value.parse().map_err(|_| {
                    error(
                        index + 1,
                        value_at,
                        ErrorCode::Syntax,
                        format!("{key} needs a number, not {value}"),
                    )
                })?;
                if !(min..=max).contains(&parsed) {
                    return Err(error(
                        index + 1,
                        value_at,
                        ErrorCode::InvalidArgument,
                        format!("{key} must be {min}..={max}, not {value}"),
                    ));
                }
                Ok(parsed)
            };
            let whole = |value: f64| {
                if value.fract() != 0.0 {
                    return Err(error(
                        index + 1,
                        value_at,
                        ErrorCode::Syntax,
                        format!("{key} needs a whole number"),
                    ));
                }
                Ok(value as u8)
            };
            match key {
                "note" => note = whole(number(0.0, 127.0)?)?,
                "velocity" => velocity = whole(number(0.0, 127.0)?)?,
                "gate" => gate = number(0.001, MAX_DURATION)?,
                "slide" => slide = number(-24000.0, 24000.0)?,
                "volume" => volume = number(0.0, 1.0)?,
                _ => return Err(at(ErrorCode::Syntax, format!("Unknown key {key}"))),
            }
        }
    }
    // Blank lines stand in for the header so the block keeps its line numbers.
    let block: String = text
        .lines()
        .enumerate()
        .map(|(index, line)| {
            if index < header {
                "\n".to_string()
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    let effect = FmEffect {
        patch: FmPatch::parse(&block)?,
        note,
        velocity,
        gate,
        slide,
        volume,
    };
    effect
        .validate()
        .map_err(|e| error(1, 1, e.code, e.message))?;
    Ok(effect)
}

fn generate(kind: FmPreset, seed: u64) -> FmEffect {
    let index = FmPreset::ALL.iter().position(|&k| k == kind).unwrap_or(0) as u64;
    let mut rng = super::preset::Rng::new(seed ^ (0xf3_0000 + index));
    let mut int = |low: u8, high: u8| rng.range(low as f64, high as f64).round() as u8;
    let ratio = |coarse: u8, fine: u8| FmFrequency::Ratio { coarse, fine };
    let fixed = |decade: u8, fine: u8| FmFrequency::Fixed { decade, fine };
    let operator = |frequency, level, rates: [u8; 4], levels: [u8; 4]| {
        Some(FmOperator {
            frequency,
            level,
            envelope: FmEnvelope { rates, levels },
            ..FmOperator::default()
        })
    };
    let mut voice = Fm {
        operators: [None; 6],
        voices: 1,
        ..Fm::default()
    };
    let (name, note, gate, slide);
    match kind {
        FmPreset::Laser => {
            name = "laser";
            voice.algorithm = 2;
            voice.feedback = int(2, 6);
            let fall = int(55, 72);
            voice.operators[0] = operator(ratio(1, 0), 99, [99, fall, 99, 70], [99, 0, 0, 0]);
            voice.operators[1] = operator(
                ratio(int(2, 7), 0),
                int(70, 88),
                [99, fall + 6, 99, 70],
                [99, 0, 0, 0],
            );
            note = int(79, 96);
            gate = int(12, 30) as f64 / 100.0;
            slide = -((int(120, 220) as f64 / 10.0) / gate).round();
        }
        FmPreset::MetalHit => {
            name = "metal-hit";
            voice.algorithm = 5;
            voice.feedback = int(4, 7);
            for pair in 0..3 {
                let decay = int(56, 70);
                voice.operators[pair * 2] = operator(
                    fixed(int(2, 3), int(5, 95)),
                    99 - pair as u8 * 6,
                    [99, decay, 99, decay],
                    [99, 0, 0, 0],
                );
                voice.operators[pair * 2 + 1] = operator(
                    fixed(int(2, 3), int(5, 95)),
                    int(72, 90),
                    [99, decay + 8, 99, 80],
                    [99, 0, 0, 0],
                );
            }
            note = 60;
            gate = 0.05;
            slide = 0.0;
        }
        FmPreset::Chime => {
            name = "chime";
            voice.algorithm = 5;
            let ring = int(34, 44);
            let fines = [50, 0, 41];
            for pair in 0..2 {
                voice.operators[pair * 2] = operator(
                    ratio(1 + pair as u8 * int(1, 3), 0),
                    99 - pair as u8 * 10,
                    [99, ring + pair as u8 * 6, 99, ring + 4],
                    [99, 0, 0, 0],
                );
                voice.operators[pair * 2 + 1] = operator(
                    ratio(int(3, 9), fines[int(0, 2) as usize]),
                    int(62, 80),
                    [99, ring + 12, 99, 60],
                    [99, 0, 0, 0],
                );
            }
            note = int(79, 96);
            gate = 0.1;
            slide = 0.0;
        }
        FmPreset::Alarm => {
            name = "alarm";
            voice.algorithm = 2;
            voice.feedback = int(3, 6);
            voice.lfo = FmLfo {
                wave: [FmLfoWave::Square, FmLfoWave::SawUp, FmLfoWave::Triangle]
                    [int(0, 2) as usize],
                speed: int(40, 62),
                pitch_depth: int(40, 99),
                pitch_sensitivity: int(4, 6),
                ..FmLfo::default()
            };
            voice.operators[0] = operator(ratio(1, 0), 99, [90, 99, 99, 75], [99, 99, 99, 0]);
            voice.operators[1] = operator(
                ratio(int(1, 3), 0),
                int(66, 80),
                [90, 99, 99, 75],
                [99, 99, 99, 0],
            );
            note = int(72, 84);
            gate = int(60, 120) as f64 / 100.0;
            slide = 0.0;
        }
    }
    FmEffect {
        patch: FmPatch {
            name: name.into(),
            volume: 15,
            voice,
        },
        note,
        velocity: 127,
        gate,
        slide,
        volume: 0.5,
    }
}
