use super::chip::{ChipPlan, ChipVoice};
use super::drum::{DrumPlan, DrumVoice};
use super::score::{Instrument, Score};
use super::wave::{Aliasing, Duty, Oscillator, Waveform};
use crate::math::cents_to_hz;
use crate::{Audio, Error, Limits, Result};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions {
    pub aliasing: Aliasing,
    /// Times the loop section repeats after the first pass through the song, or
    /// `None` to loop forever (incremental rendering only). The default is `Some(0)`.
    pub loops: Option<u32>,
    /// Linear gain applied to the sum of the channels. The default, 0.25, lets
    /// four channels at full volume reach 1.0.
    pub gain: f32,
}
impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            aliasing: Aliasing::BandLimited,
            loops: Some(0),
            gain: 0.25,
        }
    }
}

impl Score {
    /// Renders the song once through, followed by the release of its last notes.
    pub fn render(&self, sample_rate: u32) -> Result<Audio> {
        self.render_with(sample_rate, &RenderOptions::default())
    }
    /// Renders the song into mono audio. Equal to concatenating every chunk of a [`Synth`].
    pub fn render_with(&self, sample_rate: u32, options: &RenderOptions) -> Result<Audio> {
        let mut synth = Synth::new(self, sample_rate, options)?;
        let frames = synth
            .max_frames()
            .ok_or_else(|| Error::invalid("Cannot render an endless loop offline"))?;
        let frames = usize::try_from(frames).map_err(|_| Error::limit("Song is too long"))?;
        Limits::default().check(frames, 1, sample_rate)?;
        let mut samples = vec![0.0; frames];
        let written = synth.fill(&mut samples);
        samples.truncate(written);
        Audio::new(vec![samples], sample_rate)
    }
}

#[derive(Clone, Debug)]
struct Plan {
    chip: Option<ChipPlan>,
    drum: Option<DrumPlan>,
    waveform: Waveform,
    attack: u64,
    decay: u64,
    sustain: f64,
    release: u64,
    volume: f64,
    slide: f64,
    vibrato: Option<(f64, u64, u64)>,
    arpeggio: Option<(Vec<i8>, u64)>,
    duty_sweep: Option<(Vec<Duty>, u64)>,
}
impl Plan {
    fn new(instrument: &Instrument, rate: u32, tempo: u16, ticks: u16) -> Self {
        let samples = |ms: u32| (ms as u64 * rate as u64 + 500) / 1000;
        let step = |ms: u32| samples(ms).max(1);
        let e = instrument.envelope;
        Self {
            chip: instrument
                .chip
                .as_ref()
                .map(|chip| ChipPlan::new(chip, rate, tempo, ticks)),
            drum: instrument.drum.map(|d| DrumPlan::new(d, rate)),
            waveform: instrument.waveform,
            attack: samples(e.attack_ms),
            decay: samples(e.decay_ms),
            sustain: e.sustain as f64 / 100.0,
            release: samples(e.release_ms),
            volume: instrument.volume as f64 / 15.0,
            slide: instrument.slide as f64 / rate as f64,
            vibrato: instrument
                .vibrato
                .map(|v| (v.depth_cents as f64, step(v.period_ms), samples(v.delay_ms))),
            arpeggio: instrument
                .arpeggio
                .as_ref()
                .map(|a| (a.offsets.clone(), step(a.step_ms))),
            duty_sweep: instrument
                .duty_sweep
                .as_ref()
                .map(|d| (d.duties.clone(), step(d.step_ms))),
        }
    }
    fn modulated(&self) -> bool {
        self.slide != 0.0 || self.vibrato.is_some() || self.arpeggio.is_some()
    }
    fn cents(&self, base: f64, elapsed: u64) -> f64 {
        let mut cents = base + self.slide * elapsed as f64;
        if let Some((depth, period, delay)) = self.vibrato {
            if elapsed >= delay {
                let phase = ((elapsed - delay) % period) as f64 / period as f64;
                cents += depth * crate::math::sin_turns(phase);
            }
        }
        if let Some((offsets, step)) = &self.arpeggio {
            cents += offsets[(elapsed / step) as usize % offsets.len()] as f64 * 100.0;
        }
        cents
    }
    fn waveform(&self, elapsed: u64) -> Waveform {
        match (&self.duty_sweep, self.waveform) {
            (Some((duties, step)), Waveform::Pulse(_)) if elapsed >= *step => {
                let index = (elapsed / step - 1).min(duties.len() as u64 - 1) as usize;
                Waveform::Pulse(duties[index])
            }
            (_, waveform) => waveform,
        }
    }
}

