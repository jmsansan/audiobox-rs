//! Prepared bus effect chains. All times and parameter ramps are sample based.
//!
//! Install with [`MixerHandle::set_effects`], then update a matching slot with
//! [`MixerHandle::set_effect`]. Slots are zero based and follow the slice order.
//! Structural edits are immediate and reset the new chain's state; fade the bus
//! around them when needed. Parameter ramps preserve filter/delay/reverb history.
//! A delay-time ramp moves the read head and intentionally changes pitch.
//! Chorus, flanger and phaser share retained-phase [`ModulationRate`] automation.
//! [`Bitcrusher`] deliberately quantizes and holds samples; [`Saturation`] applies
//! a cubic soft curve. These colour effects do not oversample or suppress aliasing.
//! Parameter smoothing does not remove their intentional waveform discontinuities.
//!
//! Named buses run in sidechain dependency order, effects before the bus fader.
//! Detectors hear the key bus after its effects, gain and pause fade, in the same
//! frame. All named buses then feed master effects, master gain and the limiter.
//! Self sidechains, master as a key, and cycles are refused. Application-time
//! failures arrive as [`MixerEvent::EffectsRejected`]; the previous chain survives.
//!
//! A paused bus freezes its effect buffers and parameter clock. Tails continue
//! after its voices end, until the bus is paused or the chain is removed. Sidechain
//! attack/release are exponential time constants (63.2% after one time constant).
//! Stereo detectors use the louder channel and apply one gain to both channels.
//!
//! Buffers are prepared on the handle. At most `MixerConfig::queue` chain edits may
//! await reclamation: poll events to release old chains and permit more edits.
//! Even if events are not polled or the handle is dropped, rendering retains these
//! buffers rather than freeing them. Dropping the mixer itself frees its resources.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::queue::{self, Consumer, Producer, PushError};
use super::voice::Ramp;
use super::{Action, Bus, BusId, Commands, Fade, Mixer, MixerEvent, MixerHandle};
use crate::math::{cos_turns, db_to_gain, exp, log2, sin_turns};
use crate::{Error, ErrorCode, FadeCurve, Result};

mod colour;
pub use colour::{Bitcrusher, Chorus, Flanger, ModulationRate, Phaser, Saturation};

/// Maximum effects per bus, including the master.
pub const MAX_EFFECTS: usize = 16;

/// Delay time in seconds or beats with an explicit tempo. To follow a tempo change,
/// send updated settings with the new BPM; the time follows the requested ramp.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EffectTime {
    Seconds(f64),
    Beats { beats: f64, bpm: f64 },
}
impl EffectTime {
    fn seconds(self) -> Result<f64> {
        let s = match self {
            Self::Seconds(s) => s,
            Self::Beats { beats, bpm } => {
                if !(1.0..=1000.0).contains(&bpm) || !beats.is_finite() || beats <= 0.0 {
                    return Err(Error::invalid(
                        "Effect beats must be positive, BPM 1..=1000",
                    ));
                }
                beats * 60.0 / bpm
            }
        };
        if !s.is_finite() || s <= 0.0 {
            return Err(Error::invalid("Effect time must be finite and positive"));
        }
        Ok(s)
    }
}

/// Hard-knee peak compressor. `sidechain: None` detects this effect's input.
/// `depth` blends gain reduction with unity; 0 disables reduction, 1 is full depth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Compressor {
    /// -96..=0 dBFS.
    pub threshold_db: f32,
    /// 1..=100.
    pub ratio: f32,
    /// 0..=5000 ms; zero is instantaneous.
    pub attack_ms: f32,
    /// 0..=5000 ms; zero is instantaneous.
    pub release_ms: f32,
    /// -24..=24 dB.
    pub makeup_db: f32,
    /// 0..=1.
    pub depth: f32,
    pub sidechain: Option<BusId>,
}
impl Default for Compressor {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            makeup_db: 0.0,
            depth: 1.0,
            sidechain: None,
        }
    }
}
impl Compressor {
    /// A strong sidechain compressor, suitable for music under dialogue.
    pub fn ducking(key: BusId) -> Self {
        Self {
            threshold_db: -30.0,
            ratio: 20.0,
            attack_ms: 5.0,
            release_ms: 250.0,
            sidechain: Some(key),
            ..Self::default()
        }
    }
}

