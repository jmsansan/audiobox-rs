//! A real-time mixer for games. Requires the `mixer` feature.
//!
//! [`Mixer::new`] returns two halves. The [`Mixer`] goes to the audio callback, which
//! calls [`Mixer::render`] for every buffer. The [`MixerHandle`] stays with the game,
//! which starts voices, changes gains and pans, and stops them.
//!
//! ```
//! use audiobox::mixer::{Fade, Loop, Mixer, MixerConfig, Play};
//! use audiobox::Audio;
//! use std::sync::Arc;
//!
//! let (mut mixer, mut handle) = Mixer::new(MixerConfig {
//!     buses: vec!["music".into(), "effects".into()],
//!     ..MixerConfig::default()
//! })?;
//! let music = handle.bus("music").unwrap();
//! let effects = handle.bus("effects").unwrap();
//!
//! let beep = Arc::new(Audio::new(vec![vec![0.25; 4800]], 48_000)?);
//! handle.play(beep.clone(), &Play { bus: effects, pan: -0.5, ..Play::default() })?;
//! handle.play(beep, &Play { bus: music, looping: Loop::Whole, fade_in: Fade::linear(0.5), ..Play::default() })?;
//! handle.set_bus_gain(music, 0.5, Fade::linear(1.0))?;
//!
//! // In the audio callback: interleaved stereo at 48 kHz.
//! let mut buffer = [0.0f32; 2 * 512];
//! mixer.render(&mut buffer);
//! # Ok::<(), audiobox::Error>(())
//! ```
//!
//! # Sounds
//!
//! A voice plays a [`Sound`]: decoded [`Audio`] shared through an [`Arc`], so that
//! many voices play one sound without copying it, or a [`Stream`] that produces
//! frames as it plays, such as a [`Decoder`](crate::stream::Decoder) of compressed
//! music or a [`Synth`](crate::synth::Synth) rendering a score. Sounds are mono or stereo.
//!
//! Each voice has a gain, a pan, a bus, a priority, an optional fade-in, and a loop:
//! off, the whole sound, or a region between two frames. Gain, pan and bus gain
//! changes, and stops, can fade over a duration with a [`FadeCurve`]. A loop of a
//! stream seeks it back, so a stream that loops must support seeking to the loop start.
//!
//! # Voices and buses
//!
//! At most [`MixerConfig::max_voices`] voices play at once. When a voice starts while
//! that many play, it takes the place of the playing voice with the lowest priority,
//! the oldest among equals, if that priority is not higher than its own. The voice it
//! replaces fades out over 5 ms and reports [`MixerEvent::Stolen`]. Otherwise the new
//! voice does not play and reports [`MixerEvent::Rejected`]. Give music a high
//! priority so that effects never take its place.
//!
//! Every voice plays through a bus: the master bus, or one of the named buses of
//! [`MixerConfig::buses`], which play through the master. A voice's samples are scaled
//! by its gain, its fades, its pan, its bus's gain and the master gain.
//!
//! # Rendering
//!
//! [`Mixer::render`] fills an interleaved buffer of the output's channels (1 or 2) at
//! the output's sample rate. It writes `out.len() / channels` whole frames and sets any
//! remaining samples to 0; a buffer shorter than a frame renders nothing, but still
//! applies the changes that are due.
//!
//! Sources at another sample rate are resampled with a 16-tap windowed-sinc filter
//! (Blackman window, 256 interpolated phases, cutoff at 0.9 of the lower Nyquist
//! frequency). It is designed for ratios up to 2:1, such as 44.1 kHz to 48 kHz;
//! larger downsampling ratios alias more. A source at the output rate is copied
//! sample for sample.
//!
//! Panning a mono source follows a constant-power law: `cos θ` on the left and
//! `sin θ` on the right, with `θ = (pan + 1)·π/4`, so the centre is -3 dB on each side.
//! A stereo source is balanced: the centre leaves it unchanged and the far side
//! follows `√2·cos θ` or `√2·sin θ`, limited to 1. A mono output sums mono sources and
//! averages the two channels of stereo ones.
//!
//! # Real-time safety
//!
//! [`Mixer::render`] allocates nothing, takes no lock and does not block. Changes reach
//! it through a bounded queue, a [`std::sync::mpsc::sync_channel`], which keeps its
//! messages in a buffer allocated when the mixer is created; the mixer takes them with
//! `try_recv` at the start of each render, and the handle sends with `try_send`, so
//! neither side ever waits. Voices and their buffers are built by the handle, on the
//! game's thread. Finished voices go back to the handle through a second bounded queue
//! and are freed by [`MixerHandle::poll_event`], so the audio thread frees no memory
//! either. Poll events regularly, such as once per game frame.
//!
//! # Timing and determinism
//!
//! Every change applies at an output frame. [`MixerHandle::at`] schedules changes at an
//! exact frame, counted from the first rendered frame; they apply there whatever the
//! buffer sizes, splitting a buffer if needed. Changes sent without a frame apply at the
//! start of the next render. The same changes at the same frames give the same samples
//! on every platform: resampling, panning and fades use only basic IEEE arithmetic and
//! the crate's own sine series.
mod voice;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};

