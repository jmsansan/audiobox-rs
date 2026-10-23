//! Finite, synthesized one-shots. No recorded samples or platform transcendental math.
use super::wave::{Aliasing, Oscillator, Waveform};
use crate::math::{exp, exp2, sin_turns};
use crate::{Audio, Error, Limits, Result};

/// Character of the oscillators and noise, rather than a model of one console.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DrumKit {
    Chiptune,
    Electronic,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DrumKind {
    Kick,
    Snare,
    ClosedHat,
    OpenHat,
    Tom,
    Clap,
    Crash,
}

/// A drum preset with integer, portable controls. Notes transpose pitched components
/// relative to C4; noise filters stay fixed. Envelope times are finite lengths, not
/// exponential time constants. Each trigger resets oscillators, filters and seed.
/// A component shorter than its 1 ms attack (including a one-frame envelope at low
/// sample rates) can be silent. Presets use longer envelopes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Drum {
    pub kind: DrumKind,
    pub kit: DrumKit,
    /// Fundamental/metal-bank base at C4, 20..=20000 Hz, clamped below Nyquist.
    pub pitch_hz: u16,
    /// Tone envelope length, 0..=10000 ms. Zero silences that component.
    pub tone_ms: u32,
    /// Noise envelope length, 0..=10000 ms. Clap adds 30 ms for its initial bursts.
    pub noise_ms: u32,
    /// Initial pitch above the base, 0..=4800 cents, falling linearly in cents.
    pub sweep_cents: u16,
    /// Time to reach the base pitch, 0..=1000 ms. Zero starts at the base.
    pub sweep_ms: u32,
    /// Noise blend, 0..=100 percent; the tone receives the complementary amount.
    pub noise_percent: u8,
    /// A separate 3 ms filtered-noise transient, 0..=100 percent.
    pub click_percent: u8,
    /// Fixed noise seed. Zero maps to one; the chiptune register uses its low 15 bits.
    pub seed: u32,
}
impl Drum {
    pub fn new(kind: DrumKind, kit: DrumKit) -> Self {
        use DrumKind::*;
        let (pitch, tone, noise, sweep, sweep_ms, blend, click) = match kind {
            Kick => (55, 400, 0, 2400, 35, 0, 25),
            Snare => (180, 100, 220, 0, 0, 70, 10),
            ClosedHat => (3400, 45, 60, 0, 0, 80, 0),
            OpenHat => (3400, 350, 450, 0, 0, 80, 0),
            Tom => (130, 420, 80, 1200, 50, 15, 10),
            Clap => (1800, 0, 180, 0, 0, 100, 0),
            Crash => (2300, 1200, 1800, 0, 0, 65, 0),
        };
        let mut drum = Self {
            kind,
            kit,
            pitch_hz: pitch,
            tone_ms: tone,
            noise_ms: noise,
            sweep_cents: sweep,
            sweep_ms,
            noise_percent: blend,
            click_percent: click,
            seed: 1,
        };
        if kit == DrumKit::Chiptune {
            drum.tone_ms = tone * 3 / 4;
            drum.noise_ms = noise * 3 / 4;
            drum.click_percent = click / 2;
            if matches!(kind, ClosedHat | OpenHat | Crash) {
                drum.noise_percent = 100;
            }
        }
        drum
    }
    pub fn validate(&self) -> Result<()> {
        if !(20..=20_000).contains(&self.pitch_hz)
            || self.tone_ms > 10_000
            || self.noise_ms > 10_000
            || self.sweep_ms > 1000
            || self.sweep_cents > 4800
            || self.noise_percent > 100
            || self.click_percent > 100
        {
            return Err(Error::invalid(
                "Drum pitch, envelope, sweep or blend is outside its documented range",
            ));
        }
        Ok(())
    }
    /// Finite duration including a clap's bursts and the click, excluding muted components.
    pub fn duration_ms(&self) -> u32 {
        let tone = if self.noise_percent < 100 && self.kind != DrumKind::Clap {
            self.tone_ms
        } else {
            0
        };
        let noise = if self.noise_percent > 0 && self.noise_ms > 0 {
            self.noise_ms
                .saturating_add(if self.kind == DrumKind::Clap { 30 } else { 0 })
        } else {
            0
        };
        tone.max(noise)
            .max(if self.click_percent > 0 { 3 } else { 0 })
    }
    /// Renders one hit at C4, with velocity 0..=127, using band-limited chip tones.
    /// This offline convenience allocates its output; score `Synth::fill` does not.
    pub fn render(&self, sample_rate: u32, velocity: u8) -> Result<Audio> {
        self.validate()?;
        if velocity > 127 {
            return Err(Error::invalid("Velocity must be 0..=127"));
        }
        Limits::default().check(0, 1, sample_rate)?;
        let plan = DrumPlan::new(*self, sample_rate);
        let mut voice = DrumVoice::new(Aliasing::BandLimited);
        voice.trigger(&plan, 1.0);
        let mut samples = vec![0.0; plan.frames as usize];
        for sample in &mut samples {
            *sample = (voice.sample(&plan) * (velocity as f64 / 127.0)) as f32;
        }
        Audio::new(vec![samples], sample_rate)
    }
}

