use super::chip::{ChipPlan, ChipVoice};
use super::drum::{DrumPlan, DrumVoice};
use super::score::{Instrument, Score};
use super::synthwave::{WidePlan, WideVoice};
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
        self.render_channels(sample_rate, options, false)
    }
    /// Renders stereo, preserving synthwave unison spread and pan. Legacy channels
    /// are duplicated to left/right. No automatic limiting is applied.
    pub fn render_stereo(&self, sample_rate: u32) -> Result<Audio> {
        self.render_stereo_with(sample_rate, &RenderOptions::default())
    }
    pub fn render_stereo_with(&self, sample_rate: u32, options: &RenderOptions) -> Result<Audio> {
        self.render_channels(sample_rate, options, true)
    }
    fn render_channels(
        &self,
        sample_rate: u32,
        options: &RenderOptions,
        stereo: bool,
    ) -> Result<Audio> {
        let mut synth = if stereo {
            Synth::new_stereo(self, sample_rate, options)?
        } else {
            Synth::new(self, sample_rate, options)?
        };
        let frames = synth
            .max_frames()
            .ok_or_else(|| Error::invalid("Cannot render an endless loop offline"))?;
        let frames = usize::try_from(frames).map_err(|_| Error::limit("Song is too long"))?;
        let channels = synth.channels();
        Limits::default().check(frames, channels, sample_rate)?;
        let mut samples = vec![0.0; frames * channels];
        let written = synth.fill(&mut samples);
        samples.truncate(written * channels);
        if channels == 1 {
            Audio::new(vec![samples], sample_rate)
        } else {
            Audio::new(
                (0..channels)
                    .map(|c| samples.iter().skip(c).step_by(channels).copied().collect())
                    .collect(),
                sample_rate,
            )
        }
    }
}

