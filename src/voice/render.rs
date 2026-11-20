//! The prepared voice renderer: excitation, formants, noise and envelopes.
//!
//! Every value below is computed from the absolute source frame and retained
//! state, so any chunking of the output gives the same samples.
use super::plan::{FORMANT_BW, FORMANT_HZ, FORMANT_LIMIT, PreparedUtterance, RINGOUT, Segment};
use super::{Articulation, PreparedMarker, Rng, VoiceMode, VoiceSource, splitmix};
use crate::math::{cos_turns, exp, exp2, log2, sin_turns};
use crate::synth::{Aliasing, Oscillator, Waveform};
use crate::{Error, Result};

/// Frames between control updates (pitch, formant coefficients, modulation).
/// Updates also happen at every segment boundary.
const BLOCK: u64 = 16;
/// Magnitudes up to this pass the guard unchanged.
const GUARD_KNEE: f64 = 0.9;
/// Fixed gains that bring each path near the same level at `gain=1`.
const VOICED_GAIN: f64 = 1.1;
const SAW_GAIN: f64 = 0.85;
const ASPIRATION_GAIN: f64 = 1.6;
const FRICATION_GAIN: f64 = 0.5;
const ROBOT_NOISE_GAIN: f64 = 0.3;
/// Random-stream domains derived from the seed.
const ASPIRATION: u64 = 0x6173_7069;
const FRICATION: u64 = 0x6672_6963;
const JITTER: u64 = 0x6a69_7474;
const SHIMMER: u64 = 0x7368_696d;

