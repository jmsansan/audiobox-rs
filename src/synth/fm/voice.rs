//! Rendering of [`Fm`] voices. Formulas and units are those of `docs/fm-voice.md`.
use super::{FM_CARRIER_PEAK, FM_OPERATORS, Fm, FmCurve, FmEnvelope, FmLfoWave, FmOperator};
use crate::math::{cents_to_hz, exp2};
use crate::synth::controls::{ControlParameter, Values};
use crate::synth::prepared::BeatClock;

/// Level steps at full scale; 256 steps double the amplitude.
const FULL_SCALE: f64 = 3840.0;
/// An envelope at or below this level is silent.
const FLOOR: f64 = 16.0;
/// A rising stage starts from at least this level.
const ATTACK_JUMP: f64 = 1716.0;
/// Envelope clock of the instrument the rates were measured on.
const NATIVE_RATE: f64 = 49096.0;
/// Octaves per detune step.
const DETUNE: f64 = 12606.0 / 16_777_216.0;
/// Taps of the half-band filter that brings a twice-oversampled voice down.
const TAPS: usize = 4 * super::FM_OVERSAMPLE_DELAY + 1;
/// Release tails are cut after this many seconds.
const MAX_TAIL_SECONDS: u64 = 60;

const LEVELS: [u8; 20] = [
    0, 5, 9, 13, 17, 20, 23, 25, 27, 29, 31, 33, 35, 37, 39, 41, 42, 43, 45, 46,
];
#[rustfmt::skip]
const PITCH_LEVELS: [i8; 100] = [
    -128, -116, -104, -95, -85, -76, -68, -61, -56, -52, -49, -46, -43, -41, -39, -37, -35, -33,
    -32, -31, -30, -29, -28, -27, -26, -25, -24, -23, -22, -21, -20, -19, -18, -17, -16, -15, -14,
    -13, -12, -11, -10, -9, -8, -7, -6, -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11,
    12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35,
    38, 40, 43, 46, 49, 53, 58, 65, 73, 82, 92, 103, 115, 127,
];
#[rustfmt::skip]
const PITCH_RATES: [u8; 100] = [
    1, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 16,
    16, 17, 18, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 30, 31, 33, 34, 36, 37, 38, 39, 41, 42,
    44, 46, 47, 49, 51, 53, 54, 56, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76, 79, 82, 85, 88, 91, 94,
    98, 102, 106, 110, 115, 120, 125, 130, 135, 141, 147, 153, 159, 165, 171, 178, 185, 193, 202,
    211, 232, 243, 254, 255,
];

#[rustfmt::skip]
const VELOCITY: [u8; 64] = [
    0, 70, 86, 97, 106, 114, 121, 126, 132, 138, 142, 148, 152, 156, 160, 163, 166, 170, 173, 174,
    178, 181, 184, 186, 189, 190, 194, 196, 198, 200, 202, 205, 206, 209, 211, 214, 216, 218, 220,
    222, 224, 225, 227, 229, 230, 232, 233, 235, 237, 238, 240, 241, 242, 243, 244, 246, 246, 248,
    249, 250, 251, 252, 253, 254,
];
#[rustfmt::skip]
const EXPONENTIAL: [u8; 33] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 14, 16, 19, 23, 27, 33, 39, 47, 56, 66, 80, 94, 110, 126,
    142, 158, 174, 190, 206, 222, 238, 250,
];
const PITCH_SENSITIVITY: [f64; 8] = [0.0, 10.0, 20.0, 33.0, 55.0, 92.0, 153.0, 255.0];
/// Numerator of an LFO rate unit in Hz, over 2^32.
const LFO_UNIT: u128 = 25_190_424;

/// Fraction and whole cycles of `frames * num / den`, exactly.
fn cycles(frames: u128, num: u128, den: u128) -> (f64, u64) {
    let product = frames * num;
    ((product % den) as f64 / den as f64, (product / den) as u64)
}
/// A value in 1/256..=1 for one sample-and-hold cycle.
fn held_value(seed: u32, cycle: u64) -> f64 {
    let mut x = seed ^ (cycle as u32).wrapping_mul(0x9e37_79b9) ^ ((cycle >> 32) as u32);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 13;
    x = x.wrapping_mul(0xc2b2_ae35);
    x ^= x >> 16;
    ((x & 0xff) + 1) as f64 / 256.0
}