#[derive(Clone, Debug)]
struct Plan {
    wide: Option<WidePlan>,
    /// Size of the track's voice pool for this instrument's notes: synthwave voices,
    /// or the opt-in voices of a wavetable source. Zero uses the single legacy voice.
    voices: usize,
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
            // A wavetable source with synthwave `voices` keeps its chip rendering and
            // only borrows the voice pool; validation admits no other combination.
            wide: instrument
                .synthwave
                .filter(|_| instrument.chip.is_none())
                .map(|s| WidePlan::new(s, rate, tempo)),
            voices: instrument.synthwave.map_or(0, |s| s.voices as usize),
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
    wide: WideVoice,
    started: u64,
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
            wide: WideVoice::new(),
            started: 0,
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
        let active = self.active();
        let legato = plans[event.instrument]
            .wide
            .as_ref()
            .is_some_and(|p| p.settings.legato)
            && active
            && self.held
            && self
                .event
                .is_some_and(|old| old.instrument == event.instrument && old.off == event.on);
        // A pooled voice that is free starts from silence, even if a release without
        // a tail left its last level behind.
        if plans[event.instrument].voices > 0 && !active {
            self.level = 0.0;
        }
        if let Some(plan) = &plans[event.instrument].wide {
            self.wide.trigger(plan, event.cents, active, legato);
        }
        self.event = Some(event);
        self.held = true;
        if legato {
            return;
        }
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
        self.wide.release();
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
    fn sample_stereo(&mut self, plans: &[Plan], rate: u32, clock: u64) -> [f64; 2] {
        let Some(event) = self.event else {
            return [0.0; 2];
        };
        let plan = &plans[event.instrument];
        let Some(wide) = &plan.wide else {
            let sample = self.sample(plans, rate);
            return [sample; 2];
        };
        if self.stage == Stage::Idle {
            return [0.0; 2];
        }
        let gain = self.envelope(plan) * event.amplitude * plan.volume;
        let offset = plan.cents(0.0, self.elapsed);
        let mut result = self
            .wide
            .sample(wide, plan.waveform(self.elapsed), offset, clock);
        self.elapsed += 1;
        for value in &mut result {
            *value *= gain;
        }
        result
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
    poly: Vec<Voice>,
    poly_carry: Vec<Event>,
}
impl Track {
    fn trigger(&mut self, event: Event, plans: &[Plan], clock: u64) {
        let limit = plans[event.instrument].voices;
        if limit > 0 {
            if self.voice.held {
                self.voice.release(plans);
            }
            for voice in self.poly.iter_mut().skip(limit) {
                voice.stage = Stage::Idle;
                voice.held = false;
                voice.level = 0.0;
            }
            let voices = &mut self.poly[..limit];
            let slot = voices.iter().position(|v| !v.active()).unwrap_or_else(|| {
                voices
                    .iter()
                    .enumerate()
                    .min_by_key(|(i, v)| (v.held, v.started, *i))
                    .unwrap()
                    .0
            });
            voices[slot].start(event, plans);
            voices[slot].started = clock;
        } else {
            for voice in &mut self.poly {
                if voice.held {
                    voice.release(plans);
                }
            }
            self.voice.start(event, plans);
        }
    }
    fn release(&mut self, plans: &[Plan]) {
        if self.voice.held {
            self.voice.release(plans);
        }
        for voice in &mut self.poly {
            if voice.held {
                voice.release(plans);
            }
        }
    }
    fn active(&self) -> bool {
        self.voice.active() || self.poly.iter().any(Voice::active)
    }
}

/// Incremental, allocation-free rendering of a [`Score`] into mono or stereo frames.
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
    channels: usize,
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
        Self::with_channels(score, sample_rate, options, 1)
    }
    /// Builds a stereo renderer, with interleaved output through `fill` and the
    /// mixer's `Stream` adapter. Preparation may allocate; fill/reset do not.
    pub fn new_stereo(score: &Score, sample_rate: u32, options: &RenderOptions) -> Result<Self> {
        Self::with_channels(score, sample_rate, options, 2)
    }
    fn with_channels(
        score: &Score,
        sample_rate: u32,
        options: &RenderOptions,
        channels: usize,
    ) -> Result<Self> {
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
                    .filter(|e| {
                        e.off > loop_start
                            && plans[e.instrument].drum.is_none()
                            && plans[e.instrument].voices == 0
                    })
                    .map(|e| Event {
                        on: loop_start,
                        ..e
                    });
                let poly_count = events
                    .iter()
                    .map(|e| plans[e.instrument].voices)
                    .max()
                    .unwrap_or(0);
                let poly_carry = events
                    .iter()
                    .filter(|e| {
                        e.on < loop_start && e.off > loop_start && plans[e.instrument].voices > 0
                    })
                    .map(|e| Event {
                        on: loop_start,
                        ..*e
                    })
                    .collect();
                Track {
                    events,
                    next: 0,
                    loop_next,
                    loop_carry,
                    voice: Voice::new(options.aliasing),
                    poly: (0..poly_count)
                        .map(|_| Voice::new(options.aliasing))
                        .collect(),
                    poly_carry,
                }
            })
            .collect();
        Ok(Self {
            rate: sample_rate,
            channels,
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
    /// 1 for `new`, 2 for `new_stereo`.
    pub fn channels(&self) -> usize {
        self.channels
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
            for voice in &mut track.poly {
                *voice = Voice::new(self.aliasing);
            }
        }
        self.loops_left = self.loops;
        self.position = 0;
        self.ending = false;
        self.finished = false;
        self.produced = 0;
    }
    /// Fills whole interleaved frames and returns their count. Mono uses one sample
    /// per frame, stereo two. Unused samples (including an incomplete frame) are untouched.
    pub fn fill(&mut self, out: &mut [f32]) -> usize {
        let frames = out.len() / self.channels;
        for (written, slot) in out.chunks_exact_mut(self.channels).enumerate() {
            if self.finished {
                return written;
            }
            if !self.ending {
                self.schedule();
            }
            if self.ending && self.tracks.iter().all(|t| !t.active()) {
                self.finished = true;
                return written;
            }
            let mut sum = 0.0;
            let mut right = 0.0;
            for track in &mut self.tracks {
                let legacy = track.voice.sample(&self.plans, self.rate);
                sum += legacy;
                right += legacy;
                for voice in &mut track.poly {
                    let [l, r] = voice.sample_stereo(&self.plans, self.rate, self.produced);
                    if self.channels == 1 {
                        sum += (l + r) * 0.5;
                    } else {
                        sum += l;
                        right += r;
                    }
                }
            }
            slot[0] = (sum * self.gain) as f32;
            if self.channels == 2 {
                slot[1] = (right * self.gain) as f32;
            }
            self.position += 1;
            self.produced += 1;
        }
        frames
    }
    fn schedule(&mut self) {
        if self.position >= self.song_end {
            for track in &mut self.tracks {
                track.release(&self.plans);
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
                // Carries are precompiled; no clone/temporary allocation on audio.
                for i in 0..track.poly_carry.len() {
                    let event = track.poly_carry[i];
                    track.trigger(event, &self.plans, self.produced);
                }
            }
        }
        let position = self.position;
        for track in &mut self.tracks {
            while let Some(&event) = track.events.get(track.next) {
                if event.on > position {
                    break;
                }
                track.trigger(event, &self.plans, self.produced);
                track.next += 1;
            }
            let voice = &mut track.voice;
            if voice.held && voice.event.is_some_and(|e| e.off <= position) {
                voice.release(&self.plans);
            }
            for voice in &mut track.poly {
                if voice.held && voice.event.is_some_and(|e| e.off <= position) {
                    voice.release(&self.plans);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legato_keeps_envelopes_but_short_gates_and_gaps_retrigger() {
        for gate in [4, 8] {
            let score=Score::parse(&format!("tempo 600\ninstrument p voices=1 legato=on glide=50 adsr=200,0,100,20 filter=lowpass cutoff=1000 filter-adsr=200,0,100,20\nchannel a=@p q{gate} a4 b4 r4 c4")).unwrap();
            let mut synth = Synth::new(&score, 1000, &Default::default()).unwrap();
            synth.fill(&mut [0.0; 126]);
            let voice = &synth.tracks[0].poly[0];
            assert_eq!(voice.position, if gate == 8 { 126 } else { 26 });
            synth.fill(&mut [0.0; 175]);
            let voice = &synth.tracks[0].poly[0];
            assert_eq!(voice.position, 1);
            assert!(voice.held);
        }
    }
    #[test]
    fn stealing_reuses_oldest_release_and_its_new_note_off() {
        let score = Score::parse(
            "tempo 600\ninstrument p voices=2 adsr=0,0,100,500\nchannel a=@p a4 b4 c4 d4",
        )
        .unwrap();
        let mut synth = Synth::new(&score, 1000, &Default::default()).unwrap();
        synth.fill(&mut [0.0; 201]);
        let voices = &synth.tracks[0].poly;
        assert_eq!(voices.len(), 2);
        assert_eq!(voices[0].event.unwrap().cents, -900.0); // C4 replaced A4's release.
        assert!(voices[0].held);
        assert!(!voices[1].held);
        synth.fill(&mut [0.0; 50]);
        assert!(synth.tracks[0].poly[0].held);
    }
    #[test]
    fn reducing_and_restoring_a_voice_limit_restarts_retired_slots_from_silence() {
        let score=Score::parse("tempo 600\ninstrument wide voices=3 adsr=100,0,100,0\ninstrument narrow voices=1 adsr=100,0,100,0\nchannel a=@wide {c e g}4 @narrow c4 @wide {c e g}4").unwrap();
        let mut synth = Synth::new(&score, 1000, &Default::default()).unwrap();
        synth.fill(&mut [0.0; 201]);
        for v in &synth.tracks[0].poly[1..] {
            assert_eq!(v.from, 0.0);
            assert_eq!(v.level, 0.0);
            assert_eq!(v.position, 1);
        }
    }
}
