//! Named sound events, parsed and prepared on the control thread (`events` feature).
//!
//! One definition per line: `event footstep sources=left,right choice=shuffle
//! gain=0.8,1 pitch=-1,1 max-instances=2 min-gap=0.08 bus=effects`.
//! Names are case-sensitive ASCII identifiers (letters, digits, `_`, `-`, `.`).
//! `#` begins a comment. See [`EventBank::parse`] for the complete grammar.
//!
//! Register decoded sounds in [`Sources`]; optional helpers render SFX and scores
//! once, before playback. [`EventBank::prepare`] resolves every source and bus and
//! checks every loop region. No parsing, file access or synthesis runs on audio.
//! A prepared bank belongs to one mixer. Its players share per-event instance
//! limits, including across variants, but have independent seeded selection state.
//!
//! Selection/gain/pitch are deterministic per event name and seed, independent of
//! other events and the mixer's RNG. Errors returned by `play` do not consume that
//! state. A successfully queued request consumes it even if audio later reports
//! [`super::MixerEvent::Limited`] or [`super::MixerEvent::Rejected`]. Voice IDs and
//! admission depend on other mixer activity. Poll mixer events normally to reclaim
//! finished voices and distinguish submission from application. All event methods
//! are control-thread operations and may allocate.
//!
//! ```
//! use audiobox::events::{EventBank, Sources};
//! use audiobox::mixer::{Mixer, MixerConfig};
//! use audiobox::Audio;
//! let (mut mixer, mut handle) = Mixer::new(MixerConfig::default())?;
//! let mut sources = Sources::default();
//! sources.insert("click", Audio::new(vec![vec![0.1; 100]], 48_000)?)?;
//! let bank = EventBank::parse("event ui.click sources=click gain=0.5,0.8")?;
//! let mut player = bank.prepare(&handle, &sources)?.player(42);
//! let started = player.play(&mut handle, "ui.click")?;
//! # mixer.render(&mut [0.0; 256]);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
mod parse;
use super::{BusId, Emitter, Loop, MixerHandle, Play, VoiceId};
use crate::{Audio, Error, Result};
pub use parse::ParseError;
use std::collections::BTreeMap;
use std::sync::{Arc, atomic::AtomicU64};

/// Source registry. Insertion and optional synthesis happen on the control thread.
/// Prepared banks retain `Arc`s, so the registry may be dropped after preparation.
#[derive(Default)]
pub struct Sources {
    sounds: BTreeMap<String, Arc<Audio>>,
}
impl Sources {
    /// Registers mono/stereo audio without copying its samples. Duplicate or invalid
    /// names are errors, leaving the previous entry intact.
    pub fn insert(&mut self, name: &str, audio: impl Into<Arc<Audio>>) -> Result<()> {
        if !parse::name_ok(name) || self.sounds.contains_key(name) {
            return Err(Error::invalid("Invalid or duplicate source name"));
        }
        let audio = audio.into();
        if !(1..=2).contains(&audio.channels()) {
            return Err(Error::invalid("Event sources must be mono or stereo"));
        }
        self.sounds.insert(name.to_owned(), audio);
        Ok(())
    }
    /// Renders and caches an effect once; requires `sfx`.
    #[cfg(feature = "sfx")]
    pub fn insert_effect(
        &mut self,
        name: &str,
        effect: &crate::sfx::Effect,
        rate: u32,
    ) -> Result<()> {
        self.insert(name, effect.render(rate)?)
    }
    /// Renders and caches a finite score once; requires `synth`.
    #[cfg(feature = "synth")]
    pub fn insert_score(
        &mut self,
        name: &str,
        score: &crate::synth::Score,
        rate: u32,
        options: &crate::synth::RenderOptions,
    ) -> Result<()> {
        self.insert(name, score.render_with(rate, options)?)
    }
}

#[derive(Clone, Copy)]
enum Choice {
    Random,
    Shuffle,
    Sequence,
}
#[derive(Clone)]
struct Definition {
    name: String,
    sources: Vec<String>,
    choice: Choice,
    gain: [f32; 2],
    pitch: [f32; 2],
    bus: String,
    play: Play,
    source_location: parse::Location,
    bus_location: parse::Location,
    loop_location: parse::Location,
}

