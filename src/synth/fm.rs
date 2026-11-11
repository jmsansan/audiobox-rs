//! The FM voice contract: a six-operator, DX7-class parameter model and its text form.
//!
//! This module defines and validates the patch; it does not render yet. Units are the
//! instrument's own integer ranges ("DX units"). See `docs/fm-voice.md` for the
//! conversion curves, the routing table, limits and the implementation slices.
use std::fmt;

use crate::{Error, Result};

/// Operators per voice.
pub const FM_OPERATORS: usize = 6;
/// Named routings, numbered 1..=32.
pub const FM_ALGORITHMS: u8 = 32;
/// Upper bound of `voices * unison` for one FM instrument.
pub const MAX_FM_UNISON_VOICES: u16 = 32;
/// Peak of one carrier at output level 99, envelope level 99 and no velocity or
/// scaling offset, before instrument volume. Carriers add without normalization.
pub const FM_CARRIER_PEAK: f64 = 0.5;
/// Patch text longer than this is rejected.
pub const MAX_FM_TEXT_BYTES: usize = 64 * 1024;

/// Four rates and four levels, each 0..=99.
///
/// Key-on moves from `levels[3]` to `levels[0]` at `rates[0]`, on to `levels[1]` and
/// `levels[2]`, and holds there. Key-off moves to `levels[3]` at `rates[3]`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FmEnvelope {
    pub rates: [u8; 4],
    pub levels: [u8; 4],
}
impl FmEnvelope {
    /// An operator's default: instant attack, full sustain, instant release to silence.
    pub const OPERATOR: Self = Self {
        rates: [99; 4],
        levels: [99, 99, 99, 0],
    };
    /// The pitch envelope's default: level 50 everywhere, which is no pitch offset.
    pub const PITCH: Self = Self {
        rates: [99; 4],
        levels: [50; 4],
    };
    fn validate(self, what: &str) -> Result<()> {
        if self.rates.iter().chain(&self.levels).any(|&v| v > 99) {
            return Err(Error::invalid(format!(
                "{what} envelope rates and levels must be 0..=99"
            )));
        }
        Ok(())
    }
}

/// An operator's pitch source.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FmFrequency {
    /// A multiple of the note's frequency. `coarse` 0 is 0.5, 1..=31 is itself;
    /// `fine` 0..=99 multiplies by `1 + fine / 100`.
    Ratio { coarse: u8, fine: u8 },
    /// A frequency that ignores the note: `10^(decade + fine / 100)` Hz, with
    /// `decade` 0..=3 (1, 10, 100, 1000 Hz) and `fine` 0..=99.
    Fixed { decade: u8, fine: u8 },
}
impl FmFrequency {
    /// The multiple of the note's frequency, or `None` in fixed mode.
    pub fn ratio(self) -> Option<f64> {
        match self {
            Self::Ratio { coarse, fine } => {
                let base = if coarse == 0 { 0.5 } else { coarse as f64 };
                Some(base * (1.0 + fine as f64 / 100.0))
            }
            Self::Fixed { .. } => None,
        }
    }
    /// The fixed frequency in Hz, or `None` in ratio mode.
    pub fn fixed_hz(self) -> Option<f64> {
        match self {
            Self::Fixed { decade, fine } => Some(crate::math::exp2(
                (decade as f64 + fine as f64 / 100.0) * std::f64::consts::LOG2_10,
            )),
            Self::Ratio { .. } => None,
        }
    }
    fn validate(self) -> Result<()> {
        let ok = match self {
            Self::Ratio { coarse, fine } => coarse <= 31 && fine <= 99,
            Self::Fixed { decade, fine } => decade <= 3 && fine <= 99,
        };
        if !ok {
            return Err(Error::invalid(
                "Operator frequency needs coarse 0..=31 (fixed decade 0..=3) and fine 0..=99",
            ));
        }
        Ok(())
    }
}

/// The shape of keyboard level scaling on one side of the break point.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum FmCurve {
    /// `-lin`: the level falls linearly with distance from the break point.
    #[default]
    NegativeLinear,
    /// `-exp`
    NegativeExponential,
    /// `+exp`
    PositiveExponential,
    /// `+lin`
    PositiveLinear,
}
impl FmCurve {
    pub(super) const NAMES: [(&'static str, Self); 4] = [
        ("-lin", Self::NegativeLinear),
        ("-exp", Self::NegativeExponential),
        ("+exp", Self::PositiveExponential),
        ("+lin", Self::PositiveLinear),
    ];
    fn name(self) -> &'static str {
        Self::NAMES[self as usize].0
    }
}