#[derive(Clone, Debug)]
pub(super) struct DrumPlan {
    settings: Drum,
    pub(super) frames: u64,
    rate: u32,
    tone: u64,
    noise: u64,
    sweep: u64,
    attack: u64,
    click: u64,
    burst: u64,
    offsets: [u64; 4],
    low: f64,
    high: f64,
}
impl DrumPlan {
    pub(super) fn new(settings: Drum, rate: u32) -> Self {
        let samples = |ms: u32| (ms as u64 * rate as u64 + 500) / 1000;
        let (low, high) = match settings.kind {
            DrumKind::Kick | DrumKind::Tom => (8000.0, 1500.0),
            DrumKind::Snare => (8000.0, 700.0),
            DrumKind::Clap => (6500.0, 1000.0),
            _ => (14_000.0, 5000.0),
        };
        let alpha =
            |hz: f64| exp(-std::f64::consts::TAU * hz.min(rate as f64 * 0.45) / rate as f64);
        let tone = samples(settings.tone_ms);
        let noise = samples(settings.noise_ms);
        let click = samples(3).max(1);
        let offsets = [0, samples(10), samples(20), samples(30)];
        let tone_end = if settings.noise_percent < 100 && settings.kind != DrumKind::Clap {
            tone
        } else {
            0
        };
        let noise_end = if settings.noise_percent > 0 && noise > 0 {
            noise
                + if settings.kind == DrumKind::Clap {
                    offsets[3]
                } else {
                    0
                }
        } else {
            0
        };
        Self {
            frames: tone_end
                .max(noise_end)
                .max(if settings.click_percent > 0 { click } else { 0 }),
            rate,
            tone,
            noise,
            sweep: samples(settings.sweep_ms),
            attack: samples(1).max(1),
            click,
            burst: samples(8).max(1),
            offsets,
            low: alpha(low),
            high: alpha(high),
            settings,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct DrumVoice {
    age: u64,
    frames: u64,
    aliasing: Aliasing,
    oscillator: Oscillator,
    phases: [f64; 6],
    random: u32,
    noise_phase: f64,
    low: f64,
    high: f64,
    transpose: f64,
}
impl DrumVoice {
    pub(super) fn new(aliasing: Aliasing) -> Self {
        Self {
            age: 0,
            frames: 0,
            aliasing,
            oscillator: Oscillator::new(aliasing),
            phases: [0.0; 6],
            random: 1,
            noise_phase: 0.0,
            low: 0.0,
            high: 0.0,
            transpose: 1.0,
        }
    }
    pub(super) fn trigger(&mut self, plan: &DrumPlan, transpose: f64) {
        *self = Self {
            frames: plan.frames,
            random: if plan.settings.kit == DrumKit::Chiptune {
                (plan.settings.seed & 0x7fff).max(1)
            } else {
                plan.settings.seed.max(1)
            },
            transpose,
            ..Self::new(self.aliasing)
        };
    }
    pub(super) fn active(&self) -> bool {
        self.age < self.frames
    }
    pub(super) fn sample(&mut self, plan: &DrumPlan) -> f64 {
        if !self.active() {
            return 0.0;
        }
        let d = plan.settings;
        let elapsed = self.age;
        self.age += 1;
        let sweep = if plan.sweep == 0 {
            0.0
        } else {
            (1.0 - elapsed as f64 / plan.sweep as f64).max(0.0)
        };
        let hz = (d.pitch_hz as f64 * self.transpose * exp2(d.sweep_cents as f64 * sweep / 1200.0))
            .min(plan.rate as f64 * 0.45);
        let raw_noise = if d.kit == DrumKit::Chiptune {
            let value = if self.random & 1 == 0 { 1.0 } else { -1.0 };
            self.noise_phase += (d.pitch_hz as f64 * 8.0 / plan.rate as f64).min(1.0);
            if self.noise_phase >= 1.0 {
                self.noise_phase -= 1.0;
                let tap = if matches!(
                    d.kind,
                    DrumKind::ClosedHat | DrumKind::OpenHat | DrumKind::Crash
                ) {
                    6
                } else {
                    1
                };
                let bit = (self.random ^ (self.random >> tap)) & 1;
                self.random = (self.random >> 1) | (bit << 14);
            }
            value
        } else {
            self.random ^= self.random << 13;
            self.random ^= self.random >> 17;
            self.random ^= self.random << 5;
            (self.random >> 8) as f64 / (1u32 << 23) as f64 - 1.0
        };
        self.low = raw_noise * (1.0 - plan.low) + self.low * plan.low;
        self.high = self.low * (1.0 - plan.high) + self.high * plan.high;
        let noise = self.low - self.high;
        let tone = match d.kind {
            DrumKind::Clap => 0.0,
            DrumKind::ClosedHat | DrumKind::OpenHat | DrumKind::Crash => {
                let mut sum = 0.0;
                for (phase, ratio) in self
                    .phases
                    .iter_mut()
                    .zip([1.0, 1.342, 1.789, 2.129, 2.731, 3.113])
                {
                    sum += if *phase < 0.5 { 1.0 } else { -1.0 };
                    advance(
                        phase,
                        (hz * ratio).min(plan.rate as f64 * 0.45) / plan.rate as f64,
                    );
                }
                sum / 6.0
            }
            _ if d.kit == DrumKit::Chiptune => {
                self.oscillator.next(Waveform::Triangle, hz, plan.rate)
            }
            _ => {
                let fundamental = sin_turns(self.phases[0]);
                advance(&mut self.phases[0], hz / plan.rate as f64);
                if d.kind == DrumKind::Snare {
                    let overtone = sin_turns(self.phases[1]);
                    advance(
                        &mut self.phases[1],
                        (hz * 1.5).min(plan.rate as f64 * 0.45) / plan.rate as f64,
                    );
                    (fundamental + overtone * 0.35) / 1.35
                } else {
                    fundamental
                }
            }
        };
        let tone_env = envelope(elapsed, plan.tone, plan.attack);
        let noise_env = if d.kind == DrumKind::Clap && plan.noise > 0 {
            let mut bursts = 0.0;
            for &offset in &plan.offsets[..3] {
                if let Some(age) = elapsed.checked_sub(offset) {
                    bursts += envelope(age, plan.burst, 1);
                }
            }
            if let Some(age) = elapsed.checked_sub(plan.offsets[3]) {
                bursts += envelope(age, plan.noise, plan.attack);
            }
            bursts
        } else {
            envelope(elapsed, plan.noise, plan.attack)
        };
        let click_env = envelope(elapsed, plan.click, 1);
        let blend = d.noise_percent as f64 / 100.0;
        tone * tone_env * (1.0 - blend)
            + noise * noise_env * blend
            + noise * click_env * d.click_percent as f64 / 100.0
    }
}
fn envelope(age: u64, length: u64, attack: u64) -> f64 {
    if age >= length {
        return 0.0;
    }
    let t = 1.0 - age as f64 / length as f64;
    t * t * (age as f64 / attack as f64).min(1.0)
}
fn advance(phase: &mut f64, step: f64) {
    *phase += step;
    *phase -= phase.floor();
}
