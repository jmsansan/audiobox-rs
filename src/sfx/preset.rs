use super::Effect;
use crate::synth::{Duty, NoiseMode, Waveform};

/// Kinds of effect that [`Effect::preset`] draws from a seed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Preset {
    /// A coin or item: a short, bright tone, often with a jump up in pitch.
    Pickup,
    /// A shot: a fast downward slide.
    Laser,
    /// Filtered noise that falls and fades.
    Explosion,
    /// A rising tone, often repeated or with vibrato.
    PowerUp,
    /// A short, falling thud of noise or a harsh tone.
    Hit,
    /// A quick upward slide.
    Jump,
    /// A short beep for menus.
    Blip,
}
impl Preset {
    pub const ALL: [Preset; 7] = [
        Self::Pickup,
        Self::Laser,
        Self::Explosion,
        Self::PowerUp,
        Self::Hit,
        Self::Jump,
        Self::Blip,
    ];
}

/// SplitMix64: a fixed, portable generator, so a seed gives the same effect everywhere.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// A value in `low..=high`, rounded to three decimals.
    fn range(&mut self, low: f64, high: f64) -> f64 {
        ((low + (high - low) * self.unit()) * 1000.0).round() / 1000.0
    }
    fn chance(&mut self, probability: f64) -> bool {
        self.unit() < probability
    }
    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[(self.next() % items.len() as u64) as usize]
    }
}

pub(super) fn generate(kind: Preset, seed: u64) -> Effect {
    let index = Preset::ALL.iter().position(|&k| k == kind).unwrap_or(0) as u64;
    let mut r = Rng(seed ^ index.wrapping_mul(0xd6e8_feb8_6659_fd93));
    let duties = [Duty::Eighth, Duty::Quarter, Duty::Half];
    let mut e = Effect::default();
    match kind {
        Preset::Pickup => {
            e.waveform = if r.chance(0.75) {
                Waveform::Pulse(r.pick(&duties))
            } else {
                Waveform::Sawtooth
            };
            e.frequency = r.range(700.0, 1700.0);
            e.sustain = r.range(0.02, 0.1);
            e.punch = r.range(0.3, 0.7);
            e.decay = r.range(0.1, 0.35);
            if r.chance(0.6) {
                e.arpeggio = r.pick(&[3.0, 4.0, 5.0, 7.0, 12.0]);
                e.arpeggio_time = r.range(0.03, 0.1);
            }
        }
        Preset::Laser => {
            let pulse = r.chance(0.6);
            e.waveform = if pulse {
                Waveform::Pulse(r.pick(&duties))
            } else {
                r.pick(&[Waveform::Sawtooth, Waveform::Triangle])
            };
            e.frequency = r.range(500.0, 2000.0);
            e.slide = r.range(-8.0, -2.5);
            e.slide_change = if r.chance(0.3) {
                r.range(0.0, 20.0)
            } else {
                0.0
            };
            if pulse && r.chance(0.5) {
                e.duty_sweep = r.range(-2.0, 2.0);
            }
            e.sustain = r.range(0.04, 0.15);
            e.punch = if r.chance(0.5) {
                r.range(0.0, 0.4)
            } else {
                0.0
            };
            e.decay = r.range(0.05, 0.2);
            if r.chance(0.3) {
                e.high_pass = r.range(100.0, 800.0);
            }
        }
        Preset::Explosion => {
            e.waveform = Waveform::Noise(NoiseMode::Long);
            e.frequency = r.range(30.0, 250.0);
            e.slide = r.range(-2.0, 0.0);
            e.sustain = r.range(0.08, 0.3);
            e.punch = r.range(0.2, 0.8);
            e.decay = r.range(0.3, 0.9);
            if r.chance(0.6) {
                e.low_pass = r.range(800.0, 6000.0);
                e.low_pass_sweep = r.range(-3.0, 0.0);
            }
            if r.chance(0.3) {
                e.vibrato_depth = r.range(0.5, 3.0);
                e.vibrato_speed = r.range(4.0, 14.0);
            }
            e.volume = 0.6;
        }
        Preset::PowerUp => {
            e.waveform = if r.chance(0.6) {
                Waveform::Pulse(r.pick(&duties))
            } else {
                Waveform::Sawtooth
            };
            e.frequency = r.range(250.0, 600.0);
            e.slide = r.range(0.8, 3.0);
            if r.chance(0.5) {
                e.repeat = r.range(0.08, 0.2);
            } else {
                e.vibrato_depth = r.range(0.2, 1.0);
                e.vibrato_speed = r.range(8.0, 20.0);
            }
            e.sustain = r.range(0.15, 0.4);
            e.decay = r.range(0.1, 0.4);
        }
        Preset::Hit => {
            let noise = r.chance(0.5);
            e.waveform = if noise {
                Waveform::Noise(NoiseMode::Long)
            } else {
                r.pick(&[Waveform::Sawtooth, Waveform::Pulse(Duty::Half)])
            };
            e.frequency = if noise {
                r.range(40.0, 200.0)
            } else {
                r.range(100.0, 500.0)
            };
            e.slide = r.range(-6.0, -1.5);
            e.sustain = r.range(0.01, 0.05);
            e.punch = r.range(0.0, 0.5);
            e.decay = r.range(0.05, 0.2);
            if r.chance(0.5) {
                e.high_pass = r.range(80.0, 500.0);
            }
        }
        Preset::Jump => {
            e.waveform = Waveform::Pulse(r.pick(&[Duty::Quarter, Duty::Half]));
            e.frequency = r.range(250.0, 600.0);
            e.slide = r.range(1.0, 3.0);
            e.sustain = r.range(0.05, 0.15);
            e.decay = r.range(0.1, 0.25);
            if r.chance(0.4) {
                e.low_pass = r.range(2000.0, 8000.0);
            }
            if r.chance(0.4) {
                e.high_pass = r.range(100.0, 400.0);
            }
        }
        Preset::Blip => {
            e.waveform = if r.chance(0.7) {
                Waveform::Pulse(r.pick(&[Duty::Quarter, Duty::Half]))
            } else {
                Waveform::Triangle
            };
            e.frequency = r.range(600.0, 1600.0);
            e.sustain = r.range(0.02, 0.08);
            e.decay = r.range(0.01, 0.05);
            if r.chance(0.3) {
                e.high_pass = r.range(100.0, 400.0);
            }
            e.volume = 0.4;
        }
    }
    e
}
