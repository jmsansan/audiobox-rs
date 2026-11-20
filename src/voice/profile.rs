//! Voice profiles: a character's timbre and default delivery, with a text form.
use std::fmt;
use std::str::FromStr;

use super::PROFILE_FORMAT_VERSION;
use crate::synth::{Duty, NoiseMode, ParseError, Waveform};
use crate::{Error, ErrorCode, Result};

/// The longest profile text accepted, in bytes.
const MAX_PROFILE_BYTES: usize = 16 * 1024;

/// The excitation of a formant voice.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum VoiceSource {
    /// A smooth glottal-like pulse: the derivative of a raised-cosine flow with an
    /// adjustable open phase, band-limited at its closure. Softer and rounder.
    #[default]
    Smooth,
    /// The synthesizer's band-limited sawtooth: the buzzy, more robotic baseline.
    Saw,
}

/// How a profile makes its pitched sound.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VoiceMode {
    /// A source through a cascade of vowel resonances (formants).
    Formant(VoiceSource),
    /// A dry synthesizer oscillator, without formants: deliberately electronic
    /// blips. Vowels do not change its sound; formant fields are validated but unused.
    Robot(Waveform),
}
impl Default for VoiceMode {
    fn default() -> Self {
        Self::Formant(VoiceSource::Smooth)
    }
}

/// A character voice. Every field has a unit and a closed range, checked by
/// [`VoiceProfile::validate`] and again during preparation.
///
/// Fundamental pitch and vocal-resonance scale are separate: raising `pitch`
/// does not make the character sound smaller, and raising `resonance` does not
/// raise the notes.
///
/// | Field | Key | Unit | Range |
/// | --- | --- | --- | --- |
/// | `mode` | `mode`, `source` / `wave`, `duty`, `noise` | | formant (smooth, saw) or robot (any [`Waveform`]) |
/// | `pitch` | `pitch` | Hz | 50..=1000: base pitch the text planner speaks around |
/// | `range` | `range` | semitones | 0..=24: the planner's intonation range |
/// | `resonance` | `resonance` | ratio | 0.5..=2: scales every formant frequency |
/// | `bandwidth` | `bandwidth` | ratio | 0.25..=4: scales every formant bandwidth |
///
/// Formant frequencies and bandwidths are *pole* values, as in Klatt's cascade
/// synthesizer: each resonance's digital poles sit exactly at radius
/// `exp(−π·bandwidth/rate)` and angle `2π·frequency/rate`. With narrow bandwidths
/// the response peaks at the frequency; wide ones move the peak lower and flatten
/// it (300 Hz with 280 Hz of bandwidth peaks near 265 Hz).
/// | `open` | `open` | fraction | 0.3..=0.9: open phase of the smooth source |
/// | `tilt` | `tilt` | | 0..=1: softening low-pass on the source, 0 off |
/// | `breath` | `breath` | ratio | 0..=1: aspiration noise mixed into voiced sound |
/// | `fricative` | `fricative` | Hz | 1000..=12000: centre of fricative noise |
/// | `attack` | `attack` | ms | 1..=200: rise time, and fall time between gestures |
/// | `release` | `release` | ms | 1..=1000: fall time in the rests of a note list and at the end |
/// | `transition` | `transition` | ms | 1..=300: glide between vowel targets |
/// | `glide` | `glide` | ms | 0..=500: legato pitch glide between notes |
/// | `vibrato` | `vibrato` | cents | 0..=100: vibrato depth |
/// | `vibrato_rate` | `vibrato-rate` | Hz | 0.1..=12 |
/// | `jitter` | `jitter` | cents | 0..=50: slow seeded random pitch wander |
/// | `shimmer` | `shimmer` | ratio | 0..=0.5: slow seeded random level wander |
/// | `gain` | `gain` | linear | 0..=2: output gain before the guard |
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceProfile {
    pub mode: VoiceMode,
    pub pitch: f64,
    pub range: f64,
    pub resonance: f64,
    pub bandwidth: f64,
    pub open: f64,
    pub tilt: f64,
    pub breath: f64,
    pub fricative: f64,
    pub attack: f64,
    pub release: f64,
    pub transition: f64,
    pub glide: f64,
    pub vibrato: f64,
    pub vibrato_rate: f64,
    pub jitter: f64,
    pub shimmer: f64,
    pub gain: f64,
}

