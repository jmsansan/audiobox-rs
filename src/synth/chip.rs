//! Opt-in tracker primitives. This is not a cycle-accurate console emulator.
use super::{Duty, NoiseMode, Oscillator, Pitch, Waveform};
use crate::{Error, Limits, Result};

pub const MAX_MACRO_VALUES: usize = 64;
pub const MAX_SAMPLE_VALUES: usize = 65_536;
/// NTSC CPU cycles per noise-register shift, as documented by NESdev's APU Noise.
pub const NTSC_NOISE_PERIODS: [u16; 16] = [
    4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
];
/// Rounded NTSC CPU clock used by this model, in Hz.
pub const NTSC_CLOCK: u32 = 1_789_773;

/// A macro's step interval. Frame clocks are explicit simulation rates, independent
/// of audio callback size. Tick clocks are relative to the note's first sample.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MacroStep {
    /// One step per `frames` simulation frames at `hz` frames/second; both 1..=1000.
    Frames { frames: u16, hz: u16 },
    /// One step per 1..=960 score ticks.
    Ticks(u16),
}
impl Default for MacroStep {
    fn default() -> Self {
        Self::Frames { frames: 1, hz: 60 }
    }
}

/// An integer instrument sequence. Index zero applies to the first sample. At its
/// end it holds, or loops to `loop_start` after playing the initial prefix once.
/// Volume values are 0..=15; duty values 0..=3 select 12.5/25/50/75 percent;
/// arpeggio values are -48..=48 semitones; fine pitch is -1200..=1200 cents.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Macro {
    pub values: Vec<i16>,
    pub step: MacroStep,
    pub loop_start: Option<usize>,
}
impl Macro {
    pub fn new(values: Vec<i16>, step: MacroStep) -> Result<Self> {
        let m = Self {
            values,
            step,
            loop_start: None,
        };
        m.validate()?;
        Ok(m)
    }
    pub fn validate(&self) -> Result<()> {
        if self.values.is_empty()
            || self.values.len() > MAX_MACRO_VALUES
            || self.loop_start.is_some_and(|i| i >= self.values.len())
        {
            return Err(Error::invalid(
                "A macro needs 1..=64 values and an in-range loop start",
            ));
        }
        let valid = match self.step {
            MacroStep::Frames { frames, hz } => {
                (1..=1000).contains(&frames) && (1..=1000).contains(&hz)
            }
            MacroStep::Ticks(t) => (1..=960).contains(&t),
        };
        if !valid {
            return Err(Error::invalid("Invalid macro clock interval"));
        }
        Ok(())
    }
    /// Query a validated macro at a relative note age. Steps use a rational clock
    /// (no cumulative rounding). Tick boundaries can differ by one sample from
    /// globally floor-rounded score note positions.
    pub fn value_at(
        &self,
        frame: u64,
        sample_rate: u32,
        tempo: u16,
        ticks_per_quarter: u16,
    ) -> Result<i16> {
        self.validate()?;
        Limits::default().check(0, 1, sample_rate)?;
        if !(1..=999).contains(&tempo) || !(1..=960).contains(&ticks_per_quarter) {
            return Err(Error::invalid("Invalid macro score clock"));
        }
        Ok(self.value(frame, sample_rate, tempo, ticks_per_quarter))
    }
    fn value(&self, age: u64, rate: u32, tempo: u16, ticks: u16) -> i16 {
        let index = match self.step {
            MacroStep::Frames { frames, hz } => {
                age as u128 * hz as u128 / (rate as u128 * frames as u128)
            }
            MacroStep::Ticks(step) => {
                age as u128 * tempo as u128 * ticks as u128 / (rate as u128 * 60 * step as u128)
            }
        };
        let n = self.values.len();
        let index = if index < n as u128 {
            index as usize
        } else if let Some(start) = self.loop_start {
            start + ((index - start as u128) % (n - start) as u128) as usize
        } else {
            n - 1
        };
        self.values[index]
    }
}