/// Fractional-sample delay with linear interpolation and optional negative feedback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Delay {
    pub time: EffectTime,
    /// Capacity allocated at installation, 1 sample..=10 seconds. Updates may use
    /// this capacity but cannot enlarge it without installing a new chain.
    pub max_seconds: f64,
    /// -0.99..=0.99.
    pub feedback: f32,
    /// Linear dry/wet blend, 0..=1.
    pub mix: f32,
}
impl Default for Delay {
    fn default() -> Self {
        Self {
            time: EffectTime::Seconds(0.25),
            max_seconds: 2.0,
            feedback: 0.35,
            mix: 0.25,
        }
    }
}

/// Four parallel feedback combs and two allpass diffusers per channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reverb {
    /// Nominal -60 dB decay time, 0.05..=20 seconds, measured with damping at zero.
    pub decay_seconds: f32,
    /// Feedback low-pass memory, 0..=0.99; higher values shorten high-frequency tails.
    pub damping: f32,
    /// Linear dry/wet blend, 0..=1.
    pub mix: f32,
}
impl Default for Reverb {
    fn default() -> Self {
        Self {
            decay_seconds: 1.5,
            damping: 0.25,
            mix: 0.2,
        }
    }
}

/// A chain slot's typed settings. New variants can share the same preparation,
/// automation, sidechain and lifetime machinery.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Effect {
    /// First-order topology-preserving low-pass, -3 dB at cutoff, 6 dB/octave.
    LowPass {
        cutoff_hz: f32,
    },
    /// First-order topology-preserving high-pass, -3 dB at cutoff, 6 dB/octave.
    HighPass {
        cutoff_hz: f32,
    },
    Compressor(Compressor),
    Delay(Delay),
    Reverb(Reverb),
    Chorus(Chorus),
    Flanger(Flanger),
    Phaser(Phaser),
    Bitcrusher(Bitcrusher),
    Saturation(Saturation),
}

