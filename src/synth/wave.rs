use std::str::FromStr;

use crate::{Error, Result};

/// Pulse duty cycle: the fraction of each period spent high.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Duty {
    /// 12.5%.
    Eighth,
    /// 25%.
    Quarter,
    /// 50%, a square wave.
    #[default]
    Half,
    /// 75%. Sounds like 25% inverted.
    ThreeQuarters,
}
impl Duty {
    pub fn fraction(self) -> f64 {
        match self {
            Self::Eighth => 0.125,
            Self::Quarter => 0.25,
            Self::Half => 0.5,
            Self::ThreeQuarters => 0.75,
        }
    }
}
impl FromStr for Duty {
    type Err = Error;
    /// Accepts the percentages `12.5`, `25`, `50` and `75`.
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "12.5" | "12" => Ok(Self::Eighth),
            "25" => Ok(Self::Quarter),
            "50" => Ok(Self::Half),
            "75" => Ok(Self::ThreeQuarters),
            _ => Err(Error::invalid(format!(
                "Duty must be 12.5, 25, 50 or 75, not {s}"
            ))),
        }
    }
}

/// Feedback tap of the noise generator's 15-bit linear-feedback shift register.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum NoiseMode {
    /// Taps bits 0 and 1: a 32,767-step sequence that sounds like hiss.
    #[default]
    Long,
    /// Taps bits 0 and 6: a 93-step sequence with a metallic, pitched tone.
    Short,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Waveform {
    Pulse(Duty),
    Triangle,
    Sawtooth,
    /// Noise clocked at 93 times the note's frequency, so the short mode's
    /// 93-step period sounds at the note's pitch.
    Noise(NoiseMode),
}
impl Default for Waveform {
    fn default() -> Self {
        Self::Pulse(Duty::Half)
    }
}

/// How oscillators treat harmonics above the Nyquist frequency.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Aliasing {
    /// PolyBLEP steps for pulse and sawtooth, PolyBLAMP corners for triangle, and
    /// noise averaged over each sample. Aliases are strongly reduced, not removed.
    #[default]
    BandLimited,
    /// Raw waveforms sampled at each frame. Harsh, aliased "period" sound.
    Naive,
}

/// Linear-feedback shift register of 15 bits, starting at 1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lfsr {
    state: u16,
    mode: NoiseMode,
}
impl Lfsr {
    pub fn new(mode: NoiseMode) -> Self {
        Self { state: 1, mode }
    }
    pub fn mode(&self) -> NoiseMode {
        self.mode
    }
    /// Changes the tap. The register restarts at 1, so the short mode always runs its 93-step cycle.
    pub fn set_mode(&mut self, mode: NoiseMode) {
        if mode != self.mode {
            *self = Self::new(mode);
        }
    }
    pub fn state(&self) -> u16 {
        self.state
    }
    /// +1 while bit 0 is clear, otherwise -1.
    pub fn output(&self) -> f64 {
        if self.state & 1 == 0 { 1.0 } else { -1.0 }
    }
    pub fn step(&mut self) {
        let tap = match self.mode {
            NoiseMode::Long => 1,
            NoiseMode::Short => 6,
        };
        let feedback = (self.state ^ (self.state >> tap)) & 1;
        self.state = (self.state >> 1) | (feedback << 14);
    }
}

const ONE: u64 = 1 << 32;
const FRACTION: u64 = ONE - 1;

