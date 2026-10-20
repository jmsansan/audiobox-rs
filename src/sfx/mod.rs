//! A parametric sound-effect generator in the style of the [`synth`](crate::synth)
//! module. Requires the `sfx` feature, which enables `synth`.
//!
//! An [`Effect`] is a small set of numbers: a waveform, a pitch with slides, vibrato
//! and an arpeggio step, an envelope with punch, a duty sweep, swept low- and
//! high-pass filters, a repeat period and a volume. It renders deterministically
//! into mono [`Audio`], reads and writes a short text form, and can be drawn from a
//! seed by a [`Preset`].
//!
//! ```
//! use audiobox::sfx::{Effect, Preset};
//! let coin: Effect = "wave=pulse duty=50 frequency=1046.5 sustain=0.04 punch=0.5
//!                     decay=0.25 arpeggio=5 arpeggio-time=0.06".parse()?;
//! let audio = coin.render(44_100)?;
//! assert_eq!(audio.frames(), 12_789);
//!
//! // Try seeds until one sounds right, then keep its text.
//! let jump = Effect::preset(Preset::Jump, 7);
//! assert_eq!(jump.to_string().parse::<Effect>()?, jump);
//! # Ok::<(), audiobox::Error>(())
//! ```
//!
//! # Parameters and text form
//!
//! The text form is whitespace-separated `key=value` pairs, in any order, on one
//! line or several; `;` starts a comment that runs to the end of the line. Missing
//! keys keep their defaults. [`Effect`]'s `Display` writes the same form, and
//! parsing it gives back an equal effect: numbers are written in Rust's shortest
//! form that reads back to the same `f64`.
//!
//! | Key | Field | Unit | Range | Default |
//! | --- | --- | --- | --- | --- |
//! | `wave` | `waveform` | `pulse`, `triangle`, `saw`, `noise` | | `pulse` |
//! | `duty` | `waveform` | pulse duty in percent | `12.5`, `25`, `50`, `75` | `50` |
//! | `noise` | `waveform` | noise mode | `long`, `short` | `long` |
//! | `frequency` | `frequency` | Hz | 10..=20000 | 440 |
//! | `slide` | `slide` | octaves per second | -64..=64 | 0 |
//! | `slide-change` | `slide_change` | octaves per second, per second | -256..=256 | 0 |
//! | `vibrato-depth` | `vibrato_depth` | semitones | 0..=24 | 0 |
//! | `vibrato-speed` | `vibrato_speed` | Hz | 0..=100 | 0 |
//! | `attack` | `attack` | seconds | 0..=10 | 0 |
//! | `sustain` | `sustain` | seconds | 0..=10 | 0.1 |
//! | `punch` | `punch` | extra level at the start of the sustain | 0..=1 | 0 |
//! | `decay` | `decay` | seconds | 0..=10 | 0.2 |
//! | `duty-sweep` | `duty_sweep` | duty change per second (pulse only) | -10..=10 | 0 |
//! | `low-pass` | `low_pass` | cutoff in Hz, 0 for off | 0 or 10..=20000 | 0 |
//! | `low-pass-sweep` | `low_pass_sweep` | octaves per second | -16..=16 | 0 |
//! | `high-pass` | `high_pass` | cutoff in Hz, 0 for off | 0 or 10..=20000 | 0 |
//! | `high-pass-sweep` | `high_pass_sweep` | octaves per second | -16..=16 | 0 |
//! | `arpeggio` | `arpeggio` | semitones | -48..=48 | 0 |
//! | `arpeggio-time` | `arpeggio_time` | seconds | 0..=10 | 0 |
//! | `repeat` | `repeat` | seconds, 0 for off | 0 or 0.01..=10 | 0 |
//! | `volume` | `volume` | linear | 0..=1 | 0.5 |
//!
//! `attack + sustain + decay` is the effect's duration. It must be more than 0 and
//! at most [`MAX_DURATION`] seconds. Values out of range, and `duty` or `duty-sweep`
//! without a pulse or `noise` without noise, are rejected with
//! [`ErrorCode::InvalidArgument`](crate::ErrorCode); in text, the error is a
//! [`ParseError`] with the line and column of the value.
//!
//! # Sound
//!
//! At time `t` seconds since the start, or since the last repeat, the frequency is
//! `frequency × 2^(slide·t + slide_change·t²/2)`, raised by `arpeggio` semitones once
//! `t ≥ arpeggio_time`, and modulated by a vibrato of `vibrato_depth` semitones at
//! `vibrato_speed` Hz. It is limited to 0..=sample_rate / 2. The pulse duty starts at
//! `duty` and changes by `duty_sweep` per second, limited to 1..=99%. Noise uses the
//! synthesizer's shift register, clocked at 93 times the frequency.
//!
//! `repeat` restarts the pitch, slides, arpeggio and duty every `repeat` seconds;
//! the envelope, the vibrato and the filters continue.
//!
//! The envelope rises linearly from 0 to 1 over `attack`, then holds for `sustain`
//! starting at `1 + punch` and falling linearly to 1, then falls linearly to 0 over
//! `decay`. Each stage is rounded to whole samples, so an effect lasts
//! `round(attack·rate) + round(sustain·rate) + round(decay·rate)` frames. The output
//! is the filtered oscillator times the envelope times `volume`, so it peaks at
//! `volume × (1 + punch)`.
//!
//! Both filters are 2-pole (12 dB per octave) Butterworth state-variable filters: the
//! low-pass passes frequencies below its cutoff and the high-pass those above, and
//! each attenuates its cutoff by 3 dB. A sweep moves a cutoff by that many octaves per
//! second from the start of the effect, within 10 Hz..=0.45 × the sample rate.
//!
//! Like the synthesizer, rendering uses only basic IEEE arithmetic and the crate's own
//! series for `2^x`, sine and tangent, so an effect gives the same samples on every
//! platform.
mod preset;
mod text;