/// The default is [`VoicePreset::SoftRounded`].
impl Default for VoiceProfile {
    fn default() -> Self {
        Self::preset(VoicePreset::SoftRounded)
    }
}

/// The five starting characters of profile-format version 1. Each differs in
/// excitation, resonance, bandwidth, breath and envelope, not only in pitch.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VoicePreset {
    /// `robot`: a dry quarter-pulse oscillator with quick envelopes.
    Robot,
    /// `small-creature`: high pitch, short vocal tract (resonance 1.45), quick and
    /// lively.
    SmallCreature,
    /// `large-creature`: low pitch, long vocal tract (resonance 0.68), sawtooth
    /// source, slow envelopes.
    LargeCreature,
    /// `soft-rounded`: the default; a smooth source with gentle tilt and little
    /// breath.
    SoftRounded,
    /// `airy-breathy`: a wide open phase, strong aspiration and wide bandwidths.
    AiryBreathy,
}
impl VoicePreset {
    /// Every preset, in documentation order.
    pub const ALL: [VoicePreset; 5] = [
        Self::Robot,
        Self::SmallCreature,
        Self::LargeCreature,
        Self::SoftRounded,
        Self::AiryBreathy,
    ];
    /// The preset's stable text identifier.
    pub fn id(self) -> &'static str {
        match self {
            Self::Robot => "robot",
            Self::SmallCreature => "small-creature",
            Self::LargeCreature => "large-creature",
            Self::SoftRounded => "soft-rounded",
            Self::AiryBreathy => "airy-breathy",
        }
    }
}
impl fmt::Display for VoicePreset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}
impl FromStr for VoicePreset {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|p| p.id() == s)
            .ok_or_else(|| Error::invalid(format!("Unknown voice preset {s}")))
    }
}

/// A numeric field: its text key, range and accessor.
struct Field {
    key: &'static str,
    min: f64,
    max: f64,
    get: fn(&VoiceProfile) -> f64,
    set: fn(&mut VoiceProfile, f64),
}
macro_rules! field {
    ($key:literal, $name:ident, $min:expr, $max:expr) => {
        Field {
            key: $key,
            min: $min,
            max: $max,
            get: |p| p.$name,
            set: |p, v| p.$name = v,
        }
    };
}
const FIELDS: [Field; 17] = [
    field!("pitch", pitch, 50.0, 1000.0),
    field!("range", range, 0.0, 24.0),
    field!("resonance", resonance, 0.5, 2.0),
    field!("bandwidth", bandwidth, 0.25, 4.0),
    field!("open", open, 0.3, 0.9),
    field!("tilt", tilt, 0.0, 1.0),
    field!("breath", breath, 0.0, 1.0),
    field!("fricative", fricative, 1000.0, 12000.0),
    field!("attack", attack, 1.0, 200.0),
    field!("release", release, 1.0, 1000.0),
    field!("transition", transition, 1.0, 300.0),
    field!("glide", glide, 0.0, 500.0),
    field!("vibrato", vibrato, 0.0, 100.0),
    field!("vibrato-rate", vibrato_rate, 0.1, 12.0),
    field!("jitter", jitter, 0.0, 50.0),
    field!("shimmer", shimmer, 0.0, 0.5),
    field!("gain", gain, 0.0, 2.0),
];