use voice::{Feed, ONE, Ramp, SincTable, State, Voice};

use crate::{Audio, Error, ErrorCode, FadeCurve, Limits, Result};

/// Most frames rendered between changes of the voices' gains.
const BLOCK: usize = 256;
/// Extra voice slots for voices that fade out after a stop or a steal.
const TAILS: usize = 8;
const STEAL_FADE: f64 = 0.005;

/// A source of interleaved frames that a voice plays as it goes.
///
/// [`read`](Stream::read) runs on the audio thread, so it must not allocate or block.
pub trait Stream: Send {
    fn sample_rate(&self) -> u32;
    /// 1 or 2.
    fn channels(&self) -> usize;
    /// The stream's length in frames, if known.
    fn frames(&self) -> Option<u64> {
        None
    }
    /// Fills `out` with interleaved frames and returns how many; 0 at the end.
    fn read(&mut self, out: &mut [f32]) -> Result<usize>;
    /// Moves to `frame`, for loops.
    fn seek(&mut self, frame: u64) -> Result<()>;
}

impl<B: AsRef<[u8]> + Send + 'static> Stream for crate::stream::Decoder<B> {
    fn sample_rate(&self) -> u32 {
        crate::stream::Decoder::sample_rate(self)
    }
    fn channels(&self) -> usize {
        crate::stream::Decoder::channels(self)
    }
    fn frames(&self) -> Option<u64> {
        crate::stream::Decoder::frames(self)
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        crate::stream::Decoder::read(self, out)
    }
    fn seek(&mut self, frame: u64) -> Result<()> {
        crate::stream::Decoder::seek(self, frame)
    }
}

/// A synthesizer plays as a mono stream. It loops by itself when its
/// [`RenderOptions::loops`](crate::synth::RenderOptions::loops) is `None`; as a voice
/// loop it can only return to its start.
#[cfg(feature = "synth")]
impl Stream for crate::synth::Synth {
    fn sample_rate(&self) -> u32 {
        crate::synth::Synth::sample_rate(self)
    }
    fn channels(&self) -> usize {
        1
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        Ok(self.fill(out))
    }
    fn seek(&mut self, frame: u64) -> Result<()> {
        if frame != 0 {
            return Err(Error::invalid("A synthesizer can only seek to its start"));
        }
        self.reset();
        Ok(())
    }
}

/// What a voice plays.
pub enum Sound {
    Memory(Arc<Audio>),
    Stream(Box<dyn Stream>),
}
impl Sound {
    pub fn stream(stream: impl Stream + 'static) -> Self {
        Self::Stream(Box::new(stream))
    }
}
impl From<Arc<Audio>> for Sound {
    fn from(audio: Arc<Audio>) -> Self {
        Self::Memory(audio)
    }
}
impl From<Audio> for Sound {
    fn from(audio: Audio) -> Self {
        Self::Memory(Arc::new(audio))
    }
}
impl From<Box<dyn Stream>> for Sound {
    fn from(stream: Box<dyn Stream>) -> Self {
        Self::Stream(stream)
    }
}
impl<B: AsRef<[u8]> + Send + 'static> From<crate::stream::Decoder<B>> for Sound {
    fn from(decoder: crate::stream::Decoder<B>) -> Self {
        Self::stream(decoder)
    }
}
#[cfg(feature = "synth")]
impl From<crate::synth::Synth> for Sound {
    fn from(synth: crate::synth::Synth) -> Self {
        Self::stream(synth)
    }
}