/// The voice LFO, prepared for one sample rate and tempo.
#[derive(Clone, Copy, Debug)]
struct LfoPlan {
    wave: FmLfoWave,
    /// Cycles per frame as a fraction.
    rate: (u128, u128),
    beats: Option<(u16, u16)>,
    key_sync: bool,
    seed: u32,
    /// Frames of silence and of the ramp to full depth.
    hold: f64,
    ramp: f64,
    /// Peak pitch deviation in cents.
    pitch: f64,
    /// Share of each operator's gain removed at the top of the wave.
    amplitude: [f64; FM_OPERATORS],
}
impl LfoPlan {
    fn new(settings: &Fm, rate: u32, tempo: u16) -> Option<Self> {
        let l = settings.lfo;
        let amplitude = std::array::from_fn(|i| {
            settings.operators[i].map_or(0.0, |o| {
                l.amplitude_depth as f64 * 0.01 * o.amplitude_sensitivity as f64 / 3.0
            })
        });
        let depth = ((l.pitch_depth as u32 * 165) >> 6) as f64;
        let pitch = 1200.0 * depth * PITCH_SENSITIVITY[l.pitch_sensitivity as usize] / 65536.0;
        if pitch == 0.0 && amplitude.iter().all(|a| *a == 0.0) {
            return None;
        }
        let speed = match (l.speed as u128 * 165) >> 6 {
            _ if l.speed == 0 => 11,
            sr if sr < 160 => sr * 11,
            sr => sr * (11 + ((sr - 160) >> 4)),
        };
        let (hold, ramp) = match 99 - l.delay as u32 {
            99 => (0.0, 0.0),
            a => {
                let a = (16 + (a & 15)) << (1 + (a >> 4));
                let unit = (1u64 << 31) as f64 / LFO_UNIT as f64 * rate as f64;
                (unit / a as f64, unit / (a & 0xff80).max(0x80) as f64)
            }
        };
        Some(Self {
            wave: l.wave,
            rate: match l.beats {
                Some((n, d)) => (tempo as u128 * d as u128, rate as u128 * 60 * n as u128),
                None => (speed * LFO_UNIT, (rate as u128) << 32),
            },
            beats: l.beats,
            key_sync: l.key_sync,
            seed: l.seed,
            hold,
            ramp,
            pitch,
            amplitude,
        })
    }
    /// The wave in 0..=1 for a phase in turns.
    fn wave(&self, phase: f64, cycle: u64) -> f64 {
        match self.wave {
            FmLfoWave::Triangle if phase < 0.5 => 2.0 * phase,
            FmLfoWave::Triangle => 2.0 - 2.0 * phase,
            FmLfoWave::SawDown if phase < 0.5 => 0.5 - phase,
            FmLfoWave::SawDown => 1.5 - phase,
            FmLfoWave::SawUp if phase < 0.5 => phase + 0.5,
            FmLfoWave::SawUp => phase - 0.5,
            FmLfoWave::Square if phase < 0.5 => 1.0,
            FmLfoWave::Square => 0.0,
            FmLfoWave::Sine => 0.5 + 0.5 * sine(phase),
            FmLfoWave::SampleHold => held_value(self.seed, cycle),
        }
    }
}

/// Level steps the strongest velocity adds at the highest sensitivity.
const VELOCITY_HEADROOM: f64 = 224.0;