impl VoiceProfile {
    /// The recipe of a preset for profile-format version 1.
    pub fn preset(preset: VoicePreset) -> Self {
        let soft = Self {
            mode: VoiceMode::Formant(VoiceSource::Smooth),
            pitch: 190.0,
            range: 6.0,
            resonance: 1.0,
            bandwidth: 1.0,
            open: 0.65,
            tilt: 0.3,
            breath: 0.06,
            fricative: 5500.0,
            attack: 12.0,
            release: 90.0,
            transition: 45.0,
            glide: 60.0,
            vibrato: 12.0,
            vibrato_rate: 5.2,
            jitter: 6.0,
            shimmer: 0.04,
            gain: 0.8,
        };
        match preset {
            VoicePreset::SoftRounded => soft,
            VoicePreset::Robot => Self {
                mode: VoiceMode::Robot(Waveform::Pulse(Duty::Quarter)),
                pitch: 180.0,
                range: 7.0,
                tilt: 0.0,
                breath: 0.0,
                fricative: 6000.0,
                attack: 4.0,
                release: 40.0,
                transition: 30.0,
                glide: 30.0,
                vibrato: 0.0,
                vibrato_rate: 5.0,
                jitter: 0.0,
                shimmer: 0.0,
                gain: 0.3,
                ..soft
            },
            VoicePreset::SmallCreature => Self {
                pitch: 420.0,
                range: 10.0,
                resonance: 1.45,
                bandwidth: 1.2,
                open: 0.55,
                tilt: 0.1,
                breath: 0.08,
                fricative: 7500.0,
                attack: 6.0,
                release: 50.0,
                transition: 25.0,
                glide: 40.0,
                vibrato: 25.0,
                vibrato_rate: 7.5,
                jitter: 12.0,
                shimmer: 0.08,
                gain: 0.8,
                ..soft
            },
            VoicePreset::LargeCreature => Self {
                mode: VoiceMode::Formant(VoiceSource::Saw),
                pitch: 85.0,
                range: 5.0,
                resonance: 0.68,
                bandwidth: 1.4,
                open: 0.7,
                tilt: 0.35,
                breath: 0.12,
                fricative: 3200.0,
                attack: 18.0,
                release: 120.0,
                transition: 70.0,
                glide: 80.0,
                vibrato: 10.0,
                vibrato_rate: 4.0,
                jitter: 18.0,
                shimmer: 0.12,
                gain: 0.9,
            },
            VoicePreset::AiryBreathy => Self {
                pitch: 230.0,
                range: 7.0,
                resonance: 1.12,
                bandwidth: 1.6,
                open: 0.8,
                tilt: 0.45,
                breath: 0.55,
                fricative: 6500.0,
                attack: 20.0,
                release: 140.0,
                transition: 55.0,
                glide: 70.0,
                vibrato: 8.0,
                vibrato_rate: 4.8,
                jitter: 8.0,
                shimmer: 0.06,
                gain: 0.9,
                ..soft
            },
        }
    }

    /// A robot profile with the given waveform and the [`VoicePreset::Robot`]
    /// envelope and pitch.
    pub fn robot(waveform: Waveform) -> Self {
        Self {
            mode: VoiceMode::Robot(waveform),
            ..Self::preset(VoicePreset::Robot)
        }
    }

    /// Checks every field's range and that the profile can play at `sample_rate`:
    /// its base pitch with full vibrato and jitter must stay below 0.45 of the rate.
    /// Errors are [`ErrorCode::InvalidArgument`] naming the field.
    pub fn validate(&self, sample_rate: u32) -> Result<()> {
        if let Some(e) = self.range_error() {
            return Err(Error::invalid(e));
        }
        if !(super::MIN_SAMPLE_RATE..=super::MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(Error::invalid("Voice sample rate is out of range"));
        }
        let top = self.pitch * crate::math::exp2(self.modulation_cents() / 1200.0);
        if top >= 0.45 * sample_rate as f64 {
            return Err(Error::invalid(format!(
                "Voice pitch {} Hz with its modulation exceeds 0.45 of {sample_rate} Hz",
                self.pitch
            )));
        }
        Ok(())
    }

    /// Range checks without a sample rate.
    pub(crate) fn check_fields(&self) -> Result<()> {
        match self.range_error() {
            Some(e) => Err(Error::invalid(e)),
            None => Ok(()),
        }
    }

    /// Peak pitch modulation from vibrato and jitter, in cents.
    pub(crate) fn modulation_cents(&self) -> f64 {
        self.vibrato + self.jitter
    }

    fn range_error(&self) -> Option<String> {
        for field in &FIELDS {
            let v = (field.get)(self);
            if !(v.is_finite() && v >= field.min && v <= field.max) {
                return Some(format!(
                    "Voice {} must be {}..={}, not {v}",
                    field.key, field.min, field.max
                ));
            }
        }
        None
    }

    /// Parses profile text, reporting the 1-based line and column of a problem.
    ///
    /// The text holds `key=value` words separated by whitespace, on any number of
    /// lines; `#` starts a comment to the end of the line. `version=1` is
    /// required. `preset=NAME` starts from a [`VoicePreset`]; every other key
    /// overrides only what it names, wherever it appears. Without a preset the base
    /// is [`VoiceProfile::default`]. A repeated or unknown key is
    /// [`ErrorCode::Syntax`]; an out-of-range value, or a source key that does not
    /// fit the mode (`source` with robot, `wave` with formant), is
    /// [`ErrorCode::InvalidArgument`]; text over 16 KiB is
    /// [`ErrorCode::LimitExceeded`].
    ///
    /// [`Display`](fmt::Display) writes the canonical form, every field spelled
    /// out, which parses back to the identical profile, bit for bit.
    pub fn parse(text: &str) -> std::result::Result<Self, ParseError> {
        parse(text)
    }
}