pub use crate::synth::ParseError;
pub use preset::Preset;

use crate::math::{cos_turns, exp2, sin_turns};
use crate::synth::{Aliasing, Duty, NoiseMode, Oscillator, Waveform};
use crate::{Audio, Error, Limits, Result};

/// The longest effect, in seconds.
pub const MAX_DURATION: f64 = 10.0;

/// The parameters of a sound effect. See the [module documentation](self) for their
/// units, ranges and meaning.
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub waveform: Waveform,
    pub frequency: f64,
    pub slide: f64,
    pub slide_change: f64,
    pub vibrato_depth: f64,
    pub vibrato_speed: f64,
    pub attack: f64,
    pub sustain: f64,
    pub punch: f64,
    pub decay: f64,
    pub duty_sweep: f64,
    pub low_pass: f64,
    pub low_pass_sweep: f64,
    pub high_pass: f64,
    pub high_pass_sweep: f64,
    pub arpeggio: f64,
    pub arpeggio_time: f64,
    pub repeat: f64,
    pub volume: f64,
}
impl Default for Effect {
    fn default() -> Self {
        Self {
            waveform: Waveform::Pulse(Duty::Half),
            frequency: 440.0,
            slide: 0.0,
            slide_change: 0.0,
            vibrato_depth: 0.0,
            vibrato_speed: 0.0,
            attack: 0.0,
            sustain: 0.1,
            punch: 0.0,
            decay: 0.2,
            duty_sweep: 0.0,
            low_pass: 0.0,
            low_pass_sweep: 0.0,
            high_pass: 0.0,
            high_pass_sweep: 0.0,
            arpeggio: 0.0,
            arpeggio_time: 0.0,
            repeat: 0.0,
            volume: 0.5,
        }
    }
}

/// A numeric parameter: its text key, accessors and range.
pub(crate) struct Field {
    pub key: &'static str,
    pub get: fn(&Effect) -> f64,
    pub set: fn(&mut Effect, f64),
    pub min: f64,
    pub max: f64,
    /// Zero is allowed below `min`, meaning off.
    pub zero_is_off: bool,
    /// Written by `Display` even at its default.
    pub always: bool,
}
impl Field {
    pub fn check(&self, value: f64) -> Result<()> {
        if value.is_finite()
            && ((self.min..=self.max).contains(&value) || (self.zero_is_off && value == 0.0))
        {
            return Ok(());
        }
        let off = if self.zero_is_off { "0 or " } else { "" };
        Err(Error::invalid(format!(
            "{} must be {off}{}..={}, not {value}",
            self.key, self.min, self.max
        )))
    }
}