/// Keyboard level scaling: an offset to an operator's level by note number.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FmScaling {
    /// 0..=99; MIDI note `break_point + 21`. The default, 39, is MIDI 60.
    pub break_point: u8,
    /// 0..=99 below the break point.
    pub left_depth: u8,
    /// 0..=99 above the break point.
    pub right_depth: u8,
    pub left_curve: FmCurve,
    pub right_curve: FmCurve,
}
impl Default for FmScaling {
    fn default() -> Self {
        Self {
            break_point: 39,
            left_depth: 0,
            right_depth: 0,
            left_curve: FmCurve::default(),
            right_curve: FmCurve::default(),
        }
    }
}

/// One sine operator.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FmOperator {
    pub frequency: FmFrequency,
    /// -7..=7, a few cents.
    pub detune: i8,
    /// Output level, 0..=99, logarithmic. On a modulator it sets the modulation index.
    pub level: u8,
    pub envelope: FmEnvelope,
    pub scaling: FmScaling,
    /// 0..=7: higher notes run the envelope faster.
    pub rate_scaling: u8,
    /// Velocity sensitivity, 0..=7. At 0 the operator ignores velocity.
    pub velocity: u8,
    /// 0..=3: how much of the LFO's amplitude depth reaches this operator.
    pub amplitude_sensitivity: u8,
}
impl Default for FmOperator {
    fn default() -> Self {
        Self {
            frequency: FmFrequency::Ratio { coarse: 1, fine: 0 },
            detune: 0,
            level: 99,
            envelope: FmEnvelope::OPERATOR,
            scaling: FmScaling::default(),
            rate_scaling: 0,
            velocity: 0,
            amplitude_sensitivity: 0,
        }
    }
}
impl FmOperator {
    pub fn validate(&self) -> Result<()> {
        self.frequency.validate()?;
        self.envelope.validate("Operator")?;
        let s = self.scaling;
        if !(-7..=7).contains(&self.detune)
            || self.level > 99
            || s.break_point > 99
            || s.left_depth > 99
            || s.right_depth > 99
            || self.rate_scaling > 7
            || self.velocity > 7
            || self.amplitude_sensitivity > 3
        {
            return Err(Error::invalid(
                "Operator needs detune -7..=7, level and scaling 0..=99, rate scaling and velocity 0..=7, amplitude sensitivity 0..=3",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum FmLfoWave {
    #[default]
    Triangle,
    SawDown,
    SawUp,
    Square,
    Sine,
    /// A new seeded random value each cycle.
    SampleHold,
}
impl FmLfoWave {
    pub(super) const NAMES: [(&'static str, Self); 6] = [
        ("triangle", Self::Triangle),
        ("saw-down", Self::SawDown),
        ("saw-up", Self::SawUp),
        ("square", Self::Square),
        ("sine", Self::Sine),
        ("sample-hold", Self::SampleHold),
    ];
    fn name(self) -> &'static str {
        Self::NAMES[self as usize].0
    }
}

/// The voice's single LFO, routed to pitch for the voice and to amplitude per operator.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FmLfo {
    pub wave: FmLfoWave,
    /// 0..=99. Ignored while `beats` is set.
    pub speed: u8,
    /// 0..=99: time before the modulation fades in after a note starts.
    pub delay: u8,
    /// 0..=99.
    pub pitch_depth: u8,
    /// 0..=99.
    pub amplitude_depth: u8,
    /// Restart the LFO at each note. Off follows the synth's clock.
    pub key_sync: bool,
    /// 0..=7: scales `pitch_depth` for the whole voice.
    pub pitch_sensitivity: u8,
    /// Engine extension: `(numerator, denominator)` quarter-note beats per cycle,
    /// both 1..=64, following the tempo map instead of `speed`.
    pub beats: Option<(u16, u16)>,
    /// Seed of the sample-and-hold sequence.
    pub seed: u32,
}
impl Default for FmLfo {
    fn default() -> Self {
        Self {
            wave: FmLfoWave::default(),
            speed: 35,
            delay: 0,
            pitch_depth: 0,
            amplitude_depth: 0,
            key_sync: true,
            pitch_sensitivity: 3,
            beats: None,
            seed: 1,
        }
    }
}
impl FmLfo {
    fn validate(&self) -> Result<()> {
        if self.speed > 99
            || self.delay > 99
            || self.pitch_depth > 99
            || self.amplitude_depth > 99
            || self.pitch_sensitivity > 7
            || self
                .beats
                .is_some_and(|(n, d)| !(1..=64).contains(&n) || !(1..=64).contains(&d))
        {
            return Err(Error::invalid(
                "LFO needs speed, delay and depths 0..=99, pitch sensitivity 0..=7 and beats 1..=64",
            ));
        }
        Ok(())
    }
}

/// How one algorithm connects the operators. Operators are numbered 1..=6; bit
/// `n - 1` of a mask stands for operator `n`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FmRouting {
    /// For each operator, the operators that modulate its phase.
    pub modulators: [u8; FM_OPERATORS],
    /// The operators that are summed into the output.
    pub carriers: u8,
    /// The operator whose output is fed back, 1..=6.
    pub feedback_from: u8,
    /// The operator whose phase receives the feedback, 1..=6. It differs from
    /// `feedback_from` only in algorithms 4 and 6.
    pub feedback_to: u8,
}
impl FmRouting {
    /// The routing of algorithm 1..=32.
    pub fn algorithm(number: u8) -> Result<Self> {
        if !(1..=FM_ALGORITHMS).contains(&number) {
            return Err(Error::invalid("FM algorithm must be 1..=32"));
        }
        let (modulators, carriers, feedback_from, feedback_to) = ALGORITHMS[number as usize - 1];
        Ok(Self {
            modulators,
            carriers,
            feedback_from,
            feedback_to,
        })
    }
    /// Whether operator 1..=6 is summed into the output.
    pub fn is_carrier(&self, operator: u8) -> bool {
        (1..=6).contains(&operator) && self.carriers & (1 << (operator - 1)) != 0
    }
}

type Row = ([u8; FM_OPERATORS], u8, u8, u8);
/// Modulator masks of operators 1..=6, carrier mask, feedback source and destination.
/// In every row a modulator has a higher number than the operator it modulates, so
/// evaluating 6 down to 1 has each input ready; only feedback is delayed.
#[rustfmt::skip]
const ALGORITHMS: [Row; FM_ALGORITHMS as usize] = [
    ([0b000010, 0b000000, 0b001000, 0b010000, 0b100000, 0b000000], 0b000101, 6, 6), // 1: 2>1, 6>5>4>3
    ([0b000010, 0b000000, 0b001000, 0b010000, 0b100000, 0b000000], 0b000101, 2, 2), // 2
    ([0b000010, 0b000100, 0b000000, 0b010000, 0b100000, 0b000000], 0b001001, 6, 6), // 3: 3>2>1, 6>5>4
    ([0b000010, 0b000100, 0b000000, 0b010000, 0b100000, 0b000000], 0b001001, 4, 6), // 4
    ([0b000010, 0b000000, 0b001000, 0b000000, 0b100000, 0b000000], 0b010101, 6, 6), // 5: 2>1, 4>3, 6>5
    ([0b000010, 0b000000, 0b001000, 0b000000, 0b100000, 0b000000], 0b010101, 5, 6), // 6
    ([0b000010, 0b000000, 0b011000, 0b000000, 0b100000, 0b000000], 0b000101, 6, 6), // 7: 2>1, 4+5>3, 6>5
    ([0b000010, 0b000000, 0b011000, 0b000000, 0b100000, 0b000000], 0b000101, 4, 4), // 8
    ([0b000010, 0b000000, 0b011000, 0b000000, 0b100000, 0b000000], 0b000101, 2, 2), // 9
    ([0b000010, 0b000100, 0b000000, 0b110000, 0b000000, 0b000000], 0b001001, 3, 3), // 10: 3>2>1, 5+6>4
    ([0b000010, 0b000100, 0b000000, 0b110000, 0b000000, 0b000000], 0b001001, 6, 6), // 11
    ([0b000010, 0b000000, 0b111000, 0b000000, 0b000000, 0b000000], 0b000101, 2, 2), // 12: 2>1, 4+5+6>3
    ([0b000010, 0b000000, 0b111000, 0b000000, 0b000000, 0b000000], 0b000101, 6, 6), // 13
    ([0b000010, 0b000000, 0b001000, 0b110000, 0b000000, 0b000000], 0b000101, 6, 6), // 14: 2>1, 5+6>4>3
    ([0b000010, 0b000000, 0b001000, 0b110000, 0b000000, 0b000000], 0b000101, 2, 2), // 15
    ([0b010110, 0b000000, 0b001000, 0b000000, 0b100000, 0b000000], 0b000001, 6, 6), // 16: 2+3+5>1, 4>3, 6>5
    ([0b010110, 0b000000, 0b001000, 0b000000, 0b100000, 0b000000], 0b000001, 2, 2), // 17
    ([0b001110, 0b000000, 0b000000, 0b010000, 0b100000, 0b000000], 0b000001, 3, 3), // 18: 2+3+4>1, 6>5>4
    ([0b000010, 0b000100, 0b000000, 0b100000, 0b100000, 0b000000], 0b011001, 6, 6), // 19: 3>2>1, 6>4, 6>5
    ([0b000100, 0b000100, 0b000000, 0b110000, 0b000000, 0b000000], 0b001011, 3, 3), // 20: 3>1, 3>2, 5+6>4
    ([0b000100, 0b000100, 0b000000, 0b100000, 0b100000, 0b000000], 0b011011, 3, 3), // 21: 3>1, 3>2, 6>4, 6>5
    ([0b000010, 0b000000, 0b100000, 0b100000, 0b100000, 0b000000], 0b011101, 6, 6), // 22: 2>1, 6>3, 6>4, 6>5
    ([0b000000, 0b000100, 0b000000, 0b100000, 0b100000, 0b000000], 0b011011, 6, 6), // 23: 1, 3>2, 6>4, 6>5
    ([0b000000, 0b000000, 0b100000, 0b100000, 0b100000, 0b000000], 0b011111, 6, 6), // 24: 1, 2, 6>3, 6>4, 6>5
    ([0b000000, 0b000000, 0b000000, 0b100000, 0b100000, 0b000000], 0b011111, 6, 6), // 25: 1, 2, 3, 6>4, 6>5
    ([0b000000, 0b000100, 0b000000, 0b110000, 0b000000, 0b000000], 0b001011, 6, 6), // 26: 1, 3>2, 5+6>4
    ([0b000000, 0b000100, 0b000000, 0b110000, 0b000000, 0b000000], 0b001011, 3, 3), // 27
    ([0b000010, 0b000000, 0b001000, 0b010000, 0b000000, 0b000000], 0b100101, 5, 5), // 28: 2>1, 5>4>3, 6
    ([0b000000, 0b000000, 0b001000, 0b000000, 0b100000, 0b000000], 0b010111, 6, 6), // 29: 1, 2, 4>3, 6>5
    ([0b000000, 0b000000, 0b001000, 0b010000, 0b000000, 0b000000], 0b100111, 5, 5), // 30: 1, 2, 5>4>3, 6
    ([0b000000, 0b000000, 0b000000, 0b000000, 0b100000, 0b000000], 0b011111, 6, 6), // 31: 1, 2, 3, 4, 6>5
    ([0b000000, 0b000000, 0b000000, 0b000000, 0b000000, 0b000000], 0b111111, 6, 6), // 32: six carriers
];

/// A complete FM voice. `Copy` and fixed-size, so prepared rendering needs no allocation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Fm {
    /// 1..=32. See [`FmRouting::algorithm`].
    pub algorithm: u8,
    /// 0..=7 on the algorithm's feedback path.
    pub feedback: u8,
    /// Restart operator phases at each note. Off leaves them free-running.
    pub key_sync: bool,
    /// -24..=24 semitones.
    pub transpose: i8,
    /// Operators 1..=6. `None` is switched off: silent, and it modulates nothing.
    pub operators: [Option<FmOperator>; FM_OPERATORS],
    pub pitch_envelope: FmEnvelope,
    pub lfo: FmLfo,
    /// Engine extension: voice slots, 1..=16, allocated like synthwave voices.
    pub voices: u8,
    /// Engine extension: copies of the whole voice per note, 1..=8, normalized by
    /// their count. `voices * unison` is at most [`MAX_FM_UNISON_VOICES`].
    pub unison: u8,
    /// Symmetric outer detune of the unison copies, 0..=100 cents.
    pub unison_detune: u16,
    /// Stereo spread of the unison copies, 0..=100 percent.
    pub spread: u8,
    /// Whole-voice pan, -100..=100 percent.
    pub pan: i8,
    /// Portamento, 0..=60000 ms. Requires `voices == 1`.
    pub glide_ms: u32,
    /// Requires `voices == 1`.
    pub legato: bool,
}
impl Default for Fm {
    /// One sine carrier: algorithm 1 with only operator 1 switched on.
    fn default() -> Self {
        let mut operators = [None; FM_OPERATORS];
        operators[0] = Some(FmOperator::default());
        Self {
            algorithm: 1,
            feedback: 0,
            key_sync: true,
            transpose: 0,
            operators,
            pitch_envelope: FmEnvelope::PITCH,
            lfo: FmLfo::default(),
            voices: 8,
            unison: 1,
            unison_detune: 0,
            spread: 0,
            pan: 0,
            glide_ms: 0,
            legato: false,
        }
    }
}
impl Fm {
    /// The routing of this voice's algorithm.
    pub fn routing(&self) -> Result<FmRouting> {
        FmRouting::algorithm(self.algorithm)
    }
    pub fn validate(&self) -> Result<()> {
        let routing = self.routing()?;
        if self.feedback > 7 || !(-24..=24).contains(&self.transpose) {
            return Err(Error::invalid(
                "FM needs feedback 0..=7 and transpose -24..=24",
            ));
        }
        for operator in self.operators.iter().flatten() {
            operator.validate()?;
        }
        let audible =
            (1..=6u8).any(|n| routing.is_carrier(n) && self.operators[n as usize - 1].is_some());
        if !audible {
            return Err(Error::invalid(format!(
                "Algorithm {} needs at least one of its carriers switched on",
                self.algorithm
            )));
        }
        self.pitch_envelope.validate("Pitch")?;
        self.lfo.validate()?;
        if !(1..=16).contains(&self.voices)
            || !(1..=8).contains(&self.unison)
            || self.unison_detune > 100
            || self.spread > 100
            || !(-100..=100).contains(&self.pan)
            || self.glide_ms > 60000
        {
            return Err(Error::invalid(
                "FM needs voices 1..=16, unison 1..=8, unison detune and spread 0..=100, pan -100..=100 and glide 0..=60000 ms",
            ));
        }
        if self.voices as u16 * self.unison as u16 > MAX_FM_UNISON_VOICES {
            return Err(Error::limit(format!(
                "voices * unison must be at most {MAX_FM_UNISON_VOICES}"
            )));
        }
        if (self.glide_ms > 0 || self.legato) && self.voices != 1 {
            return Err(Error::invalid("Glide and legato require voices=1"));
        }
        Ok(())
    }
}

/// A named FM voice in its text form.
///
/// ```
/// use audiobox::synth::FmPatch;
/// let bell: FmPatch = "
/// fm bell algorithm=2 feedback=3
///   op 1 ratio=1 levels=99,0,0,0 rates=95,30,30,40
///   op 2 ratio=3,50 level=78 levels=99,0,0,0 rates=95,45,45,50 velocity=5
/// end".parse()?;
/// assert_eq!(bell.voice.operators[1].unwrap().frequency.ratio(), Some(4.5));
/// assert_eq!(bell.to_string().parse::<FmPatch>()?, bell);
/// # Ok::<(), audiobox::Error>(())
/// ```
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FmPatch {
    /// ASCII letters, digits, `_` and `-`.
    pub name: String,
    /// Instrument volume, 0..=15.
    pub volume: u8,
    pub voice: Fm,
}
impl FmPatch {
    /// Parses one `fm NAME` … `end` block, with positioned errors.
    pub fn parse(text: &str) -> std::result::Result<Self, super::ParseError> {
        super::parse::parse_fm_patch(text)
    }
    pub fn validate(&self) -> Result<()> {
        let name_ok = !self.name.is_empty()
            && self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !name_ok || self.volume > 15 {
            return Err(Error::invalid(
                "FM patch needs a name of ASCII letters, digits, _ or - and volume 0..=15",
            ));
        }
        self.voice.validate()
    }
}
impl std::str::FromStr for FmPatch {
    type Err = Error;
    fn from_str(text: &str) -> Result<Self> {
        Ok(Self::parse(text)?)
    }
}

fn list(values: &[u8; 4]) -> String {
    format!("{},{},{},{}", values[0], values[1], values[2], values[3])
}
fn switch(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// Writes the canonical text: keys at their defaults are left out, and parsing the
/// result gives back an equal patch.
impl fmt::Display for FmPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (v, d) = (&self.voice, Fm::default());
        write!(f, "fm {} algorithm={}", self.name, v.algorithm)?;
        if v.feedback != d.feedback {
            write!(f, " feedback={}", v.feedback)?;
        }
        if v.key_sync != d.key_sync {
            write!(f, " key-sync={}", switch(v.key_sync))?;
        }
        if v.transpose != d.transpose {
            write!(f, " transpose={}", v.transpose)?;
        }
        if self.volume != 15 {
            write!(f, " volume={}", self.volume)?;
        }
        if v.voices != d.voices {
            write!(f, " voices={}", v.voices)?;
        }
        if v.unison != d.unison {
            write!(f, " unison={}", v.unison)?;
        }
        if v.unison_detune != d.unison_detune {
            write!(f, " unison-detune={}", v.unison_detune)?;
        }
        if v.spread != d.spread {
            write!(f, " spread={}", v.spread)?;
        }
        if v.pan != d.pan {
            write!(f, " pan={}", v.pan)?;
        }
        if v.glide_ms != d.glide_ms {
            write!(f, " glide={}", v.glide_ms)?;
        }
        if v.legato != d.legato {
            write!(f, " legato={}", switch(v.legato))?;
        }
        writeln!(f)?;
        if v.pitch_envelope != d.pitch_envelope {
            writeln!(
                f,
                "  pitch rates={} levels={}",
                list(&v.pitch_envelope.rates),
                list(&v.pitch_envelope.levels)
            )?;
        }
        if v.lfo != d.lfo {
            let (l, d) = (&v.lfo, &d.lfo);
            write!(f, "  lfo")?;
            if l.wave != d.wave {
                write!(f, " wave={}", l.wave.name())?;
            }
            if l.speed != d.speed {
                write!(f, " speed={}", l.speed)?;
            }
            if let Some((n, den)) = l.beats {
                write!(f, " beats={n},{den}")?;
            }
            if l.delay != d.delay {
                write!(f, " delay={}", l.delay)?;
            }
            if l.pitch_depth != d.pitch_depth {
                write!(f, " pitch={}", l.pitch_depth)?;
            }
            if l.amplitude_depth != d.amplitude_depth {
                write!(f, " amp={}", l.amplitude_depth)?;
            }
            if l.key_sync != d.key_sync {
                write!(f, " sync={}", switch(l.key_sync))?;
            }
            if l.pitch_sensitivity != d.pitch_sensitivity {
                write!(f, " pitch-sensitivity={}", l.pitch_sensitivity)?;
            }
            if l.seed != d.seed {
                write!(f, " seed={}", l.seed)?;
            }
            writeln!(f)?;
        }
        let d = FmOperator::default();
        for (index, operator) in v.operators.iter().enumerate() {
            let Some(o) = operator else { continue };
            write!(f, "  op {}", index + 1)?;
            match o.frequency {
                FmFrequency::Ratio { coarse, fine } => {
                    match coarse {
                        0 => write!(f, " ratio=0.5")?,
                        c => write!(f, " ratio={c}")?,
                    }
                    if fine != 0 {
                        write!(f, ",{fine}")?;
                    }
                }
                FmFrequency::Fixed { decade, fine } => {
                    write!(f, " fixed={}", 10u32.pow(decade as u32))?;
                    if fine != 0 {
                        write!(f, ",{fine}")?;
                    }
                }
            }
            if o.detune != d.detune {
                write!(f, " detune={}", o.detune)?;
            }
            write!(f, " level={}", o.level)?;
            if o.envelope.rates != d.envelope.rates {
                write!(f, " rates={}", list(&o.envelope.rates))?;
            }
            if o.envelope.levels != d.envelope.levels {
                write!(f, " levels={}", list(&o.envelope.levels))?;
            }
            let (s, ds) = (o.scaling, d.scaling);
            if (s.break_point, s.left_depth, s.right_depth)
                != (ds.break_point, ds.left_depth, ds.right_depth)
            {
                write!(
                    f,
                    " scaling={},{},{}",
                    s.break_point, s.left_depth, s.right_depth
                )?;
            }
            if (s.left_curve, s.right_curve) != (ds.left_curve, ds.right_curve) {
                write!(
                    f,
                    " curves={},{}",
                    s.left_curve.name(),
                    s.right_curve.name()
                )?;
            }
            if o.rate_scaling != d.rate_scaling {
                write!(f, " rate-scaling={}", o.rate_scaling)?;
            }
            if o.velocity != d.velocity {
                write!(f, " velocity={}", o.velocity)?;
            }
            if o.amplitude_sensitivity != d.amplitude_sensitivity {
                write!(f, " amp-sensitivity={}", o.amplitude_sensitivity)?;
            }
            writeln!(f)?;
        }
        writeln!(f, "end")
    }
}