/// A two-pole resonator, `y = a·x + b·y1 + c·y2`.
#[derive(Clone, Copy, Debug, Default)]
struct Resonator {
    a: f64,
    b: f64,
    c: f64,
    y1: f64,
    y2: f64,
}
impl Resonator {
    /// Unity gain at DC (Klatt's cascade resonator).
    fn tune(&mut self, hz: f64, bw: f64, rate: f64) {
        let r = exp(-std::f64::consts::PI * bw / rate);
        self.b = 2.0 * r * cos_turns(hz / rate);
        self.c = -r * r;
        self.a = 1.0 - self.b - self.c;
    }
    fn process(&mut self, x: f64) -> f64 {
        let y = self.a * x + self.b * self.y1 + self.c * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// A band-pass with zeros at DC and Nyquist and about unity peak gain.
#[derive(Clone, Copy, Debug, Default)]
struct BandPass {
    g: f64,
    b: f64,
    c: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}
impl BandPass {
    fn new(hz: f64, bw: f64, rate: f64) -> Self {
        let r = exp(-std::f64::consts::PI * bw / rate);
        Self {
            g: (1.0 - r * r) / 2.0,
            b: 2.0 * r * cos_turns(hz / rate),
            c: -r * r,
            ..Self::default()
        }
    }
    fn process(&mut self, x: f64) -> f64 {
        let y = self.g * (x - self.x2) + self.b * self.y1 + self.c * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

fn poly_blep(t: f64, dt: f64) -> f64 {
    if dt <= 0.0 {
        0.0
    } else if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// The smooth source: the derivative of a raised-cosine glottal flow pulse,
/// normalised so its negative peak is -1. The flow rises for two thirds of the
/// open phase and falls for one third; its derivative steps back to zero at
/// closure, which is band-limited with a PolyBLEP.
fn glottal(phase: f64, dt: f64, open: f64) -> f64 {
    let rise = open * (2.0 / 3.0);
    let fall = open - rise;
    let value = if phase < rise {
        (fall / rise) * sin_turns(phase / (2.0 * rise))
    } else if phase < open {
        -sin_turns((phase - rise) / (4.0 * fall))
    } else {
        0.0
    };
    let t = phase - open;
    let t = if t < 0.0 { t + 1.0 } else { t };
    value + 0.5 * poly_blep(t, dt)
}

/// A value that ramps linearly between seeded random points at a fixed period.
#[derive(Clone, Copy, Debug)]
struct Wander {
    rng: Rng,
    period: u64,
    from: f64,
    to: f64,
}
impl Wander {
    fn new(seed: u64, period: u64) -> Self {
        let mut rng = Rng::new(seed);
        let from = rng.signed();
        let to = rng.signed();
        Self {
            rng,
            period: period.max(1),
            from,
            to,
        }
    }
    /// Advances the random points at period boundaries of the absolute frame.
    fn step(&mut self, frame: u64) {
        if frame > 0 && frame % self.period == 0 {
            self.from = self.to;
            self.to = self.rng.signed();
        }
    }
    fn value(&self, frame: u64) -> f64 {
        let u = (frame % self.period) as f64 / self.period as f64;
        self.from + (self.to - self.from) * u
    }
}

/// Renders a [`PreparedUtterance`] into caller-owned buffers.
///
/// [`fill`](VoiceSynth::fill), [`reset`](VoiceSynth::reset), the marker queries
/// and the end of the utterance allocate nothing, take no locks and do no I/O, so
/// they are safe on an audio thread. Construction clones shared `Arc`s only.
/// Whole renders and any sequence of chunk sizes give identical samples.
#[derive(Clone, Debug)]
pub struct VoiceSynth {
    voice: PreparedUtterance,
    pos: u64,
    seg: usize,
    entered: bool,
    // Excitation.
    phase: f64,
    osc: Oscillator,
    hz: f64,
    /// The pitch before vibrato and jitter.
    base: f64,
    glide: (f64, f64, u64, u64),
    tilt_state: f64,
    tilt_a: f64,
    // Envelopes: voiced, aspiration, frication.
    env: [f64; 3],
    target: [f64; 3],
    rise: f64,
    fall: f64,
    release: f64,
    falling_by_release: bool,
    // Formants: current (hz, bw), transition from/to and progress.
    formant: [(f64, f64); 4],
    from: [(f64, f64); 4],
    to: [(f64, f64); 4],
    moved: u64,
    transition: u64,
    resonators: [Resonator; 4],
    norm: f64,
    fricative: BandPass,
    // Noise and modulation.
    aspiration: Rng,
    frication: Rng,
    jitter: Wander,
    shimmer: Wander,
    dc_x: f64,
    dc_y: f64,
    dc_r: f64,
    peak: f64,
}

impl VoiceSynth {
    /// A renderer at frame 0 of `voice`.
    pub fn new(voice: &PreparedUtterance) -> Self {
        let rate = voice.rate as f64;
        let p = &voice.profile;
        let ms = |ms: f64| (ms * rate / 1000.0).max(1.0);
        let tilt_hz = 12_000.0 * exp2(-p.tilt * 5.0);
        let seed = voice.seed;
        let mut synth = Self {
            voice: voice.clone(),
            pos: 0,
            seg: 0,
            entered: false,
            phase: 0.0,
            osc: Oscillator::new(Aliasing::BandLimited),
            hz: voice.initial_hz,
            base: voice.initial_hz,
            glide: (voice.initial_hz, 0.0, 0, 1),
            tilt_state: 0.0,
            tilt_a: if p.tilt > 0.0 {
                1.0 - exp(-std::f64::consts::TAU * tilt_hz.min(0.45 * rate) / rate)
            } else {
                1.0
            },
            env: [0.0; 3],
            target: [0.0; 3],
            rise: 1.0 / ms(p.attack),
            fall: 1.0 / ms(p.attack),
            release: 1.0 / ms(p.release),
            falling_by_release: false,
            formant: [(0.0, 0.0); 4],
            from: [(0.0, 0.0); 4],
            to: [(0.0, 0.0); 4],
            moved: 0,
            transition: ms(p.transition) as u64,
            resonators: [Resonator::default(); 4],
            norm: 1.0,
            fricative: BandPass::new(
                p.fricative.min(FORMANT_LIMIT * rate),
                0.6 * p.fricative.min(FORMANT_LIMIT * rate),
                rate,
            ),
            aspiration: Rng::new(splitmix(seed ^ ASPIRATION)),
            frication: Rng::new(splitmix(seed ^ FRICATION)),
            jitter: Wander::new(splitmix(seed ^ JITTER), (rate / 30.0) as u64),
            shimmer: Wander::new(splitmix(seed ^ SHIMMER), (rate / 20.0) as u64),
            dc_x: 0.0,
            dc_y: 0.0,
            dc_r: 1.0 - std::f64::consts::TAU * 20.0 / rate,
            peak: 0.0,
        };
        let first = voice.segments.first().map_or(super::Vowel::A, |s| s.vowel);
        let start = synth.vowel_target(first);
        synth.formant = start;
        synth.from = start;
        synth.to = start;
        synth.moved = synth.transition;
        synth
    }

    fn vowel_target(&self, vowel: super::Vowel) -> [(f64, f64); 4] {
        let p = &self.voice.profile;
        let mut out = [(0.0, 0.0); 4];
        for (k, slot) in out.iter_mut().enumerate() {
            *slot = (
                FORMANT_HZ[vowel.index()][k] * p.resonance,
                FORMANT_BW[k] * p.bandwidth * p.resonance,
            );
        }
        out
    }

    pub fn sample_rate(&self) -> u32 {
        self.voice.rate
    }
    pub fn channels(&self) -> usize {
        self.voice.channels
    }
    /// The full length in frames, tail included.
    pub fn frames(&self) -> u64 {
        self.voice.total
    }
    /// The next source frame to render.
    pub fn position(&self) -> u64 {
        self.pos
    }
    pub fn is_finished(&self) -> bool {
        self.pos >= self.voice.total
    }
    /// The prepared utterance this renders.
    pub fn utterance(&self) -> &PreparedUtterance {
        &self.voice
    }
    /// The prepared markers, without allocating.
    pub fn markers(&self) -> &[PreparedMarker] {
        &self.voice.markers
    }
    /// The largest magnitude seen before the output guard since the last reset.
    /// Values above 0.9 were bent by the guard.
    pub fn peak_before_guard(&self) -> f64 {
        self.peak
    }

    /// Returns to frame 0 with the initial state, so the next fill repeats the
    /// utterance exactly. Allocation-free.
    pub fn reset(&mut self) {
        // Cloning a prepared utterance clones `Arc`s only.
        *self = Self::new(&self.voice);
    }

    /// Moves to source frame `frame` by resetting and rendering silently up to it,
    /// so it costs time proportional to `frame`. Seeking to 0 is a cheap reset.
    /// Exact, allocation-free, but not constant-time: seek from the control thread
    /// when possible. A frame past the end is
    /// [`ErrorCode::InvalidArgument`](crate::ErrorCode::InvalidArgument).
    pub fn seek(&mut self, frame: u64) -> Result<()> {
        if frame > self.voice.total {
            return Err(Error::invalid("Seek beyond the end of the utterance"));
        }
        self.reset();
        while self.pos < frame {
            self.step();
        }
        Ok(())
    }

    /// Renders up to `out.len() / channels` whole frames, interleaved, and returns
    /// how many; 0 at the end. Samples after the last whole frame written are left
    /// untouched.
    pub fn fill(&mut self, out: &mut [f32]) -> usize {
        let channels = self.voice.channels;
        let room = (out.len() / channels) as u64;
        let frames = room.min(self.voice.total - self.pos) as usize;
        for frame in out[..frames * channels].chunks_exact_mut(channels) {
            let s = self.step() as f32;
            for sample in frame {
                *sample = s;
            }
        }
        frames
    }

    /// Enters the segment at `self.seg`, or the tail if there is none.
    fn enter(&mut self) {
        let segment: Option<Segment> = self.voice.segments.get(self.seg).copied();
        let p = self.voice.profile;
        let Some(s) = segment else {
            self.target = [0.0; 3];
            self.falling_by_release = true;
            self.glide = (self.base, 0.0, self.pos, 1);
            return;
        };
        let l = s.level;
        self.target = match s.articulation {
            Articulation::Voiced => [l, l * p.breath, 0.0],
            Articulation::Breath => [0.0, l, 0.0],
            Articulation::Fricative => [0.0, 0.0, l],
            Articulation::Closure => [0.0; 3],
        };
        self.falling_by_release = s.release;
        if s.articulation == Articulation::Voiced {
            let ratio = log2(s.hz[1] / s.hz[0]);
            self.glide = (s.hz[0], ratio, s.start, (s.end - s.start).max(1));
        } else {
            self.glide = (self.base, 0.0, s.start, 1);
        }
        let to = self.vowel_target(s.vowel);
        if to != self.to {
            self.from = self.formant;
            self.to = to;
            self.moved = 0;
        }
    }

    fn control(&mut self) {
        let rate = self.voice.rate as f64;
        let p = self.voice.profile;
        let (start_hz, ratio, start, len) = self.glide;
        let base = if ratio == 0.0 {
            start_hz
        } else {
            let u = ((self.pos - start) as f64 / len as f64).min(1.0);
            start_hz * exp2(ratio * u)
        };
        self.base = base;
        let mut cents = self.jitter.value(self.pos) * p.jitter;
        if p.vibrato > 0.0 {
            cents += p.vibrato * sin_turns(self.pos as f64 * p.vibrato_rate / rate);
        }
        let hz = if cents == 0.0 {
            base
        } else {
            base * exp2(cents / 1200.0)
        };
        self.hz = hz.min(0.45 * rate);
        if self.voice.formants.iter().any(|&on| on) {
            let u = if self.moved >= self.transition {
                1.0
            } else {
                self.moved as f64 / self.transition as f64
            };
            for k in 0..4 {
                let (fh, fb) = self.from[k];
                let (th, tb) = self.to[k];
                self.formant[k] = (fh + (th - fh) * u, fb + (tb - fb) * u);
                if self.voice.formants[k] {
                    let (hz, bw) = self.formant[k];
                    self.resonators[k].tune(hz, bw, rate);
                }
            }
            // Even out loudness across vowels: divide by F1's resonance gain.
            let (f1, b1) = self.formant[0];
            self.norm = (b1 / f1).min(1.0);
        }
    }

    /// Renders one frame and advances.
    fn step(&mut self) -> f64 {
        let n = self.pos;
        let mut changed = !self.entered;
        self.entered = true;
        if changed {
            self.enter();
        }
        while self
            .voice
            .segments
            .get(self.seg)
            .is_some_and(|s| n >= s.end)
        {
            self.seg += 1;
            self.enter();
            changed = true;
        }
        self.jitter.step(n);
        self.shimmer.step(n);
        if changed || n % BLOCK == 0 {
            self.control();
        }
        if self.moved < self.transition {
            self.moved += 1;
        }
        let rate = self.voice.rate;
        let p = self.voice.profile;
        // Excitation: always advanced, so phase never restarts.
        let dt = self.hz / rate as f64;
        let source = match p.mode {
            VoiceMode::Formant(VoiceSource::Smooth) => {
                let v = glottal(self.phase, dt, p.open);
                self.phase += dt;
                self.phase -= self.phase.floor();
                v * VOICED_GAIN
            }
            VoiceMode::Formant(VoiceSource::Saw) => {
                self.osc.next(Waveform::Sawtooth, self.hz, rate) * SAW_GAIN
            }
            VoiceMode::Robot(wave) => self.osc.next(wave, self.hz, rate),
        };
        let source = if p.tilt > 0.0 {
            self.tilt_state += self.tilt_a * (source - self.tilt_state);
            self.tilt_state
        } else {
            source
        };
        let asp = self.aspiration.signed();
        let fric = self.frication.signed();
        for k in 0..3 {
            let fall = if self.falling_by_release {
                self.release
            } else {
                self.fall
            };
            let (env, target) = (self.env[k], self.target[k]);
            self.env[k] = if env < target {
                (env + self.rise).min(target)
            } else {
                (env - fall).max(target)
            };
        }
        let shimmer = 1.0 + p.shimmer * self.shimmer.value(n);
        let voiced = self.env[0] * shimmer * source;
        let mut out = match p.mode {
            VoiceMode::Formant(_) => {
                let mut x = voiced + self.env[1] * asp * ASPIRATION_GAIN;
                for k in 0..4 {
                    if self.voice.formants[k] {
                        x = self.resonators[k].process(x);
                    }
                }
                x * self.norm
            }
            VoiceMode::Robot(_) => voiced + self.env[1] * asp * ROBOT_NOISE_GAIN,
        };
        out += self.fricative.process(self.env[2] * fric) * FRICATION_GAIN;
        // DC blocker, then the profile gain.
        let blocked = out - self.dc_x + self.dc_r * self.dc_y;
        self.dc_x = out;
        self.dc_y = blocked;
        let mut out = blocked * p.gain;
        // The final fade of the ring-out reaches exact zero on the last frame.
        let total = self.voice.total;
        let fade = ((RINGOUT * rate as f64 / 8.0).ceil() as u64).clamp(1, 256);
        if n + fade >= total {
            out *= (total - 1 - n) as f64 / fade as f64;
        }
        let magnitude = out.abs();
        if magnitude > self.peak {
            self.peak = magnitude;
        }
        let guarded = if magnitude <= GUARD_KNEE {
            out
        } else {
            let t = (magnitude - GUARD_KNEE) / (1.0 - GUARD_KNEE);
            let bent = GUARD_KNEE + (1.0 - GUARD_KNEE) * t / (1.0 + t);
            if out < 0.0 { -bent } else { bent }
        };
        self.pos += 1;
        guarded
    }
}

#[cfg(feature = "mixer")]
#[cfg_attr(docsrs, doc(cfg(feature = "mixer")))]
/// A prepared voice plays as a mixer stream of known length. Seeking to 0 (a
/// whole loop, a restart) is a cheap reset. Other positions replay silently up to
/// the frame: exact and allocation-free but proportional to the position, so
/// prefer a start position (sought on the game's thread) or render the utterance
/// to [`Audio`](crate::Audio) for region loops.
impl crate::mixer::Stream for VoiceSynth {
    fn sample_rate(&self) -> u32 {
        self.voice.rate
    }
    fn channels(&self) -> usize {
        self.voice.channels
    }
    fn frames(&self) -> Option<u64> {
        Some(self.voice.total)
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        Ok(self.fill(out))
    }
    fn seek(&mut self, frame: u64) -> Result<()> {
        VoiceSynth::seek(self, frame)
    }
}

#[cfg(feature = "mixer")]
impl From<VoiceSynth> for crate::mixer::Sound {
    fn from(synth: VoiceSynth) -> Self {
        Self::stream(synth)
    }
}

#[cfg(feature = "mixer")]
impl From<&PreparedUtterance> for crate::mixer::Sound {
    fn from(voice: &PreparedUtterance) -> Self {
        Self::stream(voice.synth())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glottal_pulse_is_zero_mean_and_bounded() {
        let n = 4800;
        let dt = 1.0 / n as f64;
        let mut sum = 0.0;
        let mut peak: f64 = 0.0;
        for i in 0..n {
            let v = glottal(i as f64 * dt, dt, 0.6);
            sum += v;
            peak = peak.max(v.abs());
        }
        assert!((sum / n as f64).abs() < 1e-3, "mean {}", sum / n as f64);
        assert!(peak <= 1.0 + 1e-9);
    }

    /// Magnitude of the cascade's response at `hz`.
    fn response(res: &[Resonator], hz: f64, rate: f64) -> f64 {
        let w = std::f64::consts::TAU * hz / rate;
        let (mut re, mut im) = (1.0, 0.0);
        for r in res {
            // a / (1 - b z^-1 - c z^-2) at z = e^{jw}.
            let dr = 1.0 - r.b * w.cos() - r.c * (2.0 * w).cos();
            let di = r.b * w.sin() + r.c * (2.0 * w).sin();
            let den = dr * dr + di * di;
            let (nr, ni) = (r.a * dr / den, -r.a * di / den);
            (re, im) = (re * nr - im * ni, re * ni + im * nr);
        }
        (re * re + im * im).sqrt()
    }

    #[test]
    fn cascade_peaks_sit_on_the_vowel_targets() {
        for rate in [16_000.0, 44_100.0, 48_000.0, 96_000.0] {
            for (v, targets) in FORMANT_HZ.iter().enumerate() {
                let mut res = [Resonator::default(); 4];
                for k in 0..4 {
                    res[k].tune(targets[k], FORMANT_BW[k], rate);
                }
                let active = targets
                    .iter()
                    .filter(|&&f| f <= FORMANT_LIMIT * rate)
                    .count();
                for (k, &target) in targets.iter().enumerate().take(active).take(2) {
                    // Search ±20% around the target for the local maximum.
                    let (mut best, mut best_hz) = (0.0, 0.0);
                    let mut hz = target * 0.8;
                    while hz <= target * 1.2 {
                        let m = response(&res[..active], hz, rate);
                        if m > best {
                            (best, best_hz) = (m, hz);
                        }
                        hz += 1.0;
                    }
                    let error = (best_hz - target).abs() / target;
                    assert!(
                        error < 0.06,
                        "rate {rate} vowel {v} F{}: {best_hz} vs {target}",
                        k + 1
                    );
                }
            }
        }
    }
}