macro_rules! fields {
    ($($key:literal $field:ident $min:literal $max:literal $zero:literal $always:literal),* $(,)?) => {
        pub(crate) const FIELDS: &[Field] = &[$(Field {
            key: $key,
            get: |e| e.$field,
            set: |e, v| e.$field = v,
            min: $min,
            max: $max,
            zero_is_off: $zero,
            always: $always,
        }),*];
    };
}
fields! {
    "frequency" frequency 10.0 20000.0 false true,
    "slide" slide -64.0 64.0 false false,
    "slide-change" slide_change -256.0 256.0 false false,
    "vibrato-depth" vibrato_depth 0.0 24.0 false false,
    "vibrato-speed" vibrato_speed 0.0 100.0 false false,
    "attack" attack 0.0 10.0 false true,
    "sustain" sustain 0.0 10.0 false true,
    "punch" punch 0.0 1.0 false false,
    "decay" decay 0.0 10.0 false true,
    "duty-sweep" duty_sweep -10.0 10.0 false false,
    "low-pass" low_pass 10.0 20000.0 true false,
    "low-pass-sweep" low_pass_sweep -16.0 16.0 false false,
    "high-pass" high_pass 10.0 20000.0 true false,
    "high-pass-sweep" high_pass_sweep -16.0 16.0 false false,
    "arpeggio" arpeggio -48.0 48.0 false false,
    "arpeggio-time" arpeggio_time 0.0 10.0 false false,
    "repeat" repeat 0.01 10.0 true false,
    "volume" volume 0.0 1.0 false true,
}

impl Effect {
    /// Draws an effect of the given kind from a seed. The same kind and seed always
    /// give the same parameters, rounded to three decimals so the text stays short.
    pub fn preset(kind: Preset, seed: u64) -> Self {
        preset::generate(kind, seed)
    }
    /// Parses the text form described in the [module documentation](self).
    pub fn parse(text: &str) -> std::result::Result<Self, ParseError> {
        text::parse(text)
    }
    /// `attack + sustain + decay`, in seconds.
    pub fn duration(&self) -> f64 {
        self.attack + self.sustain + self.decay
    }
    /// Checks every parameter's range and the duration.
    pub fn validate(&self) -> Result<()> {
        for field in FIELDS {
            field.check((field.get)(self))?;
        }
        if self.duty_sweep != 0.0 && !matches!(self.waveform, Waveform::Pulse(_)) {
            return Err(Error::invalid("duty-sweep needs a pulse waveform"));
        }
        let duration = self.duration();
        if !(duration > 0.0 && duration <= MAX_DURATION) {
            return Err(Error::invalid(format!(
                "attack + sustain + decay must be more than 0 and at most {MAX_DURATION} seconds, not {duration}"
            )));
        }
        Ok(())
    }
    /// Renders with band-limited oscillators.
    pub fn render(&self, sample_rate: u32) -> Result<Audio> {
        self.render_with(sample_rate, Aliasing::BandLimited)
    }
    /// Renders the effect into mono audio.
    pub fn render_with(&self, sample_rate: u32, aliasing: Aliasing) -> Result<Audio> {
        self.validate()?;
        Limits::default().check(0, 1, sample_rate)?;
        let rate = sample_rate as f64;
        let samples = |seconds: f64| (seconds * rate).round() as usize;
        let (attack, sustain, decay) = (
            samples(self.attack),
            samples(self.sustain),
            samples(self.decay),
        );
        let total = attack + sustain + decay;
        Limits::default().check(total, 1, sample_rate)?;
        let repeat = if self.repeat > 0.0 {
            samples(self.repeat).max(1)
        } else {
            usize::MAX
        };
        let arpeggio_at = samples(self.arpeggio_time);
        let mut oscillator = Oscillator::new(aliasing);
        let mut low_pass = Svf::default();
        let mut high_pass = Svf::default();
        let mut out = Vec::with_capacity(total);
        for n in 0..total {
            let m = n % repeat;
            let t = m as f64 / rate;
            let mut octaves = self.slide * t + self.slide_change * t * t / 2.0;
            if m >= arpeggio_at {
                octaves += self.arpeggio / 12.0;
            }
            if self.vibrato_depth > 0.0 {
                octaves +=
                    self.vibrato_depth / 12.0 * sin_turns(self.vibrato_speed * n as f64 / rate);
            }
            let frequency = self.frequency * exp2(octaves);
            let mut value = match self.waveform {
                Waveform::Pulse(duty) => oscillator.next_pulse(
                    duty.fraction() + self.duty_sweep * t,
                    frequency,
                    sample_rate,
                ),
                waveform => oscillator.next(waveform, frequency, sample_rate),
            };
            let seconds = n as f64 / rate;
            if self.low_pass > 0.0 {
                let g = Svf::coefficient(self.low_pass, self.low_pass_sweep, seconds, rate);
                value = low_pass.process(value, g).0;
            }
            if self.high_pass > 0.0 {
                let g = Svf::coefficient(self.high_pass, self.high_pass_sweep, seconds, rate);
                value = high_pass.process(value, g).1;
            }
            let level = if n < attack {
                n as f64 / attack as f64
            } else if n < attack + sustain {
                1.0 + self.punch * (1.0 - (n - attack) as f64 / sustain as f64)
            } else {
                1.0 - (n - attack - sustain) as f64 / decay as f64
            };
            out.push((value * level * self.volume) as f32);
        }
        Audio::new(vec![out], sample_rate)
    }
}