/// A phase-continuous oscillator. Changing the waveform or frequency between
/// samples keeps the phase, so note changes do not click.
///
/// The phase is a 32.32 fixed-point accumulator and every sample is computed with
/// basic IEEE arithmetic only, so the output is identical on every platform.
#[derive(Clone, Debug)]
pub struct Oscillator {
    phase: u64,
    lfsr: Lfsr,
    aliasing: Aliasing,
}
impl Oscillator {
    pub fn new(aliasing: Aliasing) -> Self {
        Self {
            phase: 0,
            lfsr: Lfsr::new(NoiseMode::Long),
            aliasing,
        }
    }
    /// Returns the sample at the current phase, in -1..=1 for naive waveforms, then
    /// advances by one frame. `frequency` is clamped to 0..=sample_rate / 2.
    pub fn next(&mut self, waveform: Waveform, frequency: f64, sample_rate: u32) -> f64 {
        match waveform {
            Waveform::Pulse(duty) => {
                self.tone(Shape::Pulse(duty.fraction()), frequency, sample_rate)
            }
            Waveform::Triangle => self.tone(Shape::Triangle, frequency, sample_rate),
            Waveform::Sawtooth => self.tone(Shape::Sawtooth, frequency, sample_rate),
            Waveform::Noise(mode) => {
                let rate = sample_rate.max(1) as f64;
                self.lfsr.set_mode(mode);
                let increment =
                    (clamp_frequency(frequency, rate) * 93.0 / rate * ONE as f64).round() as u64;
                self.noise(increment)
            }
        }
    }
    /// Like [`Oscillator::next`] for a pulse of any duty, clamped to 0.01..=0.99.
    /// The four [`Duty`] values give the same samples as [`Waveform::Pulse`].
    pub fn next_pulse(&mut self, duty: f64, frequency: f64, sample_rate: u32) -> f64 {
        let duty = if duty.is_finite() {
            duty.clamp(0.01, 0.99)
        } else {
            0.5
        };
        self.tone(Shape::Pulse(duty), frequency, sample_rate)
    }
    /// Separate fixed register clock for opt-in period-table noise.
    pub(super) fn clocked_noise(&mut self, mode: NoiseMode, clock: f64, sample_rate: u32) -> f64 {
        self.lfsr.set_mode(mode);
        let increment = (clock / sample_rate as f64 * ONE as f64).round() as u64;
        self.noise(increment)
    }
    fn tone(&mut self, shape: Shape, frequency: f64, sample_rate: u32) -> f64 {
        let rate = sample_rate.max(1) as f64;
        let increment = (clamp_frequency(frequency, rate) / rate * ONE as f64).round() as u64;
        let t = (self.phase & FRACTION) as f64 / ONE as f64;
        let dt = increment as f64 / ONE as f64;
        self.phase = (self.phase & FRACTION) + increment;
        let band_limited = self.aliasing == Aliasing::BandLimited && dt > 0.0;
        match shape {
            Shape::Pulse(d) => {
                let mut value = if t < d { 1.0 } else { -1.0 };
                if band_limited {
                    value += poly_blep(t, dt) - poly_blep(wrap(t - d), dt);
                }
                value
            }
            Shape::Sawtooth => {
                let mut value = 2.0 * t - 1.0;
                if band_limited {
                    value -= poly_blep(t, dt);
                }
                value
            }
            Shape::Triangle => {
                let mut value = if t < 0.5 {
                    4.0 * t - 1.0
                } else {
                    3.0 - 4.0 * t
                };
                if band_limited {
                    value += 8.0 * dt * (poly_blamp(t, dt) - poly_blamp(wrap(t - 0.5), dt));
                }
                value
            }
        }
    }
    fn noise(&mut self, increment: u64) -> f64 {
        let start = self.phase & FRACTION;
        let end = start + increment;
        let steps = end >> 32;
        let value = if self.aliasing == Aliasing::Naive || increment == 0 {
            let value = self.lfsr.output();
            for _ in 0..steps {
                self.lfsr.step();
            }
            value
        } else {
            let mut sum = self.lfsr.output() * (end.min(ONE) - start) as f64;
            for k in 1..=steps {
                self.lfsr.step();
                let from = k * ONE;
                sum += self.lfsr.output() * (end.min(from + ONE) - from) as f64;
            }
            sum / increment as f64
        };
        self.phase = end & FRACTION;
        value
    }
}

#[derive(Clone, Copy)]
enum Shape {
    Pulse(f64),
    Triangle,
    Sawtooth,
}

fn clamp_frequency(frequency: f64, rate: f64) -> f64 {
    if frequency.is_finite() {
        frequency.clamp(0.0, rate / 2.0)
    } else {
        0.0
    }
}

fn wrap(t: f64) -> f64 {
    if t < 0.0 { t + 1.0 } else { t }
}

/// Residual of a band-limited step of height 2, for a phase `t` near the step at 0.
fn poly_blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let x = t / dt;
        2.0 * x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + 2.0 * x + 1.0
    } else {
        0.0
    }
}

/// Residual of a band-limited corner whose slope rises by one unit per sample,
/// for a phase `t` near the corner at 0: the integral of the PolyBLEP residual.
fn poly_blamp(t: f64, dt: f64) -> f64 {
    let x = if t < dt {
        1.0 - t / dt
    } else if t > 1.0 - dt {
        1.0 - (1.0 - t) / dt
    } else {
        return 0.0;
    };
    x * x * x / 6.0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lfsr_periods() {
        for (mode, period) in [(NoiseMode::Long, 32767), (NoiseMode::Short, 93)] {
            let mut lfsr = Lfsr::new(mode);
            let mut n = 0;
            loop {
                lfsr.step();
                n += 1;
                if lfsr.state() == 1 {
                    break;
                }
            }
            assert_eq!(n, period);
        }
    }
}