fn error(line: usize, column: usize, code: ErrorCode, message: impl Into<String>) -> ParseError {
    ParseError {
        line,
        column,
        code,
        message: message.into(),
    }
}

struct Pair<'a> {
    key: &'a str,
    value: &'a str,
    line: usize,
    column: usize,
    value_column: usize,
}

fn pairs(text: &str) -> std::result::Result<Vec<Pair<'_>>, ParseError> {
    let mut out = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let content = raw.split('#').next().unwrap_or("");
        let mut start = None;
        let mut column = 0;
        for (offset, c) in content.char_indices().chain([(content.len(), ' ')]) {
            column += 1;
            match (c.is_whitespace(), start) {
                (false, None) => start = Some((offset, column)),
                (true, Some((from, word_column))) => {
                    start = None;
                    let word = &content[from..offset];
                    let (key, value) = word.split_once('=').ok_or_else(|| {
                        error(
                            line,
                            word_column,
                            ErrorCode::Syntax,
                            format!("Expected key=value, found {word}"),
                        )
                    })?;
                    out.push(Pair {
                        key,
                        value,
                        line,
                        column: word_column,
                        value_column: word_column + key.chars().count() + 1,
                    });
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

fn parse(text: &str) -> std::result::Result<VoiceProfile, ParseError> {
    if text.len() > MAX_PROFILE_BYTES {
        return Err(error(
            1,
            1,
            ErrorCode::LimitExceeded,
            "Voice profile text exceeds 16 KiB",
        ));
    }
    let pairs = pairs(text)?;
    let mut seen: Vec<&str> = Vec::new();
    for pair in &pairs {
        if seen.contains(&pair.key) {
            return Err(error(
                pair.line,
                pair.column,
                ErrorCode::Syntax,
                format!("{} is set twice", pair.key),
            ));
        }
        let known = matches!(
            pair.key,
            "version" | "preset" | "mode" | "source" | "wave" | "duty" | "noise"
        ) || FIELDS.iter().any(|f| f.key == pair.key);
        if !known {
            return Err(error(
                pair.line,
                pair.column,
                ErrorCode::Syntax,
                format!("Unknown voice profile key {}", pair.key),
            ));
        }
        seen.push(pair.key);
    }
    let find = |key: &str| pairs.iter().find(|p| p.key == key);
    let at =
        |pair: &Pair<'_>, code, message: String| error(pair.line, pair.value_column, code, message);
    match find("version") {
        None => {
            return Err(error(
                1,
                1,
                ErrorCode::Syntax,
                "Voice profile text needs version=1",
            ));
        }
        Some(p) if p.value != PROFILE_FORMAT_VERSION.to_string() => {
            return Err(at(
                p,
                ErrorCode::InvalidArgument,
                format!("Unsupported voice profile version {}", p.value),
            ));
        }
        Some(_) => {}
    }
    let mut profile = match find("preset") {
        Some(p) => VoiceProfile::preset(
            p.value
                .parse()
                .map_err(|e: Error| at(p, ErrorCode::InvalidArgument, e.message))?,
        ),
        None => VoiceProfile::default(),
    };
    // The mode: explicit, or kept from the base when only its details change.
    let robot = match find("mode") {
        Some(p) => match p.value {
            "formant" => false,
            "robot" => true,
            other => {
                return Err(at(
                    p,
                    ErrorCode::Syntax,
                    format!("mode must be formant or robot, not {other}"),
                ));
            }
        },
        None => matches!(profile.mode, VoiceMode::Robot(_)),
    };
    if robot {
        if let Some(p) = find("source") {
            return Err(at(
                p,
                ErrorCode::InvalidArgument,
                "source applies only to mode=formant".into(),
            ));
        }
        let mut wave = match profile.mode {
            VoiceMode::Robot(w) => w,
            VoiceMode::Formant(_) => Waveform::Pulse(Duty::Quarter),
        };
        if let Some(p) = find("wave") {
            wave = match p.value {
                "pulse" => Waveform::Pulse(match wave {
                    Waveform::Pulse(d) => d,
                    _ => Duty::Half,
                }),
                "triangle" => Waveform::Triangle,
                "saw" => Waveform::Sawtooth,
                "noise" => Waveform::Noise(match wave {
                    Waveform::Noise(m) => m,
                    _ => NoiseMode::Long,
                }),
                other => {
                    return Err(at(
                        p,
                        ErrorCode::Syntax,
                        format!("wave must be pulse, triangle, saw or noise, not {other}"),
                    ));
                }
            };
        }
        if let Some(p) = find("duty") {
            let duty: Duty = p
                .value
                .parse()
                .map_err(|e: Error| at(p, ErrorCode::InvalidArgument, e.message))?;
            match wave {
                Waveform::Pulse(_) => wave = Waveform::Pulse(duty),
                _ => {
                    return Err(at(
                        p,
                        ErrorCode::InvalidArgument,
                        "duty applies only to wave=pulse".into(),
                    ));
                }
            }
        }
        if let Some(p) = find("noise") {
            let mode = match p.value {
                "long" => NoiseMode::Long,
                "short" => NoiseMode::Short,
                other => {
                    return Err(at(
                        p,
                        ErrorCode::Syntax,
                        format!("noise must be long or short, not {other}"),
                    ));
                }
            };
            match wave {
                Waveform::Noise(_) => wave = Waveform::Noise(mode),
                _ => {
                    return Err(at(
                        p,
                        ErrorCode::InvalidArgument,
                        "noise applies only to wave=noise".into(),
                    ));
                }
            }
        }
        profile.mode = VoiceMode::Robot(wave);
    } else {
        for key in ["wave", "duty", "noise"] {
            if let Some(p) = find(key) {
                return Err(at(
                    p,
                    ErrorCode::InvalidArgument,
                    format!("{key} applies only to mode=robot"),
                ));
            }
        }
        let mut source = match profile.mode {
            VoiceMode::Formant(s) => s,
            VoiceMode::Robot(_) => VoiceSource::Smooth,
        };
        if let Some(p) = find("source") {
            source = match p.value {
                "smooth" => VoiceSource::Smooth,
                "saw" => VoiceSource::Saw,
                other => {
                    return Err(at(
                        p,
                        ErrorCode::Syntax,
                        format!("source must be smooth or saw, not {other}"),
                    ));
                }
            };
        }
        profile.mode = VoiceMode::Formant(source);
    }
    for field in &FIELDS {
        if let Some(p) = find(field.key) {
            let value: f64 = p.value.parse().map_err(|_| {
                at(
                    p,
                    ErrorCode::Syntax,
                    format!("{} must be a number, not {}", field.key, p.value),
                )
            })?;
            if !(value.is_finite() && value >= field.min && value <= field.max) {
                return Err(at(
                    p,
                    ErrorCode::InvalidArgument,
                    format!(
                        "{} must be {}..={}, not {}",
                        field.key, field.min, field.max, p.value
                    ),
                ));
            }
            (field.set)(&mut profile, value);
        }
    }
    Ok(profile)
}

impl fmt::Display for VoiceProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "version={PROFILE_FORMAT_VERSION}")?;
        match self.mode {
            VoiceMode::Formant(VoiceSource::Smooth) => writeln!(f, "mode=formant source=smooth")?,
            VoiceMode::Formant(VoiceSource::Saw) => writeln!(f, "mode=formant source=saw")?,
            VoiceMode::Robot(wave) => match wave {
                Waveform::Pulse(duty) => {
                    let duty = match duty {
                        Duty::Eighth => "12.5",
                        Duty::Quarter => "25",
                        Duty::Half => "50",
                        Duty::ThreeQuarters => "75",
                    };
                    writeln!(f, "mode=robot wave=pulse duty={duty}")?
                }
                Waveform::Triangle => writeln!(f, "mode=robot wave=triangle")?,
                Waveform::Sawtooth => writeln!(f, "mode=robot wave=saw")?,
                Waveform::Noise(NoiseMode::Long) => {
                    writeln!(f, "mode=robot wave=noise noise=long")?
                }
                Waveform::Noise(NoiseMode::Short) => {
                    writeln!(f, "mode=robot wave=noise noise=short")?
                }
            },
        }
        for field in &FIELDS {
            writeln!(f, "{}={}", field.key, (field.get)(self))?;
        }
        Ok(())
    }
}

impl FromStr for VoiceProfile {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s).map_err(Error::from)
    }
}