/// Topology-preserving state-variable filter (Zavalishin), with Butterworth damping.
#[derive(Clone, Copy, Debug, Default)]
struct Svf {
    ic1: f64,
    ic2: f64,
}
impl Svf {
    /// `tan(π·fc/fs)` for a cutoff swept by `sweep` octaves per second.
    fn coefficient(cutoff: f64, sweep: f64, seconds: f64, rate: f64) -> f64 {
        let fc = (cutoff * exp2(sweep * seconds)).clamp(10.0, 0.45 * rate);
        let half_turns = fc / rate / 2.0;
        sin_turns(half_turns) / cos_turns(half_turns)
    }
    /// Returns the low-pass and high-pass outputs.
    fn process(&mut self, x: f64, g: f64) -> (f64, f64) {
        const K: f64 = std::f64::consts::SQRT_2;
        let a1 = 1.0 / (1.0 + g * (g + K));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, x - K * v1 - v2)
    }
}

impl std::str::FromStr for Effect {
    type Err = Error;
    fn from_str(text: &str) -> Result<Self> {
        Ok(Self::parse(text)?)
    }
}

impl std::fmt::Display for Effect {
    /// Writes one `key=value` per line: the waveform, the frequency, the envelope and
    /// the volume, then every other parameter that differs from its default.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.waveform {
            Waveform::Pulse(duty) => {
                let duty = match duty {
                    Duty::Eighth => "12.5",
                    Duty::Quarter => "25",
                    Duty::Half => "50",
                    Duty::ThreeQuarters => "75",
                };
                writeln!(f, "wave=pulse duty={duty}")?;
            }
            Waveform::Triangle => writeln!(f, "wave=triangle")?,
            Waveform::Sawtooth => writeln!(f, "wave=saw")?,
            Waveform::Noise(NoiseMode::Long) => writeln!(f, "wave=noise noise=long")?,
            Waveform::Noise(NoiseMode::Short) => writeln!(f, "wave=noise noise=short")?,
        }
        let default = Effect::default();
        for field in FIELDS {
            let value = (field.get)(self);
            if field.always || value != (field.get)(&default) {
                writeln!(f, "{}={value}", field.key)?;
            }
        }
        Ok(())
    }
}