#[derive(Clone, Copy)]
pub(super) struct Settings {
    kind: u8,
    values: [f64; 6],
    key: Option<usize>,
    capacity: f64,
}
impl Effect {
    fn settings(self, rate: u32, bus: BusId, buses: usize) -> Result<Settings> {
        let mut s = Settings {
            kind: 0,
            values: [0.0; 6],
            key: None,
            capacity: 0.0,
        };
        match self {
            Self::LowPass { cutoff_hz } | Self::HighPass { cutoff_hz } => {
                within(cutoff_hz as f64, 1.0, rate as f64 * 0.45)?;
                s.kind = u8::from(matches!(self, Self::HighPass { .. }));
                s.values[0] = cutoff_hz as f64;
            }
            Self::Compressor(c) => {
                s.kind = 2;
                for (v, lo, hi) in [
                    (c.threshold_db, -96.0, 0.0),
                    (c.ratio, 1.0, 100.0),
                    (c.attack_ms, 0.0, 5000.0),
                    (c.release_ms, 0.0, 5000.0),
                    (c.makeup_db, -24.0, 24.0),
                    (c.depth, 0.0, 1.0),
                ] {
                    within(v as f64, lo, hi)?;
                }
                s.values = [
                    c.threshold_db as f64,
                    c.ratio as f64,
                    c.attack_ms as f64,
                    c.release_ms as f64,
                    c.makeup_db as f64,
                    c.depth as f64,
                ];
                if let Some(key) = c.sidechain {
                    if key.0 == 0 || key == bus || key.0 >= buses {
                        return Err(Error::invalid("Sidechain needs another named bus"));
                    }
                    s.key = Some(key.0);
                }
            }
            Self::Delay(d) => {
                s.kind = 3;
                within(d.max_seconds, 1.0 / rate as f64, 10.0)?;
                let time = d.time.seconds()?;
                within(time, 1.0 / rate as f64, d.max_seconds)?;
                within(d.feedback as f64, -(0.99f32 as f64), 0.99f32 as f64)?;
                within(d.mix as f64, 0.0, 1.0)?;
                s.values = [
                    time * rate as f64,
                    d.feedback as f64,
                    d.mix as f64,
                    0.0,
                    0.0,
                    0.0,
                ];
                s.capacity = d.max_seconds * rate as f64;
            }
            Self::Reverb(r) => {
                s.kind = 4;
                within(r.decay_seconds as f64, 0.05, 20.0)?;
                within(r.damping as f64, 0.0, 0.99f32 as f64)?;
                within(r.mix as f64, 0.0, 1.0)?;
                s.values = [
                    r.decay_seconds as f64,
                    r.damping as f64,
                    r.mix as f64,
                    0.0,
                    0.0,
                    0.0,
                ];
            }
            Self::Chorus(c) => {
                s.kind = 5;
                s.values = c.values(rate)?;
            }
            Self::Flanger(f) => {
                s.kind = 6;
                s.values = f.values(rate)?;
            }
            Self::Phaser(p) => {
                s.kind = 7;
                s.values = p.values(rate)?;
            }
            Self::Bitcrusher(b) => {
                s.kind = 8;
                s.values = b.values()?;
            }
            Self::Saturation(d) => {
                s.kind = 9;
                s.values = d.values()?;
            }
        }
        Ok(s)
    }
}
fn within(value: f64, lo: f64, hi: f64) -> Result<()> {
    if !(lo..=hi).contains(&value) {
        Err(Error::invalid(
            "Effect parameter is non-finite or outside its documented range",
        ))
    } else {
        Ok(())
    }
}

pub(super) struct Chain {
    nodes: Vec<Node>,
}
struct Node {
    settings: Settings,
    ramps: [Ramp; 6],
    state: State,
}
enum State {
    Colour(colour::Processor),
    Filter {
        memory: [f64; 8],
        cutoff: f64,
        coefficient: f64,
    },
    Compressor {
        gain: f64,
        threshold: Cache,
        makeup: Cache,
        attack: Cache,
        release: Cache,
    },
    Delay {
        ring: Vec<f64>,
        at: usize,
    },
    Reverb {
        channels: Vec<ReverbChannel>,
    },
}
struct Line {
    ring: Vec<f64>,
    at: usize,
    memory: f64,
}
impl Line {
    fn new(len: usize) -> Self {
        Self {
            ring: vec![0.0; len.max(1)],
            at: 0,
            memory: 0.0,
        }
    }
    fn get(&self) -> f64 {
        self.ring[self.at]
    }
    fn put(&mut self, value: f64) {
        self.ring[self.at] = value;
        self.at = (self.at + 1) % self.ring.len();
    }
}
struct ReverbChannel {
    combs: [Line; 4],
    diffusers: [Line; 2],
    decay: f64,
    feedback: [f64; 4],
}
struct Cache {
    input: f64,
    output: f64,
}
impl Cache {
    fn new() -> Self {
        Self {
            input: f64::NAN,
            output: 0.0,
        }
    }
    fn get(&mut self, input: f64, compute: impl FnOnce(f64) -> f64) -> f64 {
        if self.input != input {
            self.input = input;
            self.output = compute(input);
        }
        self.output
    }
}

