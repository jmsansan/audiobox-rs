//! Modulation and colour DSP shared by the prepared chain machinery.
use super::{EffectTime, finite, within};
use crate::Result;
use crate::math::{cos_turns, db_to_gain, exp2, sin_turns};

/// LFO speed in cycles/second or one cycle per stated number of quarter-note beats.
/// Beat mode takes an explicit BPM. Send updated settings to follow tempo changes.
/// Speed updates preserve phase; this is period sync, not transport phase lock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModulationRate {
    /// 0..=20 Hz; zero freezes the phase.
    Hertz(f64),
    Beats {
        beats: f64,
        bpm: f64,
    },
}
impl ModulationRate {
    fn hz(self) -> Result<f64> {
        let hz = match self {
            Self::Hertz(hz) => hz,
            Self::Beats { beats, bpm } => 1.0 / EffectTime::Beats { beats, bpm }.seconds()?,
        };
        within(hz, 0.0, 20.0)?;
        Ok(hz)
    }
}

/// Two sinusoidally modulated delay taps per channel, with a quarter-cycle offset
/// between taps. The wet signal is their average. Linear interpolation; no added
/// dry-path latency. Delay +/- depth must fit 1 sample..=80 ms at preparation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chorus {
    pub rate: ModulationRate,
    pub delay_ms: f32,
    pub depth_ms: f32,
    /// -0.95..=0.95, fed back from the averaged wet signal.
    pub feedback: f32,
    /// Right-channel LFO offset in turns, 0..=1; mono uses zero offset.
    pub stereo_phase: f32,
    /// Linear dry/wet blend, 0..=1.
    pub mix: f32,
}
impl Default for Chorus {
    fn default() -> Self {
        Self {
            rate: ModulationRate::Hertz(0.8),
            delay_ms: 18.0,
            depth_ms: 6.0,
            feedback: 0.0,
            stereo_phase: 0.5,
            mix: 0.35,
        }
    }
}
impl Chorus {
    pub(super) fn values(self, rate: u32) -> Result<[f64; 6]> {
        modulation(
            self.rate,
            self.delay_ms,
            self.depth_ms,
            self.feedback,
            self.stereo_phase,
            self.mix,
            0.08,
            rate,
        )
    }
}

/// One sinusoidally modulated short delay tap per channel, creating a moving comb
/// response when mixed with dry audio. Delay +/- depth must fit 1 sample..=20 ms.
/// Default settings need at least 2000 Hz; reduce depth or increase delay below it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flanger {
    pub rate: ModulationRate,
    pub delay_ms: f32,
    pub depth_ms: f32,
    /// -0.95..=0.95.
    pub feedback: f32,
    /// Right-channel LFO offset in turns, 0..=1.
    pub stereo_phase: f32,
    /// Linear dry/wet blend, 0..=1.
    pub mix: f32,
}
impl Default for Flanger {
    fn default() -> Self {
        Self {
            rate: ModulationRate::Hertz(0.25),
            delay_ms: 2.0,
            depth_ms: 1.5,
            feedback: 0.5,
            stereo_phase: 0.0,
            mix: 0.5,
        }
    }
}
impl Flanger {
    pub(super) fn values(self, rate: u32) -> Result<[f64; 6]> {
        modulation(
            self.rate,
            self.delay_ms,
            self.depth_ms,
            self.feedback,
            self.stereo_phase,
            self.mix,
            0.02,
            rate,
        )
    }
}
#[allow(clippy::too_many_arguments)]
fn modulation(
    speed: ModulationRate,
    delay: f32,
    depth: f32,
    feedback: f32,
    spread: f32,
    mix: f32,
    maximum: f64,
    rate: u32,
) -> Result<[f64; 6]> {
    let delay = delay as f64 * rate as f64 / 1000.0;
    let depth = depth as f64 * rate as f64 / 1000.0;
    within(depth, 0.0, maximum * rate as f64)?;
    within(delay - depth, 1.0, maximum * rate as f64)?;
    within(delay + depth, 1.0, maximum * rate as f64)?;
    feedback_range(feedback)?;
    within(spread as f64, 0.0, 1.0)?;
    within(mix as f64, 0.0, 1.0)?;
    Ok([
        speed.hz()?,
        delay,
        depth,
        feedback as f64,
        spread as f64,
        mix as f64,
    ])
}
fn feedback_range(feedback: f32) -> Result<()> {
    within(feedback as f64, -(0.95f32 as f64), 0.95f32 as f64)
}