/// A looping 4-bit waveform, read with sample-and-hold. 2..=256 entries, 0..=15.
/// Values map linearly from -1 to +1; tables and PCM are intentionally not anti-aliased.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Wavetable {
    pub values: Vec<u8>,
}
impl Wavetable {
    pub fn new(values: Vec<u8>) -> Result<Self> {
        let table = Self { values };
        table.validate()?;
        Ok(table)
    }
    pub fn validate(&self) -> Result<()> {
        if !(2..=256).contains(&self.values.len()) || self.values.iter().any(|&v| v > 15) {
            return Err(Error::invalid("A wavetable needs 2..=256 four-bit values"));
        }
        Ok(())
    }
}

/// Unsigned low-rate PCM, 2..=8 bits stored in one byte/value. Sample-and-hold with
/// optional whole-sample looping. Pitch is relative to `root`; it follows gate/ADSR.
/// A non-looping sample stops at its data end even when its note is still held.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ChipSample {
    pub values: Vec<u8>,
    pub bits: u8,
    /// 100..=32000 Hz.
    pub sample_rate: u32,
    pub root: Pitch,
    pub looping: bool,
}
impl ChipSample {
    pub fn new(values: Vec<u8>, bits: u8, sample_rate: u32) -> Result<Self> {
        let sample = Self {
            values,
            bits,
            sample_rate,
            root: Pitch::C4,
            looping: false,
        };
        sample.validate()?;
        Ok(sample)
    }
    pub fn validate(&self) -> Result<()> {
        if self.values.is_empty()
            || self.values.len() > MAX_SAMPLE_VALUES
            || !(2..=8).contains(&self.bits)
            || !(100..=32_000).contains(&self.sample_rate)
        {
            return Err(Error::invalid(
                "PCM needs 1..=65536 values, 2..=8 bits and 100..=32000 Hz",
            ));
        }
        let maximum = (1u16 << self.bits) - 1;
        if self.values.iter().any(|&v| v as u16 > maximum) {
            return Err(Error::invalid("PCM value exceeds its bit depth"));
        }
        Ok(())
    }
}