impl Chain {
    fn new(effects: &[Effect], rate: u32, ch: usize, bus: BusId, buses: usize) -> Result<Self> {
        if effects.len() > MAX_EFFECTS {
            return Err(Error::limit("Too many effects on a bus"));
        }
        // Validate the whole chain before allocating any DSP buffers.
        let settings: Vec<_> = effects
            .iter()
            .map(|e| e.settings(rate, bus, buses))
            .collect::<Result<_>>()?;
        let nodes = settings
            .into_iter()
            .map(|s| {
                let state = match s.kind {
                    0 | 1 => State::Filter {
                        memory: [0.0; 8],
                        cutoff: 0.0,
                        coefficient: 0.0,
                    },
                    2 => State::Compressor {
                        gain: 1.0,
                        threshold: Cache::new(),
                        makeup: Cache::new(),
                        attack: Cache::new(),
                        release: Cache::new(),
                    },
                    3 => State::Delay {
                        ring: vec![0.0; (s.capacity.ceil() as usize + 2) * ch],
                        at: 0,
                    },
                    4 => State::Reverb {
                        channels: (0..ch)
                            .map(|c| {
                                let line = |seconds: f64| {
                                    Line::new(((seconds + c as f64 * 0.0011) * rate as f64).round()
                                        as usize)
                                };
                                ReverbChannel {
                                    combs: [line(0.0297), line(0.0371), line(0.0411), line(0.0437)],
                                    diffusers: [line(0.005), line(0.0017)],
                                    decay: 0.0,
                                    feedback: [0.0; 4],
                                }
                            })
                            .collect(),
                    },
                    _ => State::Colour(colour::Processor::new(s.kind, rate, ch)),
                };
                Node {
                    settings: s,
                    ramps: s.values.map(Ramp::constant),
                    state,
                }
            })
            .collect();
        Ok(Self { nodes })
    }
    fn accepts(&self, slot: usize, settings: Settings) -> bool {
        self.nodes.get(slot).is_some_and(|n| {
            n.settings.kind == settings.kind
                && (settings.kind != 3 || settings.capacity <= n.settings.capacity)
        })
    }
    fn update(&mut self, slot: usize, settings: Settings, clock: u64, len: u64, curve: FadeCurve) {
        let n = &mut self.nodes[slot];
        // Delay capacity belongs to preparation, not parameter automation.
        n.settings.key = settings.key;
        for (r, value) in n.ramps.iter_mut().zip(settings.values) {
            r.retarget(clock, value, len, curve);
        }
    }
}