/// Parsed text bank, independent of a mixer or source registry.
#[derive(Clone)]
pub struct EventBank {
    definitions: Vec<Definition>,
}
impl EventBank {
    /// Parses UTF-8 text (at most 1 MiB, 1024 events, 64 source entries per event).
    /// Blank/comment lines are allowed; an empty bank is valid. Every other line is
    /// `event NAME sources=A,B [key=value ...]`. No whitespace inside values.
    /// Identifiers contain 1..=64 ASCII letters/digits/`_`/`-`/`.`.
    /// Unknown/repeated keys and duplicate event names are errors.
    ///
    /// | Key | Values / default |
    /// | --- | --- |
    /// | `sources` | Required comma-separated registered names; duplicates weight entries |
    /// | `choice` | `random` (default), `shuffle`, `sequence` |
    /// | `gain` | Linear `min,max` or fixed value, finite >=0; default 1 |
    /// | `pitch` | Semitones `min,max` or fixed, -48..=48; default 0 |
    /// | `bus` | Named mixer bus or `master` (default) |
    /// | `priority` | 0..=255; default 128 |
    /// | `max-instances` | Concurrent voices across all variants; 0 unlimited (default) |
    /// | `min-gap` | Minimum seconds between admitted starts, 0..=60; default 0 |
    /// | `policy` | `refuse` (default), `oldest`, `quietest` |
    /// | `loop` | `off` (default), `whole`, or `start,end` source frames |
    /// | `pan` | -1..=1, default 0; replaced by spatial placement |
    /// | `rate` | 1/16..=8; default 1 |
    /// | `fade-in` | Linear fade seconds, 0..=60; default 0 |
    /// | `position` | `x,y` relative 2D position using the mixer's `Space2d` |
    /// | `emitter` | `x,y,z` world position, mutually exclusive with `position` |
    /// | `velocity` | `x,y,z` (default zero), requires `emitter` |
    /// | `attenuation` | `model,min,max,rolloff`; model is `linear`, `inverse` or `exponential`; requires `emitter`; mixer defaults |
    ///
    /// Sequence wraps in text order. Shuffle draws every entry once per cycle and
    /// avoids the previous entry at cycle boundaries when there are >=2 entries.
    /// Random samples with replacement. Distinct entries may name the same sound.
    pub fn parse(text: &str) -> std::result::Result<Self, ParseError> {
        parse::bank(text)
    }
    pub fn len(&self) -> usize {
        self.definitions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
    /// Event names in definition order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.definitions.iter().map(|d| d.name.as_str())
    }
    /// Resolves all references and validates loop regions against every candidate.
    /// Resolution errors retain the line/column of the corresponding text token.
    /// Preparing again creates independent instance groups; clone the resulting bank
    /// or create multiple players from it to share limits.
    pub fn prepare(
        &self,
        handle: &MixerHandle,
        sources: &Sources,
    ) -> std::result::Result<PreparedBank, ParseError> {
        let mut events = BTreeMap::new();
        for d in &self.definitions {
            let mut play = d.play;
            play.bus = if d.bus == "master" {
                BusId::MASTER
            } else {
                handle
                    .bus(&d.bus)
                    .ok_or_else(|| d.bus_location.invalid("Unknown event bus"))?
            };
            let mut sounds = Vec::new();
            for name in &d.sources {
                let audio = sources.sounds.get(name).ok_or_else(|| {
                    d.source_location
                        .invalid(format!("Unknown event source: {name}"))
                })?;
                if let Loop::Region { end, .. } = play.looping {
                    if end > audio.frames() as u64 {
                        return Err(d
                            .loop_location
                            .invalid(format!("Loop exceeds source: {name}")));
                    }
                }
                sounds.push(audio.clone());
            }
            if let Some(limit) = &mut play.instances {
                if limit.max == 0 && limit.min_gap == 0.0 {
                    play.instances = None;
                } else {
                    limit.key = crate::identity::next();
                }
            }
            events.insert(
                d.name.clone(),
                PreparedEvent {
                    sounds,
                    choice: d.choice,
                    gain: d.gain,
                    pitch: d.pitch,
                    play,
                },
            );
        }
        Ok(PreparedBank {
            data: Arc::new(Prepared {
                clock: handle.clock.clone(),
                events,
            }),
        })
    }
}
struct PreparedEvent {
    sounds: Vec<Arc<Audio>>,
    choice: Choice,
    gain: [f32; 2],
    pitch: [f32; 2],
    play: Play,
}
struct Prepared {
    clock: Arc<AtomicU64>,
    events: BTreeMap<String, PreparedEvent>,
}
/// Resolved, immutable, mixer-bound events. Clones share sources and instance keys.
#[derive(Clone)]
pub struct PreparedBank {
    data: Arc<Prepared>,
}
impl PreparedBank {
    /// Creates independent selection state. Each event is seeded from its name and
    /// `seed`; adding/reordering other events does not change its choices.
    pub fn player(&self, seed: u64) -> EventPlayer {
        EventPlayer {
            bank: self.clone(),
            states: self
                .data
                .events
                .keys()
                .map(|name| {
                    let hash = name.bytes().fold(0xcbf29ce484222325u64, |h, b| {
                        (h ^ b as u64).wrapping_mul(0x100000001b3)
                    });
                    (
                        name.clone(),
                        State {
                            random: seed ^ hash,
                            cursor: 0,
                            bag: Vec::new(),
                            last: None,
                        },
                    )
                })
                .collect(),
        }
    }
}
/// Optional per-request placement, replacing the event's spatial defaults.
#[derive(Clone, Copy, Debug)]
pub enum Placement {
    /// Removes spatial placement; uses the event's pan.
    Pan,
    Position([f32; 2]),
    Emitter(Emitter),
}
/// Per-request scheduling and spatial override.
#[derive(Clone, Copy, Debug, Default)]
pub struct Trigger {
    /// Absolute output frame. `None` applies at the next render, in submission
    /// order with other immediate mixer commands.
    pub frame: Option<u64>,
    pub placement: Option<Placement>,
}
/// Successfully queued request, with selected parameters for replay/debugging.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triggered {
    pub voice: VoiceId,
    /// Zero-based index in the event's `sources` list.
    pub source: usize,
    pub gain: f32,
    pub pitch: f32,
}
/// Seeded event selection state. Use returned voice IDs with normal mixer commands.
pub struct EventPlayer {
    bank: PreparedBank,
    states: BTreeMap<String, State>,
}
impl EventPlayer {
    pub fn play(&mut self, handle: &mut MixerHandle, name: &str) -> Result<Triggered> {
        self.play_with(handle, name, Trigger::default())
    }
    /// Queues a named event. Immediate errors (including queue full or wrong mixer)
    /// preserve all event selection state. Later audio-side refusal consumes it.
    pub fn play_with(
        &mut self,
        handle: &mut MixerHandle,
        name: &str,
        trigger: Trigger,
    ) -> Result<Triggered> {
        if !Arc::ptr_eq(&self.bank.data.clock, &handle.clock) {
            return Err(Error::invalid("Event bank belongs to another mixer"));
        }
        let event = self
            .bank
            .data
            .events
            .get(name)
            .ok_or_else(|| Error::invalid("Unknown sound event"))?;
        let state = self.states.get_mut(name).expect("prepared event state");
        let mut next = state.clone();
        let source = next.choose(event.choice, event.sounds.len());
        let mut play = event.play;
        play.gain = next.range(event.gain);
        play.pitch = next.range(event.pitch);
        if let Some(placement) = trigger.placement {
            play.position = None;
            play.emitter = None;
            match placement {
                Placement::Pan => {}
                Placement::Position(p) => play.position = Some(p),
                Placement::Emitter(e) => play.emitter = Some(e),
            }
        }
        let sound = event.sounds[source].clone();
        let voice = match trigger.frame {
            Some(frame) => handle.at(frame).play(sound, &play)?,
            None => handle.play(sound, &play)?,
        };
        *state = next;
        Ok(Triggered {
            voice,
            source,
            gain: play.gain,
            pitch: play.pitch,
        })
    }
}
#[derive(Clone)]
struct State {
    random: u64,
    cursor: usize,
    bag: Vec<usize>,
    last: Option<usize>,
}
impl State {
    fn next(&mut self) -> u64 {
        self.random = self.random.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.random;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    fn index(&mut self, n: usize) -> usize {
        let n = n as u64;
        let threshold = n.wrapping_neg() % n;
        loop {
            let r = self.next();
            if r >= threshold {
                return (r % n) as usize;
            }
        }
    }
    fn range(&mut self, [min, max]: [f32; 2]) -> f32 {
        if min == max {
            return min;
        }
        let unit = (self.next() >> 40) as f64 / 16_777_216.0;
        (min as f64 + (max as f64 - min as f64) * unit) as f32
    }
    fn choose(&mut self, choice: Choice, n: usize) -> usize {
        match choice {
            Choice::Random => self.index(n),
            Choice::Sequence => {
                let result = self.cursor;
                self.cursor = (self.cursor + 1) % n;
                result
            }
            Choice::Shuffle => {
                if self.cursor == self.bag.len() {
                    self.bag = (0..n).collect();
                    for i in (1..n).rev() {
                        let j = self.index(i + 1);
                        self.bag.swap(i, j);
                    }
                    if n > 1 && self.last == Some(self.bag[0]) {
                        let j = 1 + self.index(n - 1);
                        self.bag.swap(0, j);
                    }
                    self.cursor = 0;
                }
                let result = self.bag[self.cursor];
                self.cursor += 1;
                self.last = Some(result);
                result
            }
        }
    }
}