/// Four first-order allpass stages per channel. Full wet with zero feedback has
/// unity magnitude at a fixed frequency; blending dry/wet creates two notches.
/// The default 800 Hz centre needs at least 1778 Hz output; lower it at lower rates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Phaser {
    pub rate: ModulationRate,
    /// Centre of the logarithmic sweep, 1 Hz..=0.45 * sample rate.
    pub centre_hz: f32,
    /// Sweep +/- this many octaves, 0..=4. Instantaneous frequency is clamped to
    /// 1 Hz..=0.45 * sample rate, including during ramps.
    pub depth_octaves: f32,
    /// -0.95..=0.95, from the previous frame's allpass output.
    pub feedback: f32,
    /// Right-channel LFO offset in turns, 0..=1.
    pub stereo_phase: f32,
    /// Linear dry/wet blend, 0..=1.
    pub mix: f32,
}
impl Default for Phaser {
    fn default() -> Self {
        Self {
            rate: ModulationRate::Hertz(0.4),
            centre_hz: 800.0,
            depth_octaves: 2.0,
            feedback: 0.2,
            stereo_phase: 0.5,
            mix: 0.5,
        }
    }
}
impl Phaser {
    pub(super) fn values(self, rate: u32) -> Result<[f64; 6]> {
        within(self.centre_hz as f64, 1.0, rate as f64 * 0.45)?;
        within(self.depth_octaves as f64, 0.0, 4.0)?;
        feedback_range(self.feedback)?;
        within(self.stereo_phase as f64, 0.0, 1.0)?;
        within(self.mix as f64, 0.0, 1.0)?;
        Ok([
            self.rate.hz()?,
            self.centre_hz as f64,
            self.depth_octaves as f64,
            self.feedback as f64,
            self.stereo_phase as f64,
            self.mix as f64,
        ])
    }
}

/// Signed PCM quantization plus sample-and-hold rate reduction. At integer `bits`,
/// exactly 2^bits levels span -1..=1-2^(1-bits), rounded to nearest (ties away from
/// zero). Fractional bits blend adjacent integer quantizers for smooth automation.
/// Captures the first frame, then advances one shared stereo phase per frame.
/// Aliasing and quantization/hold steps are intentional; no dither or anti-aliasing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bitcrusher {
    /// 2..=24, including fractional values for automation.
    pub bits: f32,
    /// Capture rate / output rate, 0.001..=1. Default 0.25 (one in four frames).
    pub rate_ratio: f32,
    /// Linear dry/wet blend, 0..=1.
    pub mix: f32,
}
impl Default for Bitcrusher {
    fn default() -> Self {
        Self {
            bits: 8.0,
            rate_ratio: 0.25,
            mix: 1.0,
        }
    }
}
impl Bitcrusher {
    pub(super) fn values(self) -> Result<[f64; 6]> {
        within(self.bits as f64, 2.0, 24.0)?;
        within(self.rate_ratio as f64, 0.001f32 as f64, 1.0)?;
        within(self.mix as f64, 0.0, 1.0)?;
        Ok([
            self.bits as f64,
            self.rate_ratio as f64,
            self.mix as f64,
            0.0,
            0.0,
            0.0,
        ])
    }
}

/// Symmetric cubic soft saturation, without oversampling. For driven input v,
/// f(v)=v-v^3/3 when |v|<1, otherwise sign(v)*2/3. The slope joins continuously
/// at +/-1. This is a colour effect, not a physical tape model; harmonics can alias.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Saturation {
    /// Input drive, 0..=36 dB.
    pub drive_db: f32,
    /// Wet output gain, -24..=24 dB.
    pub output_db: f32,
    /// Linear dry/wet blend, 0..=1.
    pub mix: f32,
}
impl Default for Saturation {
    fn default() -> Self {
        Self {
            drive_db: 6.0,
            output_db: 0.0,
            mix: 1.0,
        }
    }
}
impl Saturation {
    pub(super) fn values(self) -> Result<[f64; 6]> {
        within(self.drive_db as f64, 0.0, 36.0)?;
        within(self.output_db as f64, -24.0, 24.0)?;
        within(self.mix as f64, 0.0, 1.0)?;
        Ok([
            self.drive_db as f64,
            self.output_db as f64,
            self.mix as f64,
            0.0,
            0.0,
            0.0,
        ])
    }
}