impl Node {
    fn process(&mut self, clock: u64, rate: u32, ch: usize, out: &mut [f32], key: Option<&[f32]>) {
        let kind = self.settings.kind;
        for (i, frame) in out.chunks_exact_mut(ch).enumerate() {
            let p = self.ramps.map(|r| r.value(clock + i as u64));
            let mut x = [0.0; 8];
            for c in 0..ch {
                x[c] = finite(frame[c] as f64);
            }
            match &mut self.state {
                State::Colour(processor) => processor.process(&mut x, p, rate, ch),
                State::Filter {
                    memory,
                    cutoff,
                    coefficient,
                } => {
                    if *cutoff != p[0] {
                        let turns = p[0] / (2.0 * rate as f64);
                        let g = sin_turns(turns) / cos_turns(turns);
                        *coefficient = g / (1.0 + g);
                        *cutoff = p[0];
                    }
                    for c in 0..ch {
                        let v = (x[c] - memory[c]) * *coefficient;
                        let low = v + memory[c];
                        memory[c] = low + v;
                        x[c] = if kind == 0 { low } else { x[c] - low };
                    }
                }
                State::Compressor {
                    gain,
                    threshold,
                    makeup,
                    attack,
                    release,
                } => {
                    let peak = if let Some(key) = key {
                        key[i * ch..i * ch + ch]
                            .iter()
                            .map(|&v| finite(v as f64).abs())
                            .fold(0.0, f64::max)
                    } else {
                        x[..ch].iter().map(|v| v.abs()).fold(0.0, f64::max)
                    };
                    let threshold = threshold.get(p[0], db_to_gain);
                    let reduction = if peak > threshold {
                        crate::math::exp2(log2(threshold / peak) * (1.0 - 1.0 / p[1]))
                    } else {
                        1.0
                    };
                    let target = 1.0 - p[5] * (1.0 - reduction);
                    let coefficient = |ms| {
                        if ms == 0.0 {
                            0.0
                        } else {
                            exp(-1000.0 / (ms * rate as f64))
                        }
                    };
                    let a = if target < *gain {
                        attack.get(p[2], coefficient)
                    } else {
                        release.get(p[3], coefficient)
                    };
                    *gain = target + (*gain - target) * a;
                    let g = *gain * makeup.get(p[4], db_to_gain);
                    for s in &mut x[..ch] {
                        *s *= g;
                    }
                }
                State::Delay { ring, at } => {
                    let frames = ring.len() / ch;
                    let mut pos = *at as f64 - p[0];
                    if pos < 0.0 {
                        pos += frames as f64;
                    }
                    let a = pos.floor() as usize;
                    let b = (a + 1) % frames;
                    let f = pos - a as f64;
                    for c in 0..ch {
                        let delayed = ring[a * ch + c] * (1.0 - f) + ring[b * ch + c] * f;
                        ring[*at * ch + c] = finite(x[c] + delayed * p[1]);
                        x[c] = x[c] * (1.0 - p[2]) + delayed * p[2];
                    }
                    *at = (*at + 1) % frames;
                }
                State::Reverb { channels } => {
                    for c in 0..ch {
                        let channel = &mut channels[c];
                        if channel.decay != p[0] {
                            channel.decay = p[0];
                            for (feedback, line) in channel.feedback.iter_mut().zip(&channel.combs)
                            {
                                *feedback = db_to_gain(
                                    -60.0 * line.ring.len() as f64 / (rate as f64 * p[0]),
                                );
                            }
                        }
                        let mut wet = 0.0;
                        for (line, &feedback) in channel.combs.iter_mut().zip(&channel.feedback) {
                            let d = line.get();
                            line.memory = d * (1.0 - p[1]) + line.memory * p[1];
                            line.put(finite(x[c] + line.memory * feedback));
                            wet += d * 0.25;
                        }
                        for line in &mut channel.diffusers {
                            let value = line.get() - wet * 0.5;
                            line.put(wet + value * 0.5);
                            wet = value;
                        }
                        x[c] = x[c] * (1.0 - p[2]) + wet * p[2];
                    }
                }
            }
            for c in 0..ch {
                frame[c] = finite(x[c]) as f32;
            }
        }
    }
}
fn finite(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(-(f32::MAX as f64), f32::MAX as f64)
    }
}

// Each owning edit has one credit until the handle destroys the returned box.
// Installing swaps its chain with the bus's; the SAME box comes back, carrying the
// old chain (or the refused new chain). Thus even endless unpolled renders cannot
// overflow retirement storage or destroy DSP memory in render.
pub(super) struct Edit {
    bus: usize,
    chain: Option<Box<Chain>>,
    event: Option<MixerEvent>,
    credits: Arc<AtomicUsize>,
}
impl Drop for Edit {
    fn drop(&mut self) {
        self.credits.fetch_sub(1, Ordering::Release);
    }
}
pub(super) struct AudioEffects {
    returns: Producer<Box<Edit>>,
    retired: VecDeque<Box<Edit>>,
    order: [usize; 256],
}
pub(super) struct ControlEffects {
    returns: Consumer<Box<Edit>>,
    credits: Arc<AtomicUsize>,
    capacity: usize,
}
pub(super) fn create(capacity: usize, buses: usize) -> (AudioEffects, ControlEffects) {
    let (tx, rx) = queue::queue(capacity);
    let mut order = [0; 256];
    for (i, b) in order[..buses - 1].iter_mut().enumerate() {
        *b = i + 1;
    }
    (
        AudioEffects {
            returns: tx,
            retired: VecDeque::with_capacity(capacity),
            order,
        },
        ControlEffects {
            returns: rx,
            credits: Arc::new(AtomicUsize::new(0)),
            capacity,
        },
    )
}
impl AudioEffects {
    pub(super) fn send_returns(&mut self) {
        while let Some(edit) = self.retired.pop_front() {
            match self.returns.push(edit) {
                Ok(()) => (),
                Err(PushError::Full(edit) | PushError::Disconnected(edit)) => {
                    self.retired.push_front(edit);
                    break;
                }
            }
        }
    }
}
impl ControlEffects {
    pub(super) fn poll(&mut self) -> Option<MixerEvent> {
        let edit = self.returns.pop()?;
        edit.event
        // The box and its old DSP storage die here, on the control thread.
    }
}