/// Steps added to an operator's base level by the note's velocity.
fn velocity_offset(sensitivity: u8, velocity: u8) -> i32 {
    let value = VELOCITY[(velocity.min(127) >> 1) as usize] as i32 - 239;
    ((sensitivity as i32 * value + 7) >> 3) << 4
}
/// Offset to the 0..=127 output level by distance from the break point.
fn level_scaling(o: &FmOperator, note: i32) -> i32 {
    let s = o.scaling;
    let offset = note - s.break_point as i32 - 17;
    let group = offset.abs() / 3;
    let (depth, curve) = if offset >= 0 {
        (s.right_depth, s.right_curve)
    } else {
        (s.left_depth, s.left_curve)
    };
    let amount = match curve {
        FmCurve::NegativeLinear | FmCurve::PositiveLinear => (group * depth as i32 * 329) >> 12,
        _ => (EXPONENTIAL[group.min(32) as usize] as i32 * depth as i32 * 329) >> 15,
    };
    match curve {
        FmCurve::NegativeLinear | FmCurve::NegativeExponential => -amount,
        _ => amount,
    }
}
/// Added to an envelope's 0..=63 rate: higher notes run faster.
fn rate_scaling(sensitivity: u8, note: i32) -> i32 {
    (sensitivity as i32 * (note / 3 - 7).clamp(0, 31)) >> 3
}

/// A 0..=99 level on the 0..=127 scale of 32 steps each.
fn scale(level: u8) -> i32 {
    if level >= 20 {
        28 + level as i32
    } else {
        LEVELS[level as usize] as i32
    }
}
/// Steps per second of a falling stage.
fn slope(rate: u8, scaling: i32) -> f64 {
    let q = (((rate as i32 * 41) >> 6) + scaling).min(63);
    NATIVE_RATE * (4 + (q & 3)) as f64 * (1u64 << (q >> 2)) as f64 / 16384.0
}
/// `sin(2*pi*x)` to about 1e-11, as a fixed polynomial without divisions: the
/// operator loop runs it for every operator of every voice.
fn sine(x: f64) -> f64 {
    const C: [f64; 9] = [
        1.0,
        -1.0 / 6.0,
        1.0 / 120.0,
        -1.0 / 5040.0,
        1.0 / 362_880.0,
        -1.0 / 39_916_800.0,
        1.0 / 6_227_020_800.0,
        -1.0 / 1_307_674_368_000.0,
        1.0 / 355_687_428_096_000.0,
    ];
    let mut q = x - x.floor();
    if q > 0.75 {
        q -= 1.0;
    } else if q > 0.25 {
        q = 0.5 - q;
    }
    let r = q * std::f64::consts::TAU;
    let r2 = r * r;
    let mut sum = C[8];
    for c in C[..8].iter().rev() {
        sum = sum * r2 + c;
    }
    sum * r
}
fn gain(steps: f64) -> f64 {
    if steps <= FLOOR {
        0.0
    } else {
        exp2((steps - FULL_SCALE) / 256.0)
    }
}