/// A note scheduled in samples.
#[derive(Clone, Copy, Debug)]
struct Event {
    on: u64,
    off: u64,
    cents: f64,
    instrument: usize,
    amplitude: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Debug)]
struct Voice {
    chip: ChipVoice,
    drum: DrumVoice,
    drumming: bool,
    oscillator: Oscillator,
    event: Option<Event>,
    held: bool,
    stage: Stage,
    position: u64,
    from: f64,
    level: f64,
    elapsed: u64,
    frequency: f64,
}
impl Voice {
    fn new(aliasing: Aliasing) -> Self {
        Self {
            chip: ChipVoice::new(aliasing),
            drum: DrumVoice::new(aliasing),
            drumming: false,
            oscillator: Oscillator::new(aliasing),
            event: None,
            held: false,
            stage: Stage::Idle,
            position: 0,
            from: 0.0,
            level: 0.0,
            elapsed: 0,
            frequency: 0.0,
        }
    }
    fn start(&mut self, event: Event, plans: &[Plan]) {
        self.event = Some(event);
        self.held = true;
        self.stage = Stage::Attack;
        self.position = 0;
        self.from = self.level;
        self.elapsed = 0;
        self.frequency = cents_to_hz(event.cents);
        if plans[event.instrument].chip.is_some() {
            self.chip.reset();
        }
        self.drumming = plans[event.instrument].drum.is_some();
        if let Some(drum) = &plans[event.instrument].drum {
            self.level = 0.0;
            self.drum
                .trigger(drum, crate::math::exp2((event.cents + 900.0) / 1200.0));
        }
    }
    fn release(&mut self, plans: &[Plan]) {
        self.held = false;
        if self.drumming {
            return;
        }
        if self.stage != Stage::Idle {
            let release = self.event.map_or(0, |e| plans[e.instrument].release);
            self.stage = if release == 0 {
                Stage::Idle
            } else {
                Stage::Release
            };
            self.position = 0;
            self.from = self.level;
        }
    }
    fn envelope(&mut self, plan: &Plan) -> f64 {
        let level = loop {
            let p = self.position as f64;
            match self.stage {
                Stage::Attack if self.position < plan.attack => {
                    break self.from + (1.0 - self.from) * p / plan.attack as f64;
                }
                Stage::Attack => (self.stage, self.position) = (Stage::Decay, 0),
                Stage::Decay if self.position < plan.decay => {
                    break 1.0 - (1.0 - plan.sustain) * p / plan.decay as f64;
                }
                Stage::Decay => self.stage = Stage::Sustain,
                Stage::Sustain => break plan.sustain,
                Stage::Release if self.position < plan.release => {
                    break self.from * (1.0 - p / plan.release as f64);
                }
                Stage::Release | Stage::Idle => break 0.0,
            }
        };
        self.position += 1;
        if self.stage == Stage::Release && self.position >= plan.release {
            self.stage = Stage::Idle;
            self.level = 0.0;
        } else {
            self.level = level;
        }
        level
    }
    fn sample(&mut self, plans: &[Plan], rate: u32) -> f64 {
        let Some(event) = self.event else {
            return 0.0;
        };
        let plan = &plans[event.instrument];
        if let Some(drum) = &plan.drum {
            return self.drum.sample(drum) * event.amplitude * plan.volume;
        }
        if self.stage == Stage::Idle {
            return 0.0;
        }
        let level = self.envelope(plan);
        let frequency = if plan.modulated() {
            cents_to_hz(plan.cents(event.cents, self.elapsed))
        } else {
            self.frequency
        };
        let waveform = plan.waveform(self.elapsed);
        if let Some(chip) = &plan.chip {
            let frequency = chip.frequency(frequency, self.elapsed);
            let (sample, finished) = self.chip.sample(chip, waveform, frequency, self.elapsed);
            let gain = chip.gain(level * event.amplitude * plan.volume, self.elapsed);
            self.elapsed += 1;
            if finished {
                self.stage = Stage::Idle;
                self.level = 0.0;
            }
            return sample * gain;
        }
        self.elapsed += 1;
        self.oscillator.next(waveform, frequency, rate) * level * event.amplitude * plan.volume
    }
    fn active(&self) -> bool {
        if self.drumming {
            self.drum.active()
        } else {
            self.stage != Stage::Idle
        }
    }
}