impl MixerHandle {
    /// Replaces a bus's chain. Preparation may allocate; installation is atomic and
    /// sample scheduled. An empty slice removes the chain and its tail immediately.
    pub fn set_effects(&mut self, bus: BusId, effects: &[Effect]) -> Result<()> {
        self.now().set_effects(bus, effects)
    }
    /// Ramps all numeric settings of an existing matching slot in place. Type or
    /// capacity mismatches and dependency cycles are reported asynchronously.
    /// Changing the sidechain key is immediate; gain reduction uses attack/release.
    pub fn set_effect(
        &mut self,
        bus: BusId,
        slot: usize,
        effect: Effect,
        fade: Fade,
    ) -> Result<()> {
        self.now().set_effect(bus, slot, effect, fade)
    }
}
impl Commands<'_> {
    pub fn set_effects(self, bus: BusId, effects: &[Effect]) -> Result<()> {
        if bus.0 > self.handle.buses.len() {
            return Err(Error::invalid("Unknown bus"));
        }
        let fx = &self.handle.effects;
        if fx.credits.load(Ordering::Acquire) >= fx.capacity {
            return Err(Error::limit(
                "Poll mixer events before sending more effect chain edits",
            ));
        }
        let chain = Chain::new(
            effects,
            self.handle.rate,
            self.handle.channels,
            bus,
            self.handle.buses.len() + 1,
        )?;
        let chain = if effects.is_empty() {
            None
        } else {
            Some(Box::new(chain))
        };
        fx.credits.fetch_add(1, Ordering::Relaxed);
        let edit = Box::new(Edit {
            bus: bus.0,
            chain,
            event: None,
            credits: fx.credits.clone(),
        });
        self.handle.send(self.frame, Action::Effects(edit))
    }
    pub fn set_effect(self, bus: BusId, slot: usize, effect: Effect, fade: Fade) -> Result<()> {
        if bus.0 > self.handle.buses.len() || slot >= MAX_EFFECTS {
            return Err(Error::invalid("Unknown bus or effect slot"));
        }
        let settings = effect.settings(self.handle.rate, bus, self.handle.buses.len() + 1)?;
        let (len, curve) = self.handle.frames(fade)?;
        self.handle.send(
            self.frame,
            Action::Effect(bus.0, slot, settings, len, curve),
        )
    }
}

enum Change<'a> {
    Chain(usize, Option<&'a Chain>),
    Parameter(usize, usize, Settings),
}
// Kahn sort with fixed storage, evaluated against actual application-time chains.
// Every named bus feeds master, so master is always last; master cannot be a key.
fn topology(buses: &[Bus], change: Change<'_>) -> Option<[usize; 256]> {
    let mut done = [false; 256];
    let mut order = [0; 256];
    for entry in &mut order[..buses.len() - 1] {
        let ready = (1..buses.len()).find(|&b| {
            if done[b] {
                return false;
            }
            let chain = match change {
                Change::Chain(target, chain) if target == b => chain,
                _ => buses[b].effects.as_deref(),
            };
            chain.is_none_or(|chain| {
                chain.nodes.iter().enumerate().all(|(slot, node)| {
                    let key = match change {
                        Change::Parameter(target, index, settings)
                            if target == b && index == slot =>
                        {
                            settings.key
                        }
                        _ => node.settings.key,
                    };
                    key.is_none_or(|key| done[key])
                })
            })
        })?;
        *entry = ready;
        done[ready] = true;
    }
    Some(order)
}