#[derive(Clone, Copy, Debug)]
struct OperatorPlan {
    settings: FmOperator,
    /// Multiple of the note frequency, detune included; unused when `fixed` is set.
    ratio: f64,
    fixed: Option<f64>,
    modulators: u8,
    carrier: bool,
}
/// One note's envelope of one operator, fixed at note-on from pitch and velocity.
#[derive(Clone, Copy, Debug, Default)]
struct Shape {
    /// Targets in steps and increments in steps per sample.
    targets: [f64; 4],
    increments: [f64; 4],
    /// Linear gain at each target, and the per-sample gain ratio of a falling stage.
    gains: [f64; 4],
    falls: [f64; 4],
}
impl Shape {
    fn new(o: &FmOperator, note: i32, velocity: u8, per_sample: f64) -> Self {
        let out = (scale(o.level) + level_scaling(o, note)).min(127);
        let base = (32 * out + velocity_offset(o.velocity, velocity)).max(0);
        let targets = o
            .envelope
            .levels
            .map(|level| (64 * (scale(level) >> 1) + base - 4256).max(16) as f64);
        let faster = rate_scaling(o.rate_scaling, note);
        let increments = o.envelope.rates.map(|r| slope(r, faster) * per_sample);
        Self {
            targets,
            increments,
            gains: targets.map(gain),
            falls: increments.map(|i| exp2(-i / 256.0)),
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::synth) struct FmPlan {
    pub settings: Fm,
    rate: f64,
    operators: [Option<OperatorPlan>; FM_OPERATORS],
    /// Scale of the mean of the feedback source's last two outputs; 0 is off.
    feedback: f64,
    feedback_from: usize,
    feedback_to: usize,
    /// Pitch envelope targets in cents and increments in cents per sample.
    pitch: Option<([f64; 4], [f64; 4])>,
    ratios: [f64; 8],
    pans: [f64; 8],
    glide: u64,
    lfo: Option<LfoPlan>,
    /// Operator passes per output frame, and the filter that follows two of them.
    passes: usize,
    taps: [f64; TAPS],
    /// Frames after key-off at which a voice is cut.
    pub tail: u64,
}
/// A Kaiser-windowed sinc low-pass at a quarter of the doubled rate, with unit gain
/// at DC: flat to 0.4 of the output rate and about 80 dB down from 0.6, so what
/// still folds lands above 0.4.
fn half_band() -> [f64; TAPS] {
    // Zeroth-order modified Bessel function, by its power series.
    let bessel = |x: f64| {
        let (mut sum, mut term) = (1.0, 1.0);
        for k in 1..40 {
            term *= x / 2.0 / k as f64;
            sum += term * term;
        }
        sum
    };
    const BETA: f64 = 7.857;
    let centre = (TAPS / 2) as f64;
    let mut taps: [f64; TAPS] = std::array::from_fn(|i| {
        let k = i as f64 - centre;
        let ideal = if k == 0.0 {
            0.5
        } else {
            sine(k / 4.0) / (std::f64::consts::PI * k)
        };
        let r = k / centre;
        ideal * bessel(BETA * (1.0 - r * r).max(0.0).sqrt()) / bessel(BETA)
    });
    let sum: f64 = taps.iter().sum();
    for tap in &mut taps {
        *tap /= sum;
    }
    taps
}
impl FmPlan {
    pub fn new(settings: Fm, rate: u32, tempo: u16) -> Self {
        let routing = settings.routing().expect("validated algorithm");
        let per_sample = 1.0 / rate as f64;
        let mut tail: f64 = 0.0;
        let operators = std::array::from_fn(|index| {
            let o = settings.operators[index]?;
            let carrier = routing.is_carrier(index as u8 + 1);
            if carrier {
                // Rate scaling only shortens a release, so the unscaled rate bounds it.
                let slowest = slope(o.envelope.rates[3], 0) * per_sample;
                tail = tail.max((FULL_SCALE + VELOCITY_HEADROOM - FLOOR) / slowest);
            }
            Some(OperatorPlan {
                settings: o,
                ratio: o.frequency.ratio().unwrap_or(1.0) * exp2(o.detune as f64 * DETUNE),
                fixed: o.frequency.fixed_hz(),
                modulators: routing.modulators[index],
                carrier,
            })
        });
        let envelope = settings.pitch_envelope;
        let pitch = (envelope.levels != FmEnvelope::PITCH.levels).then(|| {
            (
                envelope
                    .levels
                    .map(|l| PITCH_LEVELS[l as usize] as f64 * 1200.0 / 32.0),
                envelope
                    .rates
                    .map(|r| PITCH_RATES[r as usize] as f64 * 1200.0 / 21.3 * per_sample),
            )
        });
        let mut ratios = [1.0; 8];
        let mut pans = [0.0; 8];
        for i in 0..settings.unison as usize {
            let offset = if settings.unison == 1 {
                0.0
            } else {
                2.0 * i as f64 / (settings.unison - 1) as f64 - 1.0
            };
            ratios[i] = exp2(offset * settings.unison_detune as f64 / 1200.0);
            pans[i] = offset * settings.spread as f64 / 100.0;
        }
        Self {
            settings,
            rate: rate as f64,
            operators,
            feedback: match settings.feedback {
                0 => 0.0,
                f => exp2(f as f64 - 8.0),
            },
            feedback_from: routing.feedback_from as usize - 1,
            feedback_to: routing.feedback_to as usize - 1,
            pitch,
            ratios,
            pans,
            glide: (settings.glide_ms as u64 * rate as u64 + 500) / 1000,
            lfo: LfoPlan::new(&settings, rate, tempo),
            passes: settings.oversample as usize,
            taps: half_band(),
            tail: (tail.ceil() as u64 + 1).min(MAX_TAIL_SECONDS * rate as u64),
        }
    }
}

/// A four-stage rate/level envelope. Stage 3 runs only after key-off; 4 is finished.
#[derive(Clone, Copy, Debug, Default)]
struct Stages {
    level: f64,
    target: f64,
    increment: f64,
    stage: u8,
    rising: bool,
    /// Linear gain of `level`, kept in step with it by multiplication.
    gain: f64,
    /// The 256-step band of a rising level and its per-sample gain ratio.
    band: (f64, f64),
}
impl Stages {
    fn advance(&mut self, stage: u8, targets: &[f64; 4], increments: &[f64; 4]) {
        self.stage = stage;
        if let Some(&target) = targets.get(stage as usize) {
            self.target = target;
            self.increment = increments[stage as usize];
            self.rising = target > self.level;
            self.band.0 = f64::NAN;
        }
    }
    /// One sample of a pitch envelope: linear both ways.
    fn glide(&mut self, held: bool, targets: &[f64; 4], increments: &[f64; 4]) {
        if self.stage >= 4 || (self.stage == 3 && held) {
            return;
        }
        let reached = if self.rising {
            self.level += self.increment;
            self.level >= self.target
        } else {
            self.level -= self.increment;
            self.level <= self.target
        };
        if reached {
            self.level = self.target;
            self.advance(self.stage + 1, targets, increments);
        }
    }
    /// One sample of an operator level and its gain.
    fn tick(&mut self, held: bool, o: &Shape) {
        if self.stage >= 4 || (self.stage == 3 && held) {
            return;
        }
        let reached = if self.rising {
            if self.level < ATTACK_JUMP {
                self.level = ATTACK_JUMP;
                self.gain = gain(ATTACK_JUMP);
            }
            let band = (self.level / 256.0).floor();
            if self.band.0 != band {
                self.band = (band, exp2((17.0 - band) * self.increment / 256.0));
            }
            self.level += (17.0 - band) * self.increment;
            self.gain *= self.band.1;
            self.level >= self.target
        } else {
            self.level -= self.increment;
            self.gain *= o.falls[self.stage as usize];
            self.level <= self.target
        };
        if reached {
            self.level = self.target;
            self.gain = o.gains[self.stage as usize];
            self.advance(self.stage + 1, &o.targets, &o.increments);
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::synth) struct FmVoice {
    envelopes: [Stages; FM_OPERATORS],
    shapes: [Shape; FM_OPERATORS],
    pitch: Stages,
    phases: [[f64; FM_OPERATORS]; 8],
    feedback: [[f64; 2]; 8],
    held: bool,
    sounding: bool,
    released: u64,
    from: f64,
    target: f64,
    current: f64,
    glide_age: u64,
    /// Frames since the last attack, and the musical clock at it.
    age: u64,
    beat_start: Option<u128>,
    /// The last pitch in cents (as bits) and its frequency.
    hz: (u64, f64),
    /// The last oversampled stereo outputs, newest at `head`. One more than the
    /// filter needs, so its centre falls on the first pass of a frame.
    history: [[f64; 2]; TAPS + 1],
    head: usize,
}
impl FmVoice {
    pub fn new() -> Self {
        Self {
            envelopes: [Stages::default(); FM_OPERATORS],
            shapes: [Shape::default(); FM_OPERATORS],
            pitch: Stages::default(),
            phases: [[0.0; FM_OPERATORS]; 8],
            feedback: [[0.0; 2]; 8],
            held: false,
            sounding: false,
            released: 0,
            from: 0.0,
            target: 0.0,
            current: 0.0,
            glide_age: 0,
            age: 0,
            beat_start: None,
            hz: (f64::NAN.to_bits(), 0.0),
            history: [[0.0; 2]; TAPS + 1],
            head: 0,
        }
    }
    /// `gliding`: the slot still sounds an FM note to glide from. `legato` keeps the
    /// envelopes and phases and only moves the pitch.
    pub fn trigger(
        &mut self,
        plan: &FmPlan,
        cents: f64,
        velocity: u8,
        gliding: bool,
        legato: bool,
    ) {
        let cents = cents + plan.settings.transpose as f64 * 100.0;
        self.from = if gliding && plan.glide > 0 {
            self.current
        } else {
            cents
        };
        self.target = cents;
        self.current = self.from;
        self.glide_age = 0;
        self.held = true;
        if legato && self.sounding {
            return;
        }
        if !self.sounding {
            for envelope in &mut self.envelopes {
                (envelope.level, envelope.gain) = (0.0, 0.0);
            }
            self.history = [[0.0; 2]; TAPS + 1];
        }
        self.sounding = true;
        self.released = 0;
        self.age = 0;
        self.beat_start = None;
        let note = (cents / 100.0).round() as i32 + 69;
        for (index, operator) in plan.operators.iter().enumerate() {
            if let Some(o) = operator {
                let shape = Shape::new(&o.settings, note, velocity, 1.0 / plan.rate);
                self.envelopes[index].advance(0, &shape.targets, &shape.increments);
                self.shapes[index] = shape;
            }
        }
        if let Some((targets, increments)) = &plan.pitch {
            self.pitch.level = targets[3];
            self.pitch.advance(0, targets, increments);
        }
        if plan.settings.key_sync {
            self.phases = [[0.0; FM_OPERATORS]; 8];
            self.feedback = [[0.0; 2]; 8];
        }
    }
    pub fn release(&mut self, plan: &FmPlan) {
        if !std::mem::replace(&mut self.held, false) {
            return;
        }
        for (envelope, shape) in self.envelopes.iter_mut().zip(&self.shapes) {
            envelope.advance(3, &shape.targets, &shape.increments);
        }
        if let Some((targets, increments)) = &plan.pitch {
            self.pitch.advance(3, targets, increments);
        }
    }
    pub fn active(&self) -> bool {
        self.sounding
    }
    /// The LFO's wave in 0..=1 scaled by its delay, for this frame.
    fn lfo(&mut self, lfo: &LfoPlan, clock: u64, musical: Option<BeatClock>) -> (f64, f64) {
        let fade = if self.age as f64 >= lfo.hold + lfo.ramp {
            1.0
        } else if self.age as f64 <= lfo.hold {
            0.0
        } else {
            (self.age as f64 - lfo.hold) / lfo.ramp
        };
        let (phase, cycle) = match (musical, lfo.beats) {
            (Some(c), Some((n, d))) => {
                let start = match lfo.key_sync {
                    true => *self.beat_start.get_or_insert(c.numerator),
                    false => 0,
                };
                cycles(c.numerator - start, d as u128, c.denominator * n as u128)
            }
            _ => {
                let frames = if lfo.key_sync { self.age } else { clock };
                cycles(frames as u128, lfo.rate.0, lfo.rate.1)
            }
        };
        // A synced LFO starts half a cycle in, as on the instrument.
        let (phase, cycle) = match lfo.key_sync {
            true if phase < 0.5 => (phase + 0.5, cycle),
            true => (phase - 0.5, cycle + 1),
            false => (phase, cycle),
        };
        (lfo.wave(phase, cycle), fade)
    }
    pub fn sample(
        &mut self,
        plan: &FmPlan,
        clock: u64,
        musical: Option<BeatClock>,
        controls: Option<Values>,
    ) -> [f64; 2] {
        if !self.sounding {
            return [0.0; 2];
        }
        let s = &plan.settings;
        let get = |p| controls.and_then(|c| c.get(p));
        self.current = if plan.glide == 0 {
            self.target
        } else {
            self.from
                + (self.target - self.from)
                    * (self.glide_age.min(plan.glide) as f64 / plan.glide as f64)
        };
        self.glide_age = self.glide_age.saturating_add(1);
        let mut cents = self.current + get(ControlParameter::PitchCents).unwrap_or(0.0);
        if let Some((targets, increments)) = &plan.pitch {
            self.pitch.glide(self.held, targets, increments);
            cents += self.pitch.level;
        }
        let mut tremolo = [1.0; FM_OPERATORS];
        if let Some(lfo) = &plan.lfo {
            let (wave, fade) = self.lfo(lfo, clock, musical);
            cents += lfo.pitch * fade * (2.0 * wave - 1.0);
            for (gain, depth) in tremolo.iter_mut().zip(&lfo.amplitude) {
                *gain = 1.0 - depth * fade * wave;
            }
        }
        self.age += 1;
        if self.hz.0 != cents.to_bits() {
            self.hz = (cents.to_bits(), cents_to_hz(cents));
        }
        let note = self.hz.1;

        let mut carriers_done = true;
        for (index, operator) in plan.operators.iter().enumerate() {
            let Some(o) = operator else { continue };
            let envelope = &mut self.envelopes[index];
            envelope.tick(self.held, &self.shapes[index]);
            carriers_done &= !o.carrier || envelope.stage >= 4;
        }

        let passes = plan.passes;
        let limit = 0.45 * plan.rate * passes as f64;
        let step = plan.rate * passes as f64;
        let unison = s.unison as usize;
        let mut out = [0.0; 2];
        for _ in 0..passes {
            out = [0.0; 2];
            for copy in 0..unison {
                // Outputs in turns of phase: full scale is 2.
                let mut outputs = [0.0; FM_OPERATORS];
                let mut sum = 0.0;
                for index in (0..FM_OPERATORS).rev() {
                    let Some(o) = &plan.operators[index] else {
                        continue;
                    };
                    let hz = o.fixed.unwrap_or(note * o.ratio * plan.ratios[copy]);
                    let mut modulation = 0.0;
                    for (source, output) in outputs.iter().enumerate().skip(index + 1) {
                        if o.modulators & (1 << source) != 0 {
                            modulation += output;
                        }
                    }
                    if index == plan.feedback_to {
                        let [older, last] = self.feedback[copy];
                        modulation += (older + last) * 0.5 * plan.feedback;
                    }
                    let phase = &mut self.phases[copy][index];
                    let gain = self.envelopes[index].gain * tremolo[index];
                    if hz <= limit && gain != 0.0 {
                        outputs[index] = sine(*phase + modulation) * gain * 2.0;
                    }
                    *phase += hz / step;
                    *phase -= phase.floor();
                    if o.carrier {
                        sum += outputs[index];
                    }
                }
                self.feedback[copy] = [self.feedback[copy][1], outputs[plan.feedback_from]];
                let pan = plan.pans[copy];
                out[0] += sum * (1.0 - pan.max(0.0));
                out[1] += sum * (1.0 + pan.min(0.0));
            }
            if passes > 1 {
                self.head = (self.head + 1) % (TAPS + 1);
                self.history[self.head] = out;
            }
        }
        if passes > 1 {
            out = [0.0; 2];
            for (i, tap) in plan.taps.iter().enumerate() {
                let past = self.history[(self.head + TAPS - i) % (TAPS + 1)];
                out[0] += past[0] * tap;
                out[1] += past[1] * tap;
            }
        }

        if !self.held {
            self.released += 1;
            if carriers_done || self.released >= plan.tail {
                self.sounding = false;
            }
        }
        let pan = get(ControlParameter::Pan)
            .unwrap_or(s.pan as f64 / 100.0)
            .clamp(-1.0, 1.0);
        let level = FM_CARRIER_PEAK / 2.0 / unison as f64;
        [
            out[0] * level * (1.0 - pan.max(0.0)),
            out[1] * level * (1.0 + pan.min(0.0)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_and_levels_match_the_documented_curves() {
        // Rate 0 falls one step every 4096 native samples: 0.2819 dB per second.
        let db = |rate| slope(rate, 0) * 6.020_599_913_279_624 / 256.0;
        assert!((db(0) - 0.2819).abs() < 1e-4);
        assert!((db(99) / db(0) - 7.0 / 4.0 * 32768.0).abs() < 1e-6);
        assert_eq!(slope(99, 5), slope(99, 0));
        assert_eq!(
            (scale(0), scale(19), scale(20), scale(99)),
            (0, 46, 48, 127)
        );
        assert_eq!(gain(FULL_SCALE), 1.0);
        assert_eq!(gain(FULL_SCALE - 256.0), 0.5);
        assert_eq!(gain(FLOOR), 0.0);
    }

    #[test]
    fn the_sine_polynomial_matches_the_platform() {
        for i in -4000..4000 {
            let x = i as f64 / 997.0;
            assert!(
                (sine(x) - (x * std::f64::consts::TAU).sin()).abs() < 2e-11,
                "{x}"
            );
        }
        assert_eq!(sine(0.0), 0.0);
        assert!((sine(0.25) - 1.0).abs() < 1e-11);
    }
}