/// Identifies a voice in commands and events. Unique for the life of a mixer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct VoiceId(u64);

/// A bus: [`BusId::MASTER`] or a named bus from [`MixerHandle::bus`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BusId(usize);
impl BusId {
    pub const MASTER: BusId = BusId(0);
}

/// Which part of a sound repeats.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Loop {
    #[default]
    Off,
    /// The whole sound, from its end back to its start.
    Whole,
    /// Plays from the start, then repeats the source frames `start..end`.
    Region { start: u64, end: u64 },
}

/// A change over a duration, with a curve. Zero seconds changes at once.
#[derive(Clone, Copy, Debug)]
pub struct Fade {
    pub seconds: f64,
    pub curve: FadeCurve,
}
impl Fade {
    pub const NONE: Fade = Fade {
        seconds: 0.0,
        curve: FadeCurve::Linear,
    };
    pub const fn linear(seconds: f64) -> Self {
        Self {
            seconds,
            curve: FadeCurve::Linear,
        }
    }
    pub const fn new(seconds: f64, curve: FadeCurve) -> Self {
        Self { seconds, curve }
    }
}
impl Default for Fade {
    fn default() -> Self {
        Self::NONE
    }
}

/// How a voice starts playing.
#[derive(Clone, Copy, Debug)]
pub struct Play {
    pub bus: BusId,
    /// Linear gain, at least 0. Default 1.
    pub gain: f32,
    /// -1 (left) to 1 (right). Default 0.
    pub pan: f32,
    pub looping: Loop,
    pub fade_in: Fade,
    /// Decides which voice gives way when too many play. Default 128.
    pub priority: u8,
}
impl Default for Play {
    fn default() -> Self {
        Self {
            bus: BusId::MASTER,
            gain: 1.0,
            pan: 0.0,
            looping: Loop::Off,
            fade_in: Fade::NONE,
            priority: 128,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MixerConfig {
    /// The output's sample rate. Default 48000.
    pub sample_rate: u32,
    /// The output's channels: 1 or 2. Default 2.
    pub channels: usize,
    /// Most voices that play at once, 1..=1024. Default 32.
    pub max_voices: usize,
    /// Named buses besides the master, such as `music` and `effects`.
    pub buses: Vec<String>,
    /// Commands that can wait for the mixer, 1..=65536. Default 1024.
    pub queue: usize,
}
impl Default for MixerConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            max_voices: 32,
            buses: Vec::new(),
            queue: 1024,
        }
    }
}

/// Something that happened to a voice, reported by [`MixerHandle::poll_event`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixerEvent {
    /// The voice finished, was stopped, or finished fading out after being stolen.
    Ended(VoiceId),
    /// Another voice took this voice's place; it fades out, then ends.
    Stolen(VoiceId),
    /// The voice did not start because too many voices of higher priority play.
    Rejected(VoiceId),
    /// The voice's stream failed and the voice stopped.
    Failed(VoiceId, ErrorCode),
}

enum Action {
    Play(Box<Voice>),
    Stop(VoiceId, u64, FadeCurve),
    Gain(VoiceId, f64, u64, FadeCurve),
    Pan(VoiceId, f64, u64, FadeCurve),
    BusGain(usize, f64, u64, FadeCurve),
    StopAll(u64, FadeCurve),
}
struct Command {
    frame: Option<u64>,
    sequence: u64,
    action: Action,
}
struct Returned {
    event: Option<MixerEvent>,
    voice: Option<Box<Voice>>,
}

struct Bus {
    gain: Ramp,
    buffer: Vec<f32>,
}

/// The audio thread's half of the mixer. See the [module documentation](self).
pub struct Mixer {
    rate: u32,
    channels: usize,
    max_voices: usize,
    frame: u64,
    order: u64,
    voices: Vec<Option<Box<Voice>>>,
    buses: Vec<Bus>,
    pending: Vec<Command>,
    commands: Receiver<Command>,
    returns: SyncSender<Returned>,
    retired: Vec<Returned>,
    clock: Arc<AtomicU64>,
    steal_fade: u64,
}

/// The game's half of the mixer: starts, changes and stops voices.
pub struct MixerHandle {
    rate: u32,
    channels: usize,
    buses: Vec<String>,
    commands: SyncSender<Command>,
    returns: Receiver<Returned>,
    clock: Arc<AtomicU64>,
    next_voice: u64,
    next_sequence: u64,
    tables: Vec<(u32, Arc<SincTable>)>,
}