#[derive(Clone, Debug)]
struct Track {
    events: Vec<Event>,
    next: usize,
    /// First event at or after the loop start, and an event sounding across it.
    loop_next: usize,
    loop_carry: Option<Event>,
    voice: Voice,
}

/// Incremental, allocation-free rendering of a [`Score`] into mono samples.
///
/// All memory is allocated by [`Synth::new`]. [`Synth::fill`] allocates nothing,
/// takes no lock and does not block, so it can run on an audio thread. The samples
/// do not depend on how the output is split into chunks, and are identical on every
/// platform: pitch and vibrato use the crate's own polynomials, not `sin` or `exp`.
///
/// At the song's end it continues from the loop point while loops remain. A note
/// that sounds across the loop point restarts there on every repeat. After the last
/// pass it renders the release of the sounding notes, then finishes.
#[derive(Clone, Debug)]
pub struct Synth {
    rate: u32,
    aliasing: Aliasing,
    gain: f64,
    plans: Vec<Plan>,
    tracks: Vec<Track>,
    song_end: u64,
    loop_start: u64,
    loops: Option<u32>,
    loops_left: Option<u32>,
    position: u64,
    ending: bool,
    finished: bool,
    produced: u64,
    max_release: u64,
}
impl Synth {
    pub fn new(score: &Score, sample_rate: u32, options: &RenderOptions) -> Result<Self> {
        Limits::default().check(0, 1, sample_rate)?;
        if !options.gain.is_finite() {
            return Err(Error::invalid("Gain must be finite"));
        }
        let to_samples = |tick: u64| {
            (tick as u128 * sample_rate as u128 * 60
                / (score.tempo() as u128 * score.ticks_per_quarter() as u128)) as u64
        };
        let plans: Vec<Plan> = score
            .instruments()
            .iter()
            .map(|(_, i)| Plan::new(i, sample_rate, score.tempo(), score.ticks_per_quarter()))
            .collect();
        let song_end = to_samples(score.length_ticks());
        let loop_start = to_samples(score.loop_tick().unwrap_or(0));
        if options.loops.is_none() && loop_start >= song_end {
            return Err(Error::invalid(
                "Cannot loop forever over an empty loop section",
            ));
        }
        let tracks = score
            .channels()
            .iter()
            .map(|channel| {
                let events: Vec<Event> = channel
                    .notes()
                    .iter()
                    .map(|n| {
                        let on = to_samples(n.start);
                        let end = to_samples(n.start + n.length);
                        Event {
                            on,
                            off: on + (end - on) * n.gate as u64 / 8,
                            cents: (n.pitch.midi() as f64 - 69.0) * 100.0,
                            instrument: n.instrument,
                            amplitude: if n.velocity == 127 {
                                n.volume as f64 / 15.0
                            } else {
                                (n.volume as f64 / 15.0) * (n.velocity as f64 / 127.0)
                            },
                        }
                    })
                    .collect();
                let loop_next = events.partition_point(|e| e.on < loop_start);
                let loop_carry = loop_next
                    .checked_sub(1)
                    .map(|i| events[i])
                    .filter(|e| e.off > loop_start && plans[e.instrument].drum.is_none())
                    .map(|e| Event {
                        on: loop_start,
                        ..e
                    });
                Track {
                    events,
                    next: 0,
                    loop_next,
                    loop_carry,
                    voice: Voice::new(options.aliasing),
                }
            })
            .collect();
        Ok(Self {
            rate: sample_rate,
            aliasing: options.aliasing,
            gain: options.gain as f64,
            max_release: plans
                .iter()
                .map(|p| p.release.max(p.drum.as_ref().map_or(0, |d| d.frames)))
                .max()
                .unwrap_or(0),
            plans,
            tracks,
            song_end,
            loop_start,
            loops: options.loops,
            loops_left: options.loops,
            position: 0,
            ending: false,
            finished: false,
            produced: 0,
        })
    }
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
    /// Always 1: the synthesizer renders mono.
    pub fn channels(&self) -> usize {
        1
    }
    pub fn is_finished(&self) -> bool {
        self.finished
    }
    /// Frames rendered since the start or the last [`Synth::reset`].
    pub fn frames_rendered(&self) -> u64 {
        self.produced
    }
    /// An upper bound on the frames this synthesizer renders, or `None` if it loops forever.
    pub fn max_frames(&self) -> Option<u64> {
        let body = self.song_end.saturating_sub(self.loop_start);
        self.loops.map(|n| {
            (n as u64)
                .saturating_mul(body)
                .saturating_add(self.song_end + self.max_release + 1)
        })
    }
    /// Starts again from the beginning, as if newly created.
    pub fn reset(&mut self) {
        for track in &mut self.tracks {
            track.next = 0;
            track.voice = Voice::new(self.aliasing);
        }
        self.loops_left = self.loops;
        self.position = 0;
        self.ending = false;
        self.finished = false;
        self.produced = 0;
    }
    /// Fills `out` with mono samples and returns how many were written. Fewer than
    /// `out.len()` are written only when the song finishes; the rest is left untouched.
    pub fn fill(&mut self, out: &mut [f32]) -> usize {
        for (written, slot) in out.iter_mut().enumerate() {
            if self.finished {
                return written;
            }
            if !self.ending {
                self.schedule();
            }
            if self.ending && self.tracks.iter().all(|t| !t.voice.active()) {
                self.finished = true;
                return written;
            }
            let mut sum = 0.0;
            for track in &mut self.tracks {
                sum += track.voice.sample(&self.plans, self.rate);
            }
            *slot = (sum * self.gain) as f32;
            self.position += 1;
            self.produced += 1;
        }
        out.len()
    }
    fn schedule(&mut self) {
        if self.position >= self.song_end {
            for track in &mut self.tracks {
                if track.voice.held {
                    track.voice.release(&self.plans);
                }
            }
            let can_loop = self.loop_start < self.song_end && self.loops_left != Some(0);
            if !can_loop {
                self.ending = true;
                return;
            }
            if let Some(n) = &mut self.loops_left {
                *n -= 1;
            }
            self.position = self.loop_start;
            for track in &mut self.tracks {
                track.next = track.loop_next;
                if let Some(event) = track.loop_carry {
                    track.voice.start(event, &self.plans);
                }
            }
        }
        let position = self.position;
        for track in &mut self.tracks {
            while let Some(&event) = track.events.get(track.next) {
                if event.on > position {
                    break;
                }
                track.voice.start(event, &self.plans);
                track.next += 1;
            }
            let voice = &mut track.voice;
            if voice.held && voice.event.is_some_and(|e| e.off <= position) {
                voice.release(&self.plans);
            }
        }
    }
}