pub(super) enum Processor {
    ModDelay {
        ring: Vec<f64>,
        at: usize,
        phase: f64,
        chorus: bool,
    },
    Phaser {
        memory: Box<[[f64; 4]]>,
        feedback: [f64; 8],
        phase: f64,
    },
    Crusher {
        held: [f64; 8],
        phase: f64,
    },
    Saturation,
}
impl Processor {
    pub(super) fn new(kind: u8, rate: u32, channels: usize) -> Self {
        match kind {
            5 | 6 => Self::ModDelay {
                ring: vec![
                    0.0;
                    ((rate as f64 * if kind == 5 { 0.08 } else { 0.02 }).ceil() as usize + 2)
                        * channels
                ],
                at: 0,
                phase: 0.0,
                chorus: kind == 5,
            },
            7 => Self::Phaser {
                memory: vec![[0.0; 4]; channels].into_boxed_slice(),
                feedback: [0.0; 8],
                phase: 0.0,
            },
            8 => Self::Crusher {
                held: [0.0; 8],
                phase: 1.0,
            },
            _ => Self::Saturation,
        }
    }
    pub(super) fn process(&mut self, x: &mut [f64; 8], p: [f64; 6], rate: u32, channels: usize) {
        match self {
            Self::ModDelay {
                ring,
                at,
                phase,
                chorus,
            } => {
                // Endpoint-valid settings can cross a bound during nonlinear ramps.
                let max = rate as f64 * if *chorus { 0.08 } else { 0.02 };
                for c in 0..channels {
                    let phase = *phase + c as f64 * p[4];
                    let delay = (p[1] + p[2] * sin_turns(phase)).clamp(1.0, max);
                    let mut wet = read(ring, *at, delay, c, channels);
                    if *chorus {
                        let delay = (p[1] + p[2] * sin_turns(phase + 0.25)).clamp(1.0, max);
                        wet = (wet + read(ring, *at, delay, c, channels)) * 0.5;
                    }
                    ring[*at * channels + c] = finite(x[c] + wet * p[3]);
                    x[c] = x[c] * (1.0 - p[5]) + wet * p[5];
                }
                *at = (*at + 1) % (ring.len() / channels);
                advance(phase, p[0] / rate as f64);
            }
            Self::Phaser {
                memory,
                feedback,
                phase,
            } => {
                for c in 0..channels {
                    let hz = (p[1] * exp2(p[2] * sin_turns(*phase + c as f64 * p[4])))
                        .clamp(1.0, rate as f64 * 0.45);
                    let turns = hz / (2.0 * rate as f64);
                    let g = sin_turns(turns) / cos_turns(turns);
                    let a = (g - 1.0) / (g + 1.0);
                    let mut wet = finite(x[c] + feedback[c] * p[3]);
                    for state in &mut memory[c] {
                        let output = a * wet + *state;
                        *state = finite(wet - a * output);
                        wet = finite(output);
                    }
                    feedback[c] = wet;
                    x[c] = x[c] * (1.0 - p[5]) + wet * p[5];
                }
                advance(phase, p[0] / rate as f64);
            }
            Self::Crusher { held, phase } => {
                if *phase >= 1.0 {
                    *held = *x;
                    *phase -= phase.floor();
                }
                *phase += p[1];
                let bits = p[0].floor() as u32;
                let fraction = p[0] - bits as f64;
                for c in 0..channels {
                    let low = quantize(held[c], bits);
                    let high = quantize(held[c], (bits + 1).min(24));
                    let wet = low + (high - low) * fraction;
                    x[c] = x[c] * (1.0 - p[2]) + wet * p[2];
                }
            }
            Self::Saturation => {
                let drive = db_to_gain(p[0]);
                let gain = db_to_gain(p[1]);
                for sample in &mut x[..channels] {
                    let v = *sample * drive;
                    let wet = if v.abs() >= 1.0 {
                        v.signum() * (2.0 / 3.0)
                    } else {
                        v - v * v * v / 3.0
                    } * gain;
                    *sample = *sample * (1.0 - p[2]) + wet * p[2];
                }
            }
        }
    }
}
fn read(ring: &[f64], at: usize, delay: f64, channel: usize, channels: usize) -> f64 {
    let frames = ring.len() / channels;
    let mut pos = at as f64 - delay;
    if pos < 0.0 {
        pos += frames as f64;
    }
    let a = pos.floor() as usize;
    let fraction = pos - a as f64;
    ring[a * channels + channel] * (1.0 - fraction)
        + ring[((a + 1) % frames) * channels + channel] * fraction
}
fn advance(phase: &mut f64, step: f64) {
    *phase += step;
    *phase -= phase.floor();
}
fn quantize(value: f64, bits: u32) -> f64 {
    let scale = (1u64 << (bits - 1)) as f64;
    (value.clamp(-1.0, 1.0) * scale)
        .round()
        .clamp(-scale, scale - 1.0)
        / scale
}