/// Eleven-bit NTSC period models: f=1789773/(divider*(timer+1)). Pulse divider=16,
/// timer=8..=2047; triangle divider=32, timer=0..=2047. Requests clamp to this range
/// rather than modelling hardware mute rules. See NESdev APU Pulse/Triangle.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PitchTable {
    NtscPulse,
    NtscTriangle,
}
impl PitchTable {
    pub fn timer(self, frequency: f64) -> Result<u16> {
        if !frequency.is_finite() || frequency <= 0.0 {
            return Err(Error::invalid(
                "Pitch-table frequency must be finite and positive",
            ));
        }
        let (divider, minimum) = self.bounds();
        Ok((NTSC_CLOCK as f64 / (divider * frequency) - 1.0)
            .round()
            .clamp(minimum, 2047.0) as u16)
    }
    pub fn frequency(self, timer: u16) -> Result<f64> {
        let (divider, minimum) = self.bounds();
        if !(minimum as u16..=2047).contains(&timer) {
            return Err(Error::invalid("Timer is outside the selected pitch table"));
        }
        Ok(NTSC_CLOCK as f64 / (divider * (timer as f64 + 1.0)))
    }
    fn bounds(self) -> (f64, f64) {
        match self {
            Self::NtscPulse => (16.0, 8.0),
            Self::NtscTriangle => (32.0, 0.0),
        }
    }
    fn quantize(self, frequency: f64) -> f64 {
        if !frequency.is_finite() || frequency <= 0.0 {
            return 0.0;
        }
        let (divider, minimum) = self.bounds();
        let timer = (NTSC_CLOCK as f64 / (divider * frequency) - 1.0)
            .round()
            .clamp(minimum, 2047.0);
        NTSC_CLOCK as f64 / (divider * (timer + 1.0))
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub enum ChipSource {
    #[default]
    Waveform,
    Wavetable(Wavetable),
    Sample(ChipSample),
    /// Fixed noise-register clock, bypassing the legacy note-frequency*93 clock.
    Noise {
        period: u8,
    },
}

/// Optional tracker controls. Volume quantization rounds the combined ADSR,
/// instrument volume, channel volume, velocity and volume-macro gain to 0..=15.
/// Pitch quantization runs after slide/vibrato and additive macro pitch changes.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct ChipInstrument {
    pub source: ChipSource,
    pub volume: Option<Macro>,
    pub duty: Option<Macro>,
    pub arpeggio: Option<Macro>,
    pub pitch: Option<Macro>,
    pub volume_4bit: bool,
    pub pitch_table: Option<PitchTable>,
}
impl ChipInstrument {
    pub(super) fn validate(&self, waveform: Waveform) -> Result<()> {
        for (m, lo, hi) in [
            (&self.volume, 0, 15),
            (&self.duty, 0, 3),
            (&self.arpeggio, -48, 48),
            (&self.pitch, -1200, 1200),
        ] {
            if let Some(m) = m {
                m.validate()?;
                if m.values.iter().any(|&v| !(lo..=hi).contains(&v)) {
                    return Err(Error::invalid("Macro value is outside its lane's range"));
                }
            }
        }
        match &self.source {
            ChipSource::Wavetable(w) => w.validate()?,
            ChipSource::Sample(s) => s.validate()?,
            ChipSource::Noise { period } => {
                if *period > 15 || !matches!(waveform, Waveform::Noise(_)) {
                    return Err(Error::invalid(
                        "Period noise needs wave=noise and period 0..=15",
                    ));
                }
                if self.arpeggio.is_some() || self.pitch.is_some() || self.pitch_table.is_some() {
                    return Err(Error::invalid(
                        "Fixed-period noise does not take pitch controls",
                    ));
                }
            }
            _ => (),
        }
        if matches!(
            self.source,
            ChipSource::Wavetable(_) | ChipSource::Sample(_)
        ) && waveform != Waveform::default()
        {
            return Err(Error::invalid(
                "Custom chip sources do not use a melodic waveform",
            ));
        }
        if self.duty.is_some()
            && (!matches!(self.source, ChipSource::Waveform)
                || !matches!(waveform, Waveform::Pulse(_)))
        {
            return Err(Error::invalid("Duty macros need a pulse waveform"));
        }
        if let Some(table) = self.pitch_table {
            let valid = matches!(self.source, ChipSource::Waveform)
                && match table {
                    PitchTable::NtscPulse => matches!(waveform, Waveform::Pulse(_)),
                    PitchTable::NtscTriangle => waveform == Waveform::Triangle,
                };
            if !valid {
                return Err(Error::invalid(
                    "Pitch table does not match its waveform source",
                ));
            }
        }
        Ok(())
    }
}

/// Clock frequency of one validated period entry, independent of note pitch.
pub fn noise_clock(period: u8) -> Result<f64> {
    NTSC_NOISE_PERIODS
        .get(period as usize)
        .map(|&p| NTSC_CLOCK as f64 / p as f64)
        .ok_or_else(|| Error::invalid("Noise period must be 0..=15"))
}

#[derive(Clone, Debug)]
pub(super) struct ChipPlan {
    settings: ChipInstrument,
    rate: u32,
    tempo: u16,
    ticks: u16,
    root_hz: f64,
}
impl ChipPlan {
    pub(super) fn new(settings: &ChipInstrument, rate: u32, tempo: u16, ticks: u16) -> Self {
        Self {
            settings: settings.clone(),
            root_hz: if let ChipSource::Sample(sample) = &settings.source {
                sample.root.frequency()
            } else {
                1.0
            },
            rate,
            tempo,
            ticks,
        }
    }
    fn lane(&self, lane: &Option<Macro>, age: u64, default: i16) -> i16 {
        lane.as_ref()
            .map_or(default, |m| m.value(age, self.rate, self.tempo, self.ticks))
    }
    pub(super) fn frequency(&self, frequency: f64, age: u64) -> f64 {
        let cents = self.lane(&self.settings.arpeggio, age, 0) as f64 * 100.0
            + self.lane(&self.settings.pitch, age, 0) as f64;
        let frequency = if cents == 0.0 {
            frequency
        } else {
            frequency * crate::math::exp2(cents / 1200.0)
        };
        self.settings
            .pitch_table
            .map_or(frequency, |table| table.quantize(frequency))
    }
    pub(super) fn gain(&self, level: f64, age: u64) -> f64 {
        let gain = level * self.lane(&self.settings.volume, age, 15) as f64 / 15.0;
        if self.settings.volume_4bit {
            (gain.clamp(0.0, 1.0) * 15.0).round() / 15.0
        } else {
            gain
        }
    }
}

const ONE: u64 = 1 << 32;
#[derive(Clone, Debug)]
pub(super) struct ChipVoice {
    oscillator: Oscillator,
    aliasing: super::Aliasing,
    phase: u64,
}
impl ChipVoice {
    pub(super) fn new(aliasing: super::Aliasing) -> Self {
        Self {
            oscillator: Oscillator::new(aliasing),
            aliasing,
            phase: 0,
        }
    }
    pub(super) fn reset(&mut self) {
        *self = Self::new(self.aliasing);
    }
    /// Returns a value and whether a finite PCM source just ended. All state advances
    /// even when a volume macro/ADSR/velocity happens to be zero.
    pub(super) fn sample(
        &mut self,
        plan: &ChipPlan,
        waveform: Waveform,
        frequency: f64,
        age: u64,
    ) -> (f64, bool) {
        let frequency = if frequency.is_finite() {
            frequency.max(0.0)
        } else {
            0.0
        };
        match &plan.settings.source {
            ChipSource::Waveform => {
                let waveform = if plan.settings.duty.is_some() {
                    Waveform::Pulse(match plan.lane(&plan.settings.duty, age, 2) {
                        0 => Duty::Eighth,
                        1 => Duty::Quarter,
                        2 => Duty::Half,
                        _ => Duty::ThreeQuarters,
                    })
                } else {
                    waveform
                };
                (self.oscillator.next(waveform, frequency, plan.rate), false)
            }
            ChipSource::Wavetable(table) => {
                let index = ((self.phase as u128 * table.values.len() as u128) >> 32) as usize;
                let value = table.values[index] as f64 * 2.0 / 15.0 - 1.0;
                let frequency = frequency.min(plan.rate as f64 / 2.0);
                self.phase =
                    (self.phase + (frequency / plan.rate as f64 * ONE as f64).round() as u64) % ONE;
                (value, false)
            }
            ChipSource::Sample(sample) => {
                let n = sample.values.len() as u64;
                let index = (self.phase >> 32).min(n - 1) as usize;
                let value =
                    sample.values[index] as f64 * 2.0 / ((1u16 << sample.bits) - 1) as f64 - 1.0;
                let step = sample.sample_rate as f64 / plan.rate as f64 * frequency / plan.root_hz;
                let end = n * ONE;
                let increment = (step * ONE as f64).round() as u64;
                self.phase = if sample.looping {
                    (self.phase + increment % end) % end
                } else {
                    self.phase.saturating_add(increment)
                };
                let finished = self.phase >= end && !sample.looping;
                (value, finished)
            }
            ChipSource::Noise { period } => {
                let mode = if let Waveform::Noise(mode) = waveform {
                    mode
                } else {
                    NoiseMode::Long
                };
                let clock = NTSC_CLOCK as f64 / NTSC_NOISE_PERIODS[*period as usize] as f64;
                (self.oscillator.clocked_noise(mode, clock, plan.rate), false)
            }
        }
    }
}
