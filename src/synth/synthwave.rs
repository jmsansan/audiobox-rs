//! Opt-in polyphonic, stereo instruments. Legacy instruments keep their sample bits.
use super::controls::{ControlParameter, Values};
use super::prepared::BeatClock;
use super::{Envelope, Oscillator, Waveform};
use crate::math::{cos_turns, exp2, sin_turns};
use crate::{Error, Result};

/// Low/high-pass have a 12 dB/octave slope; band-pass has a normalized peak.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FilterMode {
    LowPass,
    HighPass,
    BandPass,
}
/// A per-note trapezoidal state-variable filter. Output is not limited: resonance
/// can exceed full scale. Applied before the amplitude envelope, so tails are bounded.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VoiceFilter {
    pub mode: FilterMode,
    /// Base cutoff, 1..=96000 Hz. Modulated cutoff clamps to 1..=0.45*sample_rate Hz.
    pub cutoff_hz: u32,
    /// Q multiplied by 100, 50..=1000 (Q 0.5..=10).
    pub resonance: u16,
    /// Cutoff tracking relative to C4: 0..=200 percent; 100 follows note pitch.
    pub keytrack: u16,
    /// Independent cutoff ADSR (same units/bounds as the amplitude envelope).
    pub envelope: Envelope,
    /// Peak envelope offset, -9600..=9600 cents.
    pub envelope_cents: i16,
}
impl Default for VoiceFilter {
    fn default() -> Self {
        Self {
            mode: FilterMode::LowPass,
            cutoff_hz: 2000,
            resonance: 71,
            keytrack: 0,
            envelope: Envelope::default(),
            envelope_cents: 0,
        }
    }
}
/// A free-running frequency or a rational period in quarter-note beats.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LfoRate {
    /// 1..=20000 millihertz (0.001..=20 Hz).
    MilliHertz(u32),
    /// `numerator / denominator` quarter-note beats per cycle, both 1..=64.
    Beats { numerator: u16, denominator: u16 },
}
/// Sinusoidal modulation. All LFOs follow total rendered frames, including loops;
/// note triggers/legato do not reset their phase. Synth reset starts at phase zero.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Lfo {
    pub rate: LfoRate,
    /// Pitch/cutoff cents (0..=2400 / 0..=9600); amplitude/pan percent (0..=100).
    pub depth: u16,
}
impl Lfo {
    fn validate(self, max: u16) -> Result<()> {
        let rate = match self.rate {
            LfoRate::MilliHertz(n) => (1..=20000).contains(&n),
            LfoRate::Beats {
                numerator,
                denominator,
            } => (1..=64).contains(&numerator) && (1..=64).contains(&denominator),
        };
        if !rate || self.depth > max {
            return Err(Error::invalid("Invalid LFO rate or depth"));
        }
        Ok(())
    }
    fn value(self, clock: u64, rate: u32, tempo: u16) -> f64 {
        let (num, den) = match self.rate {
            LfoRate::MilliHertz(n) => (n as u128, rate as u128 * 1000),
            LfoRate::Beats {
                numerator,
                denominator,
            } => (
                tempo as u128 * denominator as u128,
                rate as u128 * 60 * numerator as u128,
            ),
        };
        // The phase's integer numerator, in 64 bits while the product fits: the same
        // value as the 128-bit form, without a 128-bit division per sample.
        let phase = match ((clock as u128).checked_mul(num), u64::try_from(den)) {
            (Some(product), Ok(den64)) if product <= u64::MAX as u128 => {
                (product as u64 % den64) as u128
            }
            _ => (clock as u128 * num) % den,
        };
        sin_turns(phase as f64 / den as f64) * self.depth as f64
    }
    fn value_clock(self, clock: u64, rate: u32, tempo: u16, musical: Option<BeatClock>) -> f64 {
        if let (
            Some(c),
            LfoRate::Beats {
                numerator,
                denominator,
            },
        ) = (musical, self.rate)
        {
            let den = c.denominator * numerator as u128;
            let phase = c.numerator * denominator as u128 % den;
            sin_turns(phase as f64 / den as f64) * self.depth as f64
        } else {
            self.value(clock, rate, tempo)
        }
    }
}
/// Opt-in synthwave controls, combined with an instrument's waveform and amplitude
/// ADSR. Supports pulse/square, saw and triangle. Mutually exclusive with chip/drum,
/// except that a wavetable chip source takes `voices` alone, with every other field at
/// its default, for per-note voices.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Synthwave {
    /// Oscillators per note, 1..=8, normalized by their count.
    pub unison: u8,
    /// Symmetric outer detune, 0..=100 cents; one oscillator has zero detune.
    pub detune: u16,
    /// Symmetric linear stereo spread, 0..=100 percent.
    pub spread: u8,
    /// Whole-note stereo pan, -100..=100 percent.
    pub pan: i8,
    /// Voice slots per channel, 1..=16. Idle, then oldest releasing, then oldest
    /// held voice is reused, ties by slot index. Stealing is immediate.
    pub voices: u8,
    /// Portamento duration in milliseconds, 0..=60000. Requires voices=1.
    /// Active-note retriggers interpolate in cents from the previous current pitch.
    pub glide_ms: u32,
    /// With voices=1, adjacent full-gate notes of the same instrument preserve
    /// amplitude/filter envelopes, phase and filter state. Gaps retrigger.
    pub legato: bool,
    pub filter: Option<VoiceFilter>,
    pub pitch_lfo: Option<Lfo>,
    pub cutoff_lfo: Option<Lfo>,
    pub amplitude_lfo: Option<Lfo>,
    pub pan_lfo: Option<Lfo>,
}
impl Default for Synthwave {
    fn default() -> Self {
        Self {
            unison: 1,
            detune: 0,
            spread: 0,
            pan: 0,
            voices: 8,
            glide_ms: 0,
            legato: false,
            filter: None,
            pitch_lfo: None,
            cutoff_lfo: None,
            amplitude_lfo: None,
            pan_lfo: None,
        }
    }
}
impl Synthwave {
    pub fn validate(&self) -> Result<()> {
        if !(1..=8).contains(&self.unison)
            || self.detune > 100
            || self.spread > 100
            || !(-100..=100).contains(&self.pan)
            || !(1..=16).contains(&self.voices)
            || self.glide_ms > 60000
            || ((self.glide_ms != 0 || self.legato) && self.voices != 1)
        {
            return Err(Error::invalid(
                "Invalid unison, spread, pan, voice limit or mono glide/legato settings",
            ));
        }
        if let Some(f) = self.filter {
            let e = f.envelope;
            if !(1..=96000).contains(&f.cutoff_hz)
                || !(50..=1000).contains(&f.resonance)
                || f.keytrack > 200
                || !(-9600..=9600).contains(&f.envelope_cents)
                || e.sustain > 100
                || [e.attack_ms, e.decay_ms, e.release_ms]
                    .iter()
                    .any(|&t| t > 60000)
            {
                return Err(Error::invalid("Invalid voice filter or filter envelope"));
            }
        } else if self.cutoff_lfo.is_some() {
            return Err(Error::invalid("Cutoff LFO requires a filter"));
        }
        for (lfo, max) in [
            (self.pitch_lfo, 2400),
            (self.cutoff_lfo, 9600),
            (self.amplitude_lfo, 100),
            (self.pan_lfo, 100),
        ] {
            if let Some(lfo) = lfo {
                lfo.validate(max)?;
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub(super) struct WidePlan {
    pub settings: Synthwave,
    rate: u32,
    tempo: u16,
    ratios: [f64; 8],
    pans: [f64; 8],
    glide: u64,
}
impl WidePlan {
    pub fn new(settings: Synthwave, rate: u32, tempo: u16) -> Self {
        let mut ratios = [1.0; 8];
        let mut pans = [0.0; 8];
        for i in 0..settings.unison as usize {
            let offset = if settings.unison == 1 {
                0.0
            } else {
                2.0 * i as f64 / (settings.unison - 1) as f64 - 1.0
            };
            ratios[i] = exp2(offset * settings.detune as f64 / 1200.0);
            pans[i] = offset * settings.spread as f64 / 100.0;
        }
        Self {
            settings,
            rate,
            tempo,
            ratios,
            pans,
            glide: (settings.glide_ms as u64 * rate as u64 + 500) / 1000,
        }
    }
}
// Trapezoidal SVF equations: Andrew Simper, Cytomic, 2013.
// https://cytomic.com/files/dsp/SvfLinearTrapOptimised2.pdf
#[derive(Clone, Copy, Debug, Default)]
struct FilterState {
    ic1: f64,
    ic2: f64,
}
impl FilterState {
    fn sample(&mut self, x: f64, mode: FilterMode, g: f64, k: f64) -> f64 {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        match mode {
            FilterMode::LowPass => v2,
            FilterMode::HighPass => x - k * v1 - v2,
            FilterMode::BandPass => k * v1,
        }
    }
}
#[derive(Clone, Debug)]
pub(super) struct WideVoice {
    oscillators: [Oscillator; 8],
    filters: [FilterState; 2],
    age: u64,
    released: Option<(u64, f64)>,
    filter_level: f64,
    pub pitch: f64,
    from: f64,
    target: f64,
    glide_age: u64,
    /// The last frequency's input cents (as bits) and result, reused while unchanged.
    hz: (u64, f64),
    /// The last filter coefficient's inputs (cents bits, base cutoff, rate) and `g`.
    coefficient: (u64, u64, u32, f64),
}
impl WideVoice {
    pub fn new() -> Self {
        Self {
            oscillators: std::array::from_fn(|_| Oscillator::new(super::Aliasing::BandLimited)),
            filters: [FilterState::default(); 2],
            age: 0,
            released: None,
            filter_level: 0.0,
            pitch: 0.0,
            from: 0.0,
            target: 0.0,
            glide_age: 0,
            hz: (f64::NAN.to_bits(), 0.0),
            coefficient: (f64::NAN.to_bits(), 0, 0, 0.0),
        }
    }
    pub fn trigger(&mut self, plan: &WidePlan, cents: f64, active: bool, legato: bool) {
        self.from = if active && plan.settings.voices == 1 && plan.glide > 0 {
            self.pitch
        } else {
            cents
        };
        self.target = cents;
        self.glide_age = 0;
        self.pitch = self.from;
        if !legato {
            self.age = 0;
            self.released = None;
            self.filter_level = 0.0;
            self.filters = [FilterState::default(); 2];
            for (i, osc) in self.oscillators.iter_mut().enumerate() {
                *osc = Oscillator::new(super::Aliasing::BandLimited);
                // A small fixed offset decorrelates voices without cancelling a
                // zero-detune square wave into silence.
                osc.phase_offset(i as f64 * 0.037);
            }
        }
    }
    pub fn release(&mut self) {
        if self.released.is_none() {
            self.released = Some((0, self.filter_level));
        }
    }
    pub fn sample(
        &mut self,
        plan: &WidePlan,
        wave: Waveform,
        cents_offset: f64,
        clock: u64,
        musical: Option<BeatClock>,
        controls: Option<Values>,
    ) -> [f64; 2] {
        let s = plan.settings;
        self.pitch = if plan.glide == 0 {
            self.target
        } else {
            self.from
                + (self.target - self.from)
                    * (self.glide_age.min(plan.glide) as f64 / plan.glide as f64)
        };
        self.glide_age = self.glide_age.saturating_add(1);
        let get = |p| controls.and_then(|c| c.get(p));
        let lfo = |l: Option<Lfo>, parameter: ControlParameter| {
            l.map_or(0.0, |l| {
                if let Some(depth) = get(parameter) {
                    Lfo { depth: 1, ..l }.value_clock(clock, plan.rate, plan.tempo, musical) * depth
                } else {
                    l.value_clock(clock, plan.rate, plan.tempo, musical)
                }
            })
        };
        let mut cents =
            self.pitch + cents_offset + lfo(s.pitch_lfo, ControlParameter::PitchLfoDepth);
        if let Some(offset) = get(ControlParameter::PitchCents) {
            cents += offset;
        }
        if self.hz.0 != cents.to_bits() {
            self.hz = (cents.to_bits(), crate::math::cents_to_hz(cents));
        }
        let hz = self.hz.1;
        let mut out = [0.0; 2];
        for i in 0..s.unison as usize {
            let sample =
                self.oscillators[i].next(wave, hz * plan.ratios[i], plan.rate) / s.unison as f64;
            let pan = plan.pans[i];
            out[0] += sample * (1.0 - pan.max(0.0));
            out[1] += sample * (1.0 + pan.min(0.0));
        }
        if let Some(f) = s.filter {
            self.filter_level = if let Some((age, from)) = &mut self.released {
                let release = frames(f.envelope.release_ms, plan.rate);
                let level = if release == 0 {
                    0.0
                } else {
                    *from * (1.0 - (*age as f64 / release as f64).min(1.0))
                };
                *age += 1;
                level
            } else {
                envelope(f.envelope, self.age, plan.rate)
            };
            let cents = f.envelope_cents as f64 * self.filter_level
                + (self.pitch + 900.0) * f.keytrack as f64 / 100.0
                + lfo(s.cutoff_lfo, ControlParameter::CutoffLfoDepth);
            let base = get(ControlParameter::CutoffHz).unwrap_or(f.cutoff_hz as f64);
            let key = (cents.to_bits(), base.to_bits(), plan.rate);
            if (self.coefficient.0, self.coefficient.1, self.coefficient.2) != key {
                let cutoff = (base * exp2(cents / 1200.0)).clamp(1.0, plan.rate as f64 * 0.45);
                let turns = cutoff / (2.0 * plan.rate as f64);
                self.coefficient = (key.0, key.1, key.2, sin_turns(turns) / cos_turns(turns));
            }
            let g = self.coefficient.3;
            let k = get(ControlParameter::ResonanceQ)
                .map_or_else(|| 100.0 / f.resonance as f64, |q| 1.0 / q);
            for (state, value) in self.filters.iter_mut().zip(&mut out) {
                *value = state.sample(*value, f.mode, g, k);
            }
        }
        let amp = s.amplitude_lfo.map_or(1.0, |l| {
            1.0 - (get(ControlParameter::AmplitudeLfoDepth).unwrap_or(l.depth as f64)
                + lfo(Some(l), ControlParameter::AmplitudeLfoDepth))
                / 200.0
        });
        let pan_lfo = lfo(s.pan_lfo, ControlParameter::PanLfoDepth);
        let pan = if let Some(p) = get(ControlParameter::Pan) {
            (p + pan_lfo / 100.0).clamp(-1.0, 1.0)
        } else {
            ((s.pan as f64 + pan_lfo) / 100.0).clamp(-1.0, 1.0)
        };
        out[0] *= amp * (1.0 - pan.max(0.0));
        out[1] *= amp * (1.0 + pan.min(0.0));
        self.age += 1;
        out
    }
}
fn frames(ms: u32, rate: u32) -> u64 {
    (ms as u64 * rate as u64 + 500) / 1000
}
fn envelope(e: Envelope, age: u64, rate: u32) -> f64 {
    let attack = frames(e.attack_ms, rate);
    let decay = frames(e.decay_ms, rate);
    let sustain = e.sustain as f64 / 100.0;
    if age < attack {
        age as f64 / attack as f64
    } else if age - attack < decay {
        1.0 - (1.0 - sustain) * (age - attack) as f64 / decay as f64
    } else {
        sustain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_have_measured_pass_stop_and_resonance_responses() {
        let rate = 48000.0;
        let cutoff = 1000.0;
        let g = sin_turns(cutoff / (2.0 * rate)) / cos_turns(cutoff / (2.0 * rate));
        let response = |mode, q, freq| {
            let mut state = FilterState::default();
            let mut input = 0.0;
            let mut output = 0.0;
            for n in 0..24000 {
                let x = sin_turns(n as f64 * freq / rate);
                let y = state.sample(x, mode, g, 1.0 / q);
                if n > 12000 {
                    input += x * x;
                    output += y * y;
                }
            }
            (output / input).sqrt()
        };
        for mode in [
            FilterMode::LowPass,
            FilterMode::HighPass,
            FilterMode::BandPass,
        ] {
            for q in [0.5, 1.0, 4.0, 10.0] {
                let centre = response(mode, q, cutoff);
                let expected = if mode == FilterMode::BandPass { 1.0 } else { q };
                assert!(
                    (centre / expected - 1.0).abs() < 0.003,
                    "{mode:?} Q={q}: {centre}"
                );
            }
        }
        assert!(response(FilterMode::LowPass, 0.71, 125.0) > 0.98);
        assert!(response(FilterMode::LowPass, 0.71, 8000.0) < 0.02);
        assert!(response(FilterMode::HighPass, 0.71, 125.0) < 0.02);
        assert!(response(FilterMode::HighPass, 0.71, 8000.0) > 0.98);
        for freq in [125.0, 8000.0] {
            assert!(response(FilterMode::BandPass, 4.0, freq) < 0.04);
        }
    }
    #[test]
    fn glide_reaches_target_at_the_exact_prepared_duration() {
        let plan = WidePlan::new(
            Synthwave {
                voices: 1,
                glide_ms: 100,
                legato: true,
                ..Default::default()
            },
            1000,
            120,
        );
        let mut voice = WideVoice::new();
        voice.trigger(&plan, 0.0, false, false);
        voice.sample(&plan, Waveform::Triangle, 0.0, 0, None, None);
        voice.trigger(&plan, 1200.0, true, true);
        for frame in 0..=100 {
            voice.sample(&plan, Waveform::Triangle, 0.0, frame, None, None);
            assert!((voice.pitch - frame as f64 * 12.0).abs() < 1e-9);
        }
        voice.sample(&plan, Waveform::Triangle, 0.0, 101, None, None);
        assert_eq!(voice.pitch, 1200.0);
        voice.trigger(&plan, -1200.0, true, true);
        voice.sample(&plan, Waveform::Triangle, 0.0, 102, None, None);
        assert_eq!(voice.pitch, 1200.0);
    }
    #[test]
    fn modulation_extremes_remain_finite_at_supported_rate_edges() {
        for rate in [1000, 8000, 48000, 768000] {
            for mode in [
                FilterMode::LowPass,
                FilterMode::HighPass,
                FilterMode::BandPass,
            ] {
                let plan = WidePlan::new(
                    Synthwave {
                        unison: 8,
                        detune: 100,
                        spread: 100,
                        filter: Some(VoiceFilter {
                            mode,
                            cutoff_hz: 96000,
                            resonance: 1000,
                            keytrack: 200,
                            envelope_cents: 9600,
                            envelope: Envelope {
                                attack_ms: 1,
                                decay_ms: 2,
                                sustain: 0,
                                release_ms: 1,
                            },
                        }),
                        cutoff_lfo: Some(Lfo {
                            rate: LfoRate::MilliHertz(20000),
                            depth: 9600,
                        }),
                        pitch_lfo: Some(Lfo {
                            rate: LfoRate::MilliHertz(20000),
                            depth: 2400,
                        }),
                        ..Default::default()
                    },
                    rate,
                    999,
                );
                let mut voice = WideVoice::new();
                voice.trigger(&plan, -6900.0, false, false);
                for n in 0..4000 {
                    let frame = voice.sample(&plan, Waveform::Sawtooth, 0.0, n, None, None);
                    assert!(
                        frame.iter().all(|x| x.is_finite() && x.abs() < 1000.0),
                        "rate={rate} {mode:?}: {frame:?}"
                    );
                }
            }
        }
    }
}