impl Mixer {
    pub fn new(config: MixerConfig) -> Result<(Mixer, MixerHandle)> {
        let MixerConfig {
            sample_rate,
            channels,
            max_voices,
            buses,
            queue,
        } = config;
        Limits::default().check(0, channels, sample_rate)?;
        if !(1..=2).contains(&channels) {
            return Err(Error::invalid("The mixer's output has 1 or 2 channels"));
        }
        if !(1..=1024).contains(&max_voices) || !(1..=65536).contains(&queue) {
            return Err(Error::invalid(
                "max_voices must be 1..=1024 and queue 1..=65536",
            ));
        }
        for (i, name) in buses.iter().enumerate() {
            if name.is_empty() || buses[..i].contains(name) {
                return Err(Error::invalid(format!(
                    "Bus names must be unique and not empty: {name:?}"
                )));
            }
        }
        if buses.len() > 255 {
            return Err(Error::invalid("A mixer has at most 255 named buses"));
        }
        let slots = max_voices + TAILS;
        let (command_tx, command_rx) = sync_channel(queue);
        let (return_tx, return_rx) = sync_channel(slots + queue);
        let clock = Arc::new(AtomicU64::new(0));
        let mixer = Mixer {
            rate: sample_rate,
            channels,
            max_voices,
            frame: 0,
            order: 0,
            voices: (0..slots).map(|_| None).collect(),
            buses: (0..=buses.len())
                .map(|_| Bus {
                    gain: Ramp::constant(1.0),
                    buffer: vec![0.0; BLOCK * channels],
                })
                .collect(),
            pending: Vec::with_capacity(queue),
            commands: command_rx,
            returns: return_tx,
            retired: Vec::with_capacity(slots + queue),
            clock: clock.clone(),
            steal_fade: (STEAL_FADE * sample_rate as f64).round() as u64,
        };
        let handle = MixerHandle {
            rate: sample_rate,
            channels,
            buses,
            commands: command_tx,
            returns: return_rx,
            clock,
            next_voice: 0,
            next_sequence: 0,
            tables: Vec::new(),
        };
        Ok((mixer, handle))
    }
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    /// Frames rendered so far.
    pub fn frame(&self) -> u64 {
        self.frame
    }
    /// Voices playing or fading out.
    pub fn active_voices(&self) -> usize {
        self.voices.iter().flatten().count()
    }

    /// Renders the next `out.len() / channels` frames into the interleaved `out`.
    /// Allocates nothing, takes no lock and does not block.
    pub fn render(&mut self, out: &mut [f32]) {
        let ch = self.channels;
        let frames = out.len() / ch;
        out.fill(0.0);
        self.receive();
        let start = self.frame;
        let end = start + frames as u64;
        let mut cursor = start;
        loop {
            self.apply_due(cursor);
            if cursor >= end {
                break;
            }
            let next = self
                .pending
                .iter()
                .filter_map(|c| c.frame)
                .filter(|&f| f > cursor)
                .min()
                .unwrap_or(end);
            let stop = next.min(end).min(cursor + BLOCK as u64);
            let from = (cursor - start) as usize * ch;
            let to = (stop - start) as usize * ch;
            self.mix(cursor, &mut out[from..to]);
            cursor = stop;
        }
        self.frame = end;
        self.send_returns();
        self.clock.store(end, Ordering::Release);
    }