impl Mixer {
    pub(super) fn apply_effects(&mut self, frame: u64, mut edit: Box<Edit>) {
        let bus = edit.bus;
        if let Some(order) = topology(&self.buses, Change::Chain(bus, edit.chain.as_deref())) {
            self.effects.order = order;
            std::mem::swap(&mut self.buses[bus].effects, &mut edit.chain);
            edit.event = Some(MixerEvent::EffectsApplied(BusId(bus), frame));
        } else {
            edit.event = Some(MixerEvent::EffectsRejected(
                BusId(bus),
                ErrorCode::InvalidArgument,
            ));
        }
        self.effects.retired.push_back(edit);
    }
    pub(super) fn apply_effect(
        &mut self,
        bus: usize,
        slot: usize,
        settings: Settings,
        len: u64,
        curve: FadeCurve,
    ) {
        let valid = self.buses[bus]
            .effects
            .as_ref()
            .is_some_and(|c| c.accepts(slot, settings));
        let order = valid
            .then(|| topology(&self.buses, Change::Parameter(bus, slot, settings)))
            .flatten();
        if let Some(order) = order {
            self.effects.order = order;
            let bus = &mut self.buses[bus];
            bus.effects
                .as_mut()
                .unwrap()
                .update(slot, settings, bus.effect_clock, len, curve);
        } else {
            self.event(MixerEvent::EffectsRejected(
                BusId(bus),
                ErrorCode::InvalidArgument,
            ));
        }
    }

    /// Effects and routing for a segment, using post-effect/post-fader key buses.
    pub(super) fn mix_effects(&mut self, first: u64, out: &mut [f32]) {
        let ch = self.channels;
        let frames = out.len() / ch;
        let master_paused = self.buses[0].paused;
        for index in 0..self.buses.len() {
            let b = self.effects.order[index];
            if b == 0 {
                for bus in &self.buses[1..] {
                    for (o, &s) in out.iter_mut().zip(&bus.buffer) {
                        *o += s;
                    }
                }
                for (o, &s) in out.iter_mut().zip(&self.buses[0].buffer) {
                    *o += s;
                }
                self.buses[0].buffer[..out.len()].copy_from_slice(out);
            }
            // Split borrowing around the target, then supply key buffers through a
            // small stack view rather than copying any audio or allocating a Vec.
            let (before, rest) = self.buses.split_at_mut(b);
            let (bus, after) = rest.split_first_mut().unwrap();
            let buffer = &mut bus.buffer[..out.len()];
            if master_paused || bus.paused {
                buffer.fill(0.0);
            } else {
                if let Some(chain) = &mut bus.effects {
                    // Each node only needs its own key; splitting gives disjoint borrows.
                    for node in &mut chain.nodes {
                        let key = node.settings.key.map(|key| {
                            if key < b {
                                &before[key].buffer[..out.len()]
                            } else {
                                &after[key - b - 1].buffer[..out.len()]
                            }
                        });
                        node.process(bus.effect_clock, self.rate, ch, buffer, key);
                    }
                }
                bus.effect_clock += frames as u64;
                for (i, frame) in buffer.chunks_exact_mut(ch).enumerate() {
                    let at = first + i as u64;
                    let gain = bus.gain.value(at) * bus.pause.value(at);
                    for sample in frame {
                        *sample = (*sample as f64 * gain) as f32;
                    }
                }
            }
            if b == 0 {
                out.copy_from_slice(buffer);
            }
        }
    }
}