    fn receive(&mut self) {
        while self.pending.len() < self.pending.capacity() {
            match self.commands.try_recv() {
                Ok(command) => self.pending.push(command),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    /// Applies the pending commands due at `frame`, in order of frame and sending.
    fn apply_due(&mut self, frame: u64) {
        loop {
            let due = self
                .pending
                .iter()
                .enumerate()
                .filter(|(_, c)| c.frame.unwrap_or(0) <= frame)
                .min_by_key(|(_, c)| (c.frame.unwrap_or(0), c.sequence))
                .map(|(i, _)| i);
            let Some(index) = due else {
                return;
            };
            let command = self.pending.swap_remove(index);
            self.apply(frame, command.action);
        }
    }

    fn voice_mut(&mut self, id: VoiceId) -> Option<&mut Voice> {
        self.voices
            .iter_mut()
            .flatten()
            .find(|v| v.id == id)
            .map(|v| &mut **v)
    }

    fn apply(&mut self, frame: u64, action: Action) {
        match action {
            Action::Play(voice) => self.start(frame, voice),
            Action::Stop(id, len, curve) => {
                if let Some(v) = self.voice_mut(id) {
                    v.stop(frame, len, curve);
                }
                self.retire_done();
            }
            Action::Gain(id, gain, len, curve) => {
                if let Some(v) = self.voice_mut(id) {
                    v.gain.retarget(frame, gain, len, curve);
                }
            }
            Action::Pan(id, pan, len, curve) => {
                if let Some(v) = self.voice_mut(id) {
                    v.pan.retarget(frame, pan, len, curve);
                }
            }
            Action::BusGain(bus, gain, len, curve) => {
                self.buses[bus].gain.retarget(frame, gain, len, curve);
            }
            Action::StopAll(len, curve) => {
                for v in self.voices.iter_mut().flatten() {
                    v.stop(frame, len, curve);
                }
                self.retire_done();
            }
        }
    }

    fn start(&mut self, frame: u64, mut voice: Box<Voice>) {
        voice.order = self.order;
        self.order += 1;
        voice.start(frame);
        let live = self.voices.iter().flatten().filter(|v| v.live()).count();
        if live >= self.max_voices {
            let victim = self
                .voices
                .iter_mut()
                .flatten()
                .filter(|v| v.live())
                .min_by_key(|v| (v.priority, v.order));
            match victim {
                Some(v) if v.priority <= voice.priority => {
                    v.stop(frame, self.steal_fade, FadeCurve::Linear);
                    let id = v.id;
                    self.event(MixerEvent::Stolen(id));
                }
                _ => {
                    let id = voice.id;
                    self.retire(voice, Some(MixerEvent::Rejected(id)));
                    return;
                }
            }
        }
        self.retire_done();
        let slot = match self.voices.iter().position(Option::is_none) {
            Some(slot) => slot,
            None => {
                // Every slot holds a live or fading voice: cut the oldest fading one.
                let oldest = self
                    .voices
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| v.as_ref().is_some_and(|v| !v.live()))
                    .min_by_key(|(_, v)| v.as_ref().map(|v| v.order))
                    .map(|(i, _)| i);
                let Some(slot) = oldest else {
                    let id = voice.id;
                    self.retire(voice, Some(MixerEvent::Rejected(id)));
                    return;
                };
                let old = self.voices[slot].take().expect("slot is occupied");
                let id = old.id;
                self.retire(old, Some(MixerEvent::Ended(id)));
                slot
            }
        };
        self.voices[slot] = Some(voice);
    }

    fn mix(&mut self, first: u64, out: &mut [f32]) {
        let ch = self.channels;
        let frames = out.len() / ch;
        for bus in &mut self.buses {
            bus.buffer[..frames * ch].fill(0.0);
        }
        for v in self.voices.iter_mut().flatten() {
            let buffer = &mut self.buses[v.bus].buffer[..frames * ch];
            v.render(first, frames, buffer, ch);
        }
        self.retire_done();
        let master = self.buses[0].gain;
        for (b, bus) in self.buses.iter().enumerate() {
            let constant = !bus.gain.changing(first) && !master.changing(first);
            let fixed = bus_gain(b, &bus.gain, &master, first);
            for j in 0..frames {
                let g = if constant {
                    fixed
                } else {
                    bus_gain(b, &bus.gain, &master, first + j as u64)
                };
                for c in 0..ch {
                    out[j * ch + c] += (bus.buffer[j * ch + c] as f64 * g) as f32;
                }
            }
        }
    }

    fn retire_done(&mut self) {
        for slot in 0..self.voices.len() {
            if self.voices[slot].as_ref().is_some_and(|v| v.done()) {
                let voice = self.voices[slot].take().expect("slot is occupied");
                let event = match voice.state {
                    State::Failed(code) => MixerEvent::Failed(voice.id, code),
                    _ => MixerEvent::Ended(voice.id),
                };
                self.retire(voice, Some(event));
            }
        }
    }

    fn event(&mut self, event: MixerEvent) {
        if self.retired.len() < self.retired.capacity() {
            self.retired.push(Returned {
                event: Some(event),
                voice: None,
            });
        }
    }

    /// Queues a finished voice to be freed by the handle. If the queue back to the
    /// handle is full because events are not polled, the voice is freed here.
    fn retire(&mut self, voice: Box<Voice>, event: Option<MixerEvent>) {
        if self.retired.len() < self.retired.capacity() {
            self.retired.push(Returned {
                event,
                voice: Some(voice),
            });
        }
    }

    fn send_returns(&mut self) {
        let mut sent = 0;
        while sent < self.retired.len() {
            let item = std::mem::replace(
                &mut self.retired[sent],
                Returned {
                    event: None,
                    voice: None,
                },
            );
            match self.returns.try_send(item) {
                Ok(()) => sent += 1,
                Err(TrySendError::Full(item)) => {
                    self.retired[sent] = item;
                    break;
                }
                Err(TrySendError::Disconnected(_)) => sent += 1,
            }
        }
        self.retired.drain(..sent);
    }
}

fn bus_gain(bus: usize, gain: &Ramp, master: &Ramp, frame: u64) -> f64 {
    if bus == 0 {
        master.value(frame)
    } else {
        gain.value(frame) * master.value(frame)
    }
}

impl MixerHandle {
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    /// The named bus, if the mixer has it.
    pub fn bus(&self, name: &str) -> Option<BusId> {
        self.buses
            .iter()
            .position(|b| b == name)
            .map(|i| BusId(i + 1))
    }
    /// Frames the mixer had rendered when it last finished a render.
    pub fn frame(&self) -> u64 {
        self.clock.load(Ordering::Acquire)
    }
    /// Commands that apply at the start of the next render.
    pub fn now(&mut self) -> Commands<'_> {
        Commands {
            handle: self,
            frame: None,
        }
    }
    /// Commands that apply exactly at output frame `frame`, or at the next render if
    /// it has passed.
    pub fn at(&mut self, frame: u64) -> Commands<'_> {
        Commands {
            handle: self,
            frame: Some(frame),
        }
    }
    pub fn play(&mut self, sound: impl Into<Sound>, play: &Play) -> Result<VoiceId> {
        self.now().play(sound, play)
    }
    pub fn stop(&mut self, voice: VoiceId, fade: Fade) -> Result<()> {
        self.now().stop(voice, fade)
    }
    pub fn stop_all(&mut self, fade: Fade) -> Result<()> {
        self.now().stop_all(fade)
    }
    pub fn set_gain(&mut self, voice: VoiceId, gain: f32, fade: Fade) -> Result<()> {
        self.now().set_gain(voice, gain, fade)
    }
    pub fn set_pan(&mut self, voice: VoiceId, pan: f32, fade: Fade) -> Result<()> {
        self.now().set_pan(voice, pan, fade)
    }
    pub fn set_bus_gain(&mut self, bus: BusId, gain: f32, fade: Fade) -> Result<()> {
        self.now().set_bus_gain(bus, gain, fade)
    }
    /// Returns the next event, freeing finished voices on this thread.
    pub fn poll_event(&mut self) -> Option<MixerEvent> {
        while let Ok(returned) = self.returns.try_recv() {
            drop(returned.voice);
            if returned.event.is_some() {
                return returned.event;
            }
        }
        None
    }

    fn frames(&self, fade: Fade) -> Result<(u64, FadeCurve)> {
        if !fade.seconds.is_finite() || fade.seconds < 0.0 || fade.seconds > 3600.0 {
            return Err(Error::invalid("A fade lasts 0 to 3600 seconds"));
        }
        Ok(((fade.seconds * self.rate as f64).round() as u64, fade.curve))
    }
    fn table(&mut self, source_rate: u32) -> Arc<SincTable> {
        if let Some((_, table)) = self.tables.iter().find(|(r, _)| *r == source_rate) {
            return table.clone();
        }
        let table = Arc::new(SincTable::new(source_rate as f64 / self.rate as f64));
        self.tables.push((source_rate, table.clone()));
        table
    }
    fn send(&mut self, frame: Option<u64>, action: Action) -> Result<()> {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let command = Command {
            frame,
            sequence,
            action,
        };
        match self.commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(Error::limit("The mixer's command queue is full")),
            Err(TrySendError::Disconnected(_)) => Err(Error::invalid("The mixer was dropped")),
        }
    }
}

/// Commands applied at one moment: see [`MixerHandle::now`] and [`MixerHandle::at`].
pub struct Commands<'a> {
    handle: &'a mut MixerHandle,
    frame: Option<u64>,
}
impl Commands<'_> {
    /// Starts a voice and returns its id. Fails if the sound has more than two
    /// channels, a loop region is outside it, or the command queue is full.
    pub fn play(self, sound: impl Into<Sound>, play: &Play) -> Result<VoiceId> {
        let handle = self.handle;
        let sound = sound.into();
        let (rate, channels, frames) = match &sound {
            Sound::Memory(audio) => (
                audio.sample_rate(),
                audio.channels(),
                Some(audio.frames() as u64),
            ),
            Sound::Stream(stream) => (stream.sample_rate(), stream.channels(), stream.frames()),
        };
        Limits::default().check(0, channels, rate)?;
        if !(1..=2).contains(&channels) {
            return Err(Error::invalid("A voice plays a mono or stereo sound"));
        }
        if !play.gain.is_finite() || play.gain < 0.0 || !play.pan.is_finite() {
            return Err(Error::invalid(
                "Gain must be finite and non-negative, and pan finite",
            ));
        }
        if play.bus.0 > handle.buses.len() {
            return Err(Error::invalid("Unknown bus"));
        }
        if let Loop::Region { start, end } = play.looping {
            if start >= end || frames.is_some_and(|n| end > n) {
                return Err(Error::invalid(
                    "A loop region needs start < end <= the sound's frames",
                ));
            }
        }
        let fade_in = handle.frames(play.fade_in)?;
        let step = (((rate as u128) << 32) + handle.rate as u128 / 2) / handle.rate as u128;
        let step = step as u64;
        let table = (step != ONE).then(|| handle.table(rate));
        let id = VoiceId(handle.next_voice);
        handle.next_voice += 1;
        let feed = match sound {
            Sound::Memory(audio) => Feed::Memory(audio),
            Sound::Stream(stream) => Feed::Stream(stream),
        };
        let mut voice = Box::new(Voice::new(id, feed, channels, play.looping, step, table));
        voice.bus = play.bus.0;
        voice.priority = play.priority;
        voice.gain = Ramp::constant(play.gain as f64);
        voice.pan = Ramp::constant(play.pan.clamp(-1.0, 1.0) as f64);
        voice.fade_in = fade_in;
        handle.send(self.frame, Action::Play(voice))?;
        Ok(id)
    }
    /// Stops a voice, fading it out first. Unknown or ended voices are ignored.
    pub fn stop(self, voice: VoiceId, fade: Fade) -> Result<()> {
        let (len, curve) = self.handle.frames(fade)?;
        self.handle
            .send(self.frame, Action::Stop(voice, len, curve))
    }
    pub fn stop_all(self, fade: Fade) -> Result<()> {
        let (len, curve) = self.handle.frames(fade)?;
        self.handle.send(self.frame, Action::StopAll(len, curve))
    }
    pub fn set_gain(self, voice: VoiceId, gain: f32, fade: Fade) -> Result<()> {
        if !gain.is_finite() || gain < 0.0 {
            return Err(Error::invalid("Gain must be finite and non-negative"));
        }
        let (len, curve) = self.handle.frames(fade)?;
        self.handle
            .send(self.frame, Action::Gain(voice, gain as f64, len, curve))
    }
    /// Pans a voice, from -1 (left) to 1 (right).
    pub fn set_pan(self, voice: VoiceId, pan: f32, fade: Fade) -> Result<()> {
        if !pan.is_finite() {
            return Err(Error::invalid("Pan must be finite"));
        }
        let (len, curve) = self.handle.frames(fade)?;
        let pan = pan.clamp(-1.0, 1.0) as f64;
        self.handle
            .send(self.frame, Action::Pan(voice, pan, len, curve))
    }
    /// Sets a bus's gain; [`BusId::MASTER`] sets the master gain.
    pub fn set_bus_gain(self, bus: BusId, gain: f32, fade: Fade) -> Result<()> {
        if !gain.is_finite() || gain < 0.0 {
            return Err(Error::invalid("Gain must be finite and non-negative"));
        }
        if bus.0 > self.handle.buses.len() {
            return Err(Error::invalid("Unknown bus"));
        }
        let (len, curve) = self.handle.frames(fade)?;
        self.handle
            .send(self.frame, Action::BusGain(bus.0, gain as f64, len, curve))
    }
}
