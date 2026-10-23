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
//! changes, and stops, can fade over a duration with a [`FadeCurve`].
//!
//! A voice also has a playback rate, which changes speed and pitch together, and a
//! pitch offset in semitones on top of it. Both can change while the voice plays, with
//! a ramp. [`Variation`] draws a random pitch and gain for each start of a repeated
//! sound, such as a footstep, from a generator seeded by [`MixerConfig::seed`].
//!
//! A voice can start partway through its sound ([`Play::start`]), and be paused and
//! resumed, optionally with a fade; it keeps its position, its fades and its loop.
//! Pausing a bus pauses every voice routed through it and no others, so a pause menu
//! can stop the game's sounds and keep its own. [`MixerHandle::voice_status`] reads a
//! voice's state and position as of the last render, without blocking the audio side. A loop of a
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
//! A sound can also limit its own voices with [`Instances`]: at most so many at once,
//! with a policy for one more, and a minimum gap between starts. A refused start
//! reports [`MixerEvent::Limited`].
//!
//! A voice can have a 2D position relative to the listener ([`Play::position`],
//! [`MixerHandle::set_position`]), which sets its pan and a distance gain through the
//! mixer's [`Space2d`].
//!
//! Every voice plays through a bus: the master bus, or one of the named buses of
//! [`MixerConfig::buses`], which play through the master. A voice's samples are scaled
//! by its gain, its fades, its pan, its bus's gain and the master gain.
//! [`effects`] adds ordered bus chains, ramped filters, compression/ducking, delay
//! and reverb. Sidechain dependencies order named buses before master processing.
//!
//! Many voices together can go beyond full scale. [`MixerConfig::limiter`] enables a
//! [`MasterLimiter`] on the master bus, off by default, which keeps the output at or
//! below its ceiling and passes quieter signals unchanged. Its look-ahead, 0 by
//! default, delays the output by [`MixerHandle::latency`] frames.
//!
//! # Rendering
//!
//! [`Mixer::render`] fills an interleaved buffer of the output's channels (1/2/4/6/8) at
//! the output's sample rate. It writes `out.len() / channels` whole frames and sets any
//! remaining samples to 0; a buffer shorter than a frame renders nothing, but still
//! applies the changes that are due.
//!
//! Sources at another sample rate, and voices whose rate or pitch differs from 1, are
//! resampled with a 16-tap windowed-sinc filter
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
//! [`Mixer::render`] allocates nothing, takes no lock that can wait and does not block.
//! Changes reach it through a bounded single-producer, single-consumer queue of the
//! crate's own, allocated when the mixer is created. The handle and the mixer each own
//! one end and one counter, so a slot is only ever used by one side at a time; each slot
//! is taken with `try_lock`, which never blocks and, under this protocol, is never
//! contended. A full queue is reported to the handle; neither side ever waits. Voices
//! and their buffers are built by the handle, on the game's thread. Finished voices go
//! back to the handle through a second such queue and are freed by
//! [`MixerHandle::poll_event`], so the audio thread frees no memory either. Poll events
//! regularly, such as once per game frame.
//!
//! # Timing and determinism
//!
//! Every change applies at an output frame. [`MixerHandle::at`] schedules changes at an
//! exact frame, counted from the first rendered frame; they apply there whatever the
//! buffer sizes, splitting a buffer if needed. Changes sent without a frame apply at the
//! start of the next render. The same changes at the same frames give the same samples
//! on every platform: resampling, panning and fades use only basic IEEE arithmetic and
//! the crate's own sine series.
pub mod effects;
mod limiter;
#[cfg(feature = "music")]
#[cfg_attr(docsrs, doc(cfg(feature = "music")))]
pub mod music;
mod queue;
pub mod spatial;
mod status;
mod voice;
pub use spatial::{
    Attenuation, DistanceModel, Emitter, Listener, OutputLayout, Placement, SpatialConfig,
};

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub use limiter::MasterLimiter;
use queue::{Consumer, Producer, PushError};
pub use status::{VoiceState, VoiceStatus};
use voice::{Feed, ONE, Ramp, Resampling, SincBank, SincTable, State, Voice};

use crate::{Audio, Error, ErrorCode, FadeCurve, Limits, Result, TimePosition};

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
    /// `ErrorCode::Underrun` is temporary: the mixer emits silence and retries
    /// next render segment, preserving source position. Other errors fail the voice.
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
#[cfg_attr(docsrs, doc(cfg(feature = "synth")))]
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
#[cfg_attr(docsrs, doc(cfg(feature = "synth")))]
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
    /// Playback rate, which changes speed and pitch together: 2 plays twice as fast
    /// and an octave higher. 1/16..=8. Default 1.
    pub rate: f32,
    /// Pitch offset in semitones on top of the rate; fractions are cents, so 0.01 is
    /// one cent. -48..=48. Rate, pitch and [`Variation::pitch`] together span
    /// 1/1024..=512 times the speed. Default 0.
    pub pitch: f32,
    /// Random variation of pitch and gain, drawn from the mixer's seed.
    pub variation: Variation,
    /// Where in the sound to start, in seconds or source frames; a negative position
    /// counts from the end of a sound of known length. A stream seeks there before the
    /// voice is sent. Default the start.
    pub start: TimePosition,
    /// Limits on how many voices of this sound play at once and how often it starts.
    pub instances: Option<Instances>,
    /// The sound's position relative to the listener, such as the camera's centre, in
    /// the units of [`MixerConfig::space`]. It sets the voice's pan, replacing
    /// [`Play::pan`], and a distance gain on top of [`Play::gain`]. Default none.
    pub position: Option<[f32; 2]>,
    /// Optional 3D point emitter. Takes precedence over pan; cannot combine with position.
    pub emitter: Option<Emitter>,
}

/// How a 2D position relative to the listener becomes a pan and a gain.
///
/// The horizontal offset `x` pans the sound with the mixer's constant-power law:
/// `pan = x / width`, limited to -1..=1, so a sound `width` units to the right is fully
/// right. The distance `d = √(x² + y²)` sets a gain of 1 up to `min_distance` and 0 from
/// `max_distance`; in between, [`Distance::Linear`] falls in a straight line and
/// [`Distance::Inverse`] follows `min/d`, shifted and scaled so it reaches 0 at the
/// maximum instead of jumping there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Space2d {
    /// Horizontal offset at which a sound is fully to one side. Default 1.
    pub width: f32,
    /// Distance up to which a sound plays at full gain. Default 1.
    pub min_distance: f32,
    /// Distance from which a sound is silent. Default 10.
    pub max_distance: f32,
    pub model: Distance,
}
impl Default for Space2d {
    fn default() -> Self {
        Self {
            width: 1.0,
            min_distance: 1.0,
            max_distance: 10.0,
            model: Distance::Linear,
        }
    }
}
impl Space2d {
    /// The pan and the distance gain of a sound at `[x, y]` from the listener.
    pub fn place(&self, [x, y]: [f32; 2]) -> (f32, f32) {
        let (x, y) = (x as f64, y as f64);
        let pan = (x / self.width as f64).clamp(-1.0, 1.0);
        let d = (x * x + y * y).sqrt();
        let (min, max) = (self.min_distance as f64, self.max_distance as f64);
        let gain = if d <= min {
            1.0
        } else if d >= max {
            0.0
        } else {
            match self.model {
                Distance::Linear => 1.0 - (d - min) / (max - min),
                Distance::Inverse => {
                    let floor = min / max;
                    (min / d - floor) / (1.0 - floor)
                }
            }
        };
        (pan as f32, gain as f32)
    }
    fn validate(&self) -> Result<()> {
        let ok = self.width.is_finite()
            && self.width > 0.0
            && self.min_distance.is_finite()
            && self.min_distance >= 0.0
            && self.max_distance.is_finite()
            && self.max_distance > self.min_distance;
        if ok {
            Ok(())
        } else {
            Err(Error::invalid(
                "A 2D space needs a positive width and 0 <= min_distance < max_distance, all finite",
            ))
        }
    }
}

/// How gain falls with distance in a [`Space2d`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Distance {
    #[default]
    Linear,
    Inverse,
}

/// Limits for one sound: at most `max` voices with the same key play at once, and a
/// new one starts no sooner than `min_gap` seconds after the last, so that ten coins
/// collected together do not sum into distortion.
///
/// When `max` voices already play, `policy` decides: refuse the new voice, or stop
/// the oldest or the quietest one with a 5 ms fade. A refused start reports
/// [`MixerEvent::Limited`]; a start within the gap is always refused. Limits are
/// judged with the mixer's bound on voices and its priorities when the start is
/// applied, so scheduled starts are judged at their own frame, and a start refused for
/// any reason stops no voice and does not begin a gap. Only instances of no higher
/// priority than the new voice give way to it. Each start is checked against
/// its own gap, from the sound's last start. The mixer remembers the last starts of up
/// to 1024 limited sounds within any 60 seconds; a limited start beyond that is refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instances {
    /// Voices with the same key share the limits. [`Instances::of`] keys a sound by
    /// its identity, so every voice of that audio counts, also through clones and
    /// `Arc`s. Keys you pick yourself should be below 2^63, where those identities are.
    pub key: u64,
    /// Most voices playing at once; 0 for no limit.
    pub max: u16,
    /// Seconds from one start to the next, 0..=60.
    pub min_gap: f64,
    pub policy: OverLimit,
}
impl Instances {
    /// Limits keyed by `key`, with no limit set yet.
    pub const fn new(key: u64) -> Self {
        Self {
            key,
            max: 0,
            min_gap: 0.0,
            policy: OverLimit::Refuse,
        }
    }
    /// Limits keyed by a decoded sound's identity, which no other audio value shares,
    /// even after this one is freed.
    pub fn of(sound: &Audio) -> Self {
        Self::new(sound.id())
    }
    pub const fn max(self, max: u16) -> Self {
        Self { max, ..self }
    }
    pub const fn min_gap(self, seconds: f64) -> Self {
        Self {
            min_gap: seconds,
            ..self
        }
    }
    pub const fn policy(self, policy: OverLimit) -> Self {
        Self { policy, ..self }
    }
}

/// What happens when a sound already plays its [`Instances::max`] voices.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OverLimit {
    /// The new voice does not start.
    #[default]
    Refuse,
    /// The voice that started first fades out and the new one starts.
    StopOldest,
    /// The voice with the lowest gain fades out and the new one starts.
    StopQuietest,
}

/// Sounds with instance limits whose last start the mixer remembers at once. An
/// entry is kept for [`MAX_GAP`] seconds, the longest gap a start can ask for; a
/// limited start that finds the table full is refused.
const STARTS: usize = 1024;
/// The longest minimum gap between starts, in seconds.
const MAX_GAP: f64 = 60.0;
impl Default for Play {
    fn default() -> Self {
        Self {
            bus: BusId::MASTER,
            gain: 1.0,
            pan: 0.0,
            looping: Loop::Off,
            fade_in: Fade::NONE,
            priority: 128,
            rate: 1.0,
            pitch: 0.0,
            variation: Variation::NONE,
            start: TimePosition::Frames(0),
            instances: None,
            position: None,
            emitter: None,
        }
    }
}

/// Random variation for sounds that repeat, such as footsteps, coins and impacts.
///
/// Each start draws a pitch offset uniformly within `±pitch` semitones and a gain
/// within `±gain_db` decibels from the mixer's generator, seeded by
/// [`MixerConfig::seed`] or [`MixerHandle::reseed`]. The same seed and the same
/// sequence of starts give the same variations on every platform.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Variation {
    /// 0..=24 semitones.
    pub pitch: f32,
    /// 0..=24 decibels.
    pub gain_db: f32,
}
impl Variation {
    pub const NONE: Variation = Variation {
        pitch: 0.0,
        gain_db: 0.0,
    };
}

#[derive(Clone, Debug)]
pub struct MixerConfig {
    /// The output's sample rate. Default 48000.
    pub sample_rate: u32,
    /// Output layout: 1 mono, 2 stereo, 4 quad, 6 5.1, 8 7.1. Default 2.
    pub channels: usize,
    /// Most voices that play at once, 1..=1024. Default 32.
    pub max_voices: usize,
    /// Named buses besides the master, such as `music` and `effects`.
    pub buses: Vec<String>,
    /// Commands that can wait for the mixer, 1..=65536. Default 1024.
    pub queue: usize,
    /// Seed of the generator that draws each voice's [`Variation`]. Default 0.
    pub seed: u64,
    /// How positions of voices become pans and gains.
    pub space: Space2d,
    pub listener: Listener,
    pub spatial: SpatialConfig,
    /// A peak limiter on the master bus, off by default. Its look-ahead delays the
    /// output, which [`MixerHandle::latency`] reports.
    pub limiter: Option<MasterLimiter>,
}
impl Default for MixerConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            max_voices: 32,
            buses: Vec::new(),
            queue: 1024,
            seed: 0,
            space: Space2d::default(),
            listener: Listener::default(),
            spatial: SpatialConfig::default(),
            limiter: None,
        }
    }
}

/// Something that happened to a voice, reported by [`MixerHandle::poll_event`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MixerEvent {
    /// The voice finished, was stopped, or finished fading out after being stolen.
    Ended(VoiceId),
    /// Another voice took this voice's place; it fades out, then ends.
    Stolen(VoiceId),
    /// The voice did not start because too many voices of higher priority play.
    Rejected(VoiceId),
    /// The voice did not start because of its [`Instances`] limit.
    Limited(VoiceId),
    /// The voice's stream failed and the voice stopped.
    Failed(VoiceId, ErrorCode),
    /// A bus effect chain was installed at this output frame.
    EffectsApplied(BusId, u64),
    /// An effect edit was refused; the existing chain and settings are unchanged.
    EffectsRejected(BusId, ErrorCode),
    /// A whole music operation took effect at this output frame; a fade may continue.
    #[cfg(feature = "music")]
    MusicApplied(music::MusicId, u64),
    /// A music operation was refused as a whole; the previous track is unchanged.
    #[cfg(feature = "music")]
    MusicRejected(music::MusicId, ErrorCode),
}

enum Action {
    Effects(Box<effects::Edit>),
    Effect(usize, usize, effects::Settings, u64, FadeCurve),
    #[cfg(feature = "music")]
    Music(Box<music::Request>),
    Play(Box<Voice>),
    Stop(VoiceId, u64, FadeCurve),
    Gain(VoiceId, f64, u64, FadeCurve),
    Pan(VoiceId, f64, u64, FadeCurve),
    Rate(VoiceId, f64, u64, FadeCurve),
    Pitch(VoiceId, f64, u64, FadeCurve),
    Position(VoiceId, f64, f64, u64, FadeCurve),
    Emitter(VoiceId, Emitter, u64, FadeCurve),
    Listener(Listener, u64, FadeCurve),
    Pause(VoiceId, u64, FadeCurve),
    Resume(VoiceId, u64, FadeCurve),
    PauseBus(usize, u64, FadeCurve),
    ResumeBus(usize, u64, FadeCurve),
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
    #[cfg(feature = "music")]
    music: Option<Box<music::Request>>,
}

struct Bus {
    gain: Ramp,
    /// Gain of pause and resume fades.
    pause: Ramp,
    pausing: bool,
    paused: bool,
    buffer: Vec<f32>,
    effects: Option<Box<effects::Chain>>,
    effect_clock: u64,
}

/// The audio thread's half of the mixer. See the [module documentation](self).
pub struct Mixer {
    effects: effects::AudioEffects,
    rate: u32,
    channels: usize,
    listener: Listener,
    spatial: SpatialConfig,
    max_voices: usize,
    frame: u64,
    order: u64,
    voices: Vec<Option<Box<Voice>>>,
    buses: Vec<Bus>,
    pending: Vec<Command>,
    commands: Consumer<Command>,
    returns: Producer<Returned>,
    retired: Vec<Returned>,
    clock: Arc<AtomicU64>,
    status: Arc<status::Table>,
    steal_fade: u64,
    /// Keys of sounds with instance limits and the frame of each one's last start
    /// (`None` for a free entry).
    starts: [(u64, Option<u64>); STARTS],
    /// Frames after which a start can no longer be within any gap.
    max_gap: u64,
    limiter: Option<limiter::Limiter>,
    limiter_gain: Arc<AtomicU32>,
}

/// The game's half of the mixer: starts, changes and stops voices.
pub struct MixerHandle {
    effects: effects::ControlEffects,
    rate: u32,
    channels: usize,
    buses: Vec<String>,
    commands: Producer<Command>,
    returns: Consumer<Returned>,
    clock: Arc<AtomicU64>,
    status: Arc<status::Table>,
    next_voice: u64,
    next_sequence: u64,
    tables: Vec<(u32, Arc<SincTable>)>,
    bank: Arc<SincBank>,
    random: SplitMix,
    space: Space2d,
    latency: u64,
    limited: bool,
    limiter_gain: Arc<AtomicU32>,
}

/// SplitMix64: a small, fixed generator, so variations are the same on every platform.
struct SplitMix(u64);
impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// Uniform in -1..1.
    fn symmetric(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}

impl Mixer {
    pub fn new(config: MixerConfig) -> Result<(Mixer, MixerHandle)> {
        let MixerConfig {
            sample_rate,
            channels,
            max_voices,
            buses,
            queue,
            seed,
            space,
            listener,
            spatial,
            limiter,
        } = config;
        space.validate()?;
        listener.validate()?;
        spatial.validate()?;
        Limits::default().check(0, channels, sample_rate)?;
        OutputLayout::from_channels(channels)?;
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
        // The limiter's buffers depend on the validated rate and channels.
        let limiter = limiter
            .map(|settings| limiter::Limiter::new(settings, sample_rate, channels))
            .transpose()?;
        let latency = limiter.as_ref().map_or(0, |l| l.latency() as u64);
        let has_limiter = limiter.is_some();
        let limiter_gain = Arc::new(AtomicU32::new(1f32.to_bits()));
        let slots = max_voices + TAILS;
        let (command_tx, command_rx) = queue::queue(queue);
        // Per render, at most `slots + queue` voices end or are refused, and as many
        // are stolen, so the way back holds every event and voice of a render.
        #[cfg(not(feature = "music"))]
        let returns_per_command = 1;
        #[cfg(feature = "music")]
        let returns_per_command = music::MAX_LAYERS + 1;
        // A musical command may start MAX_LAYERS voices, plus return its container.
        // Reserve their worst-case returns too; all storage is made off audio.
        let returns = queue
            .checked_mul(returns_per_command)
            .and_then(|n| n.checked_add(slots))
            .and_then(|n| n.checked_mul(2))
            .ok_or_else(|| Error::limit("Mixer return capacity overflow"))?;
        let (return_tx, return_rx) = queue::queue(returns);
        let clock = Arc::new(AtomicU64::new(0));
        let status = Arc::new(status::Table::new(slots));
        let (audio_effects, control_effects) = effects::create(queue, buses.len() + 1);
        let mixer = Mixer {
            effects: audio_effects,
            rate: sample_rate,
            channels,
            listener,
            spatial,
            max_voices,
            frame: 0,
            order: 0,
            voices: (0..slots).map(|_| None).collect(),
            buses: (0..=buses.len())
                .map(|_| Bus {
                    gain: Ramp::constant(1.0),
                    pause: Ramp::constant(1.0),
                    pausing: false,
                    paused: false,
                    buffer: vec![0.0; BLOCK * channels],
                    effects: None,
                    effect_clock: 0,
                })
                .collect(),
            pending: Vec::with_capacity(queue),
            commands: command_rx,
            returns: return_tx,
            retired: Vec::with_capacity(returns),
            clock: clock.clone(),
            status: status.clone(),
            steal_fade: (STEAL_FADE * sample_rate as f64).round() as u64,
            starts: [(0, None); STARTS],
            max_gap: (MAX_GAP * sample_rate as f64).round() as u64,
            limiter,
            limiter_gain: limiter_gain.clone(),
        };
        let handle = MixerHandle {
            effects: control_effects,
            rate: sample_rate,
            channels,
            buses,
            commands: command_tx,
            returns: return_rx,
            clock,
            status,
            next_voice: 0,
            next_sequence: 0,
            tables: Vec::new(),
            bank: Arc::new(SincBank::new()),
            random: SplitMix(seed),
            space,
            latency,
            limited: has_limiter,
            limiter_gain,
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
            // Segments also end where a bus's pause fade does, so its voices freeze
            // on that frame.
            let pause_ends = self
                .buses
                .iter()
                .filter(|b| b.pausing)
                .map(|b| b.pause.end());
            let next = self
                .pending
                .iter()
                .filter_map(|c| c.frame)
                .chain(pause_ends)
                .filter(|&f| f > cursor)
                .min()
                .unwrap_or(end);
            let stop = next.min(end).min(cursor + BLOCK as u64);
            let from = (cursor - start) as usize * ch;
            let to = (stop - start) as usize * ch;
            self.mix(cursor, &mut out[from..to]);
            cursor = stop;
        }
        if let Some(limiter) = &mut self.limiter {
            let lowest = limiter.process(&mut out[..frames * ch]);
            self.limiter_gain
                .store((lowest as f32).to_bits(), Ordering::Relaxed);
        }
        self.frame = end;
        // Read each voice up to its position before publishing it, and retire any
        // stream that fails doing so, also on renders of no frames.
        for v in self.voices.iter_mut().flatten() {
            v.settle();
        }
        self.retire_done();
        self.publish();
        self.send_returns();
        self.effects.send_returns();
        self.clock.store(end, Ordering::Release);
    }

    fn receive(&mut self) {
        // Refused music commands do not enter pending. They still consume this
        // render's receive budget, even if a producer keeps refilling the queue.
        let available = self.pending.capacity() - self.pending.len();
        for _ in 0..available {
            match self.commands.pop() {
                Some(command) => {
                    #[cfg(feature = "music")]
                    let Some(command) = self.quantize_music(command) else {
                        continue;
                    };
                    self.pending.push(command);
                }
                None => break,
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

    fn slot_of(&self, id: VoiceId) -> Option<usize> {
        self.voices
            .iter()
            .position(|v| v.as_ref().is_some_and(|v| v.id == id))
    }

    fn apply(&mut self, frame: u64, action: Action) {
        match action {
            Action::Effects(edit) => self.apply_effects(frame, edit),
            Action::Effect(bus, slot, settings, len, curve) => {
                self.apply_effect(bus, slot, settings, len, curve)
            }
            #[cfg(feature = "music")]
            Action::Music(request) => self.apply_music(frame, request),
            Action::Play(voice) => self.start(frame, voice),
            Action::Stop(id, len, curve) => {
                if let Some(slot) = self.slot_of(id) {
                    self.stop_slot(slot, frame, len, curve);
                }
                self.retire_done();
            }
            Action::Gain(id, gain, len, curve) => {
                if let Some(v) = self.voice_mut(id) {
                    v.gain.retarget(v.time(frame), gain, len, curve);
                }
            }
            Action::Pan(id, pan, len, curve) => {
                if let Some(v) = self.voice_mut(id) {
                    v.pan.retarget(v.time(frame), pan, len, curve);
                }
            }
            Action::Rate(id, rate, len, curve) => {
                if let Some(v) = self.voice_mut(id) {
                    v.rate.retarget(v.time(frame), rate, len, curve);
                }
            }
            Action::Pitch(id, pitch, len, curve) => {
                if let Some(v) = self.voice_mut(id) {
                    v.pitch.retarget(v.time(frame), pitch, len, curve);
                }
            }
            Action::Position(id, pan, gain, len, curve) => {
                if let Some(v) = self.voice_mut(id) {
                    if v.emitter.is_none() {
                        let time = v.time(frame);
                        v.pan.retarget(time, pan, len, curve);
                        v.distance.retarget(time, gain, len, curve);
                    }
                }
            }
            Action::Emitter(id, emitter, len, curve) => {
                let channels = self.channels;
                let placement = self.spatial.place_validated(
                    &self.listener,
                    &emitter,
                    OutputLayout::from_channels(self.channels).unwrap(),
                );
                if let Some(v) = self.voice_mut(id) {
                    v.emitter = Some(emitter);
                    v.update_spatial(placement, frame, len, curve, channels);
                }
            }
            Action::Listener(listener, len, curve) => {
                self.listener = listener;
                let layout = OutputLayout::from_channels(self.channels).unwrap();
                for v in self.voices.iter_mut().flatten() {
                    if let Some(emitter) = v.emitter {
                        let placement = self.spatial.place_validated(&listener, &emitter, layout);
                        v.update_spatial(placement, frame, len, curve, layout.channels());
                    }
                }
            }
            Action::Pause(id, len, curve) => {
                if let Some(v) = self.voice_mut(id) {
                    v.pause(v.time(frame), len, curve);
                }
            }
            Action::Resume(id, len, curve) => {
                if let Some(v) = self.voice_mut(id) {
                    v.resume(v.time(frame), len, curve);
                }
            }
            Action::PauseBus(b, len, curve) => {
                let bus = &mut self.buses[b];
                if !bus.paused {
                    bus.pause.retarget(frame, 0.0, len, curve);
                    bus.pausing = len > 0;
                    bus.paused = len == 0;
                }
            }
            Action::ResumeBus(b, len, curve) => {
                let bus = &mut self.buses[b];
                if bus.paused || bus.pausing {
                    bus.paused = false;
                    bus.pausing = false;
                    bus.pause.retarget(frame, 1.0, len, curve);
                }
            }
            Action::BusGain(bus, gain, len, curve) => {
                self.buses[bus].gain.retarget(frame, gain, len, curve);
            }
            Action::StopAll(len, curve) => {
                for slot in 0..self.voices.len() {
                    self.stop_slot(slot, frame, len, curve);
                }
                self.retire_done();
            }
        }
    }

    /// Refuses a voice before it starts.
    fn refuse(&mut self, voice: Box<Voice>, event: fn(VoiceId) -> MixerEvent) {
        let id = voice.id;
        self.retire(voice, Some(event(id)));
    }

    /// Stops the voice in `slot`. A voice frozen by its own pause, its bus or the
    /// master bus cannot play out a fade, so it stops at once.
    fn stop_slot(&mut self, slot: usize, frame: u64, len: u64, curve: FadeCurve) {
        let master_paused = self.buses[0].paused;
        let buses = &self.buses;
        if let Some(v) = self.voices[slot].as_mut() {
            let frozen = buses[v.bus].paused || master_paused;
            v.stop(v.time(frame), len, curve, frozen);
        }
    }

    fn start(&mut self, frame: u64, mut voice: Box<Voice>) {
        if let Some(emitter) = voice.emitter {
            let placement = self.spatial.place_validated(
                &self.listener,
                &emitter,
                OutputLayout::from_channels(self.channels).unwrap(),
            );
            voice.update_spatial(placement, frame, 0, FadeCurve::Linear, self.channels);
        }
        // Voices that played all their source no longer count against any limit. At
        // high speeds a voice can pass its source's end before reading it: read up to
        // the position, as its next frame would, to find out.
        for v in self.voices.iter_mut().flatten() {
            v.settle();
        }
        // Decide everything first, so that a refused voice changes nothing.
        let mut start_entry = None;
        if let Some(limit) = voice.instances {
            // The incoming start's own gap, measured from the sound's last start.
            let own = self
                .starts
                .iter()
                .position(|&(key, last)| key == limit.key && last.is_some());
            if let Some(last) = own.and_then(|i| self.starts[i].1) {
                if limit.min_gap > 0 && frame < last + limit.min_gap {
                    return self.refuse(voice, MixerEvent::Limited);
                }
            }
            // Its entry, a free one, or one too old to refuse any start. When every
            // entry is still needed, the start is refused rather than forgetting one.
            let max_gap = self.max_gap;
            let entry = own.or_else(|| {
                self.starts
                    .iter()
                    .position(|&(_, last)| last.is_none_or(|last| frame >= last + max_gap))
            });
            let Some(entry) = entry else {
                return self.refuse(voice, MixerEvent::Limited);
            };
            start_entry = Some(entry);
            if limit.max > 0 {
                let same = |v: &Voice| v.live() && v.instances.is_some_and(|l| l.key == limit.key);
                let candidates = || {
                    self.voices
                        .iter()
                        .enumerate()
                        .filter_map(|(i, v)| v.as_deref().filter(|v| same(v)).map(|v| (i, v)))
                };
                // Each start carries its own maximum, which may be lower than the
                // voices already playing: mark enough of them to fit.
                let playing = candidates().count();
                if playing >= limit.max as usize {
                    if limit.policy == OverLimit::Refuse {
                        return self.refuse(voice, MixerEvent::Limited);
                    }
                    // Only instances of no higher priority may give way, as with the
                    // mixer's bound; without enough of them, the start is refused.
                    let priority = voice.priority;
                    for _ in 0..playing + 1 - limit.max as usize {
                        let voices = &self.voices;
                        let unmarked = || {
                            voices.iter().enumerate().filter_map(|(i, v)| {
                                v.as_deref()
                                    .filter(|v| same(v) && !v.marked && v.priority <= priority)
                                    .map(|v| (i, v))
                            })
                        };
                        let pick = match limit.policy {
                            OverLimit::StopQuietest => unmarked().min_by(|(_, a), (_, b)| {
                                a.level(frame)
                                    .total_cmp(&b.level(frame))
                                    .then(a.order.cmp(&b.order))
                            }),
                            _ => unmarked().min_by_key(|(_, v)| v.order),
                        }
                        .map(|(i, _)| i);
                        match pick.and_then(|i| self.voices[i].as_mut()) {
                            Some(v) => v.marked = true,
                            None => {
                                for v in self.voices.iter_mut().flatten() {
                                    v.marked = false;
                                }
                                return self.refuse(voice, MixerEvent::Limited);
                            }
                        }
                    }
                }
            }
        }
        // The mixer's bound on voices, counting the marked voices as already gone.
        let others = || {
            self.voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.as_ref().is_some_and(|v| v.live() && !v.marked))
        };
        let mut global_victim = None;
        if others().count() >= self.max_voices {
            let victim = others()
                .filter_map(|(i, v)| v.as_deref().map(|v| (i, v)))
                .min_by_key(|(_, v)| (v.priority, v.order));
            match victim {
                Some((i, v)) if v.priority <= voice.priority => global_victim = Some(i),
                _ => {
                    for v in self.voices.iter_mut().flatten() {
                        v.marked = false;
                    }
                    return self.refuse(voice, MixerEvent::Rejected);
                }
            }
        }
        if let Some(v) = global_victim.and_then(|i| self.voices[i].as_mut()) {
            v.marked = true;
        }
        // Admitted: stop the voices it replaces and record its start.
        for slot in 0..self.voices.len() {
            if self.voices[slot].as_ref().is_some_and(|v| v.marked) {
                self.stop_slot(slot, frame, self.steal_fade, FadeCurve::Linear);
                if let Some(v) = self.voices[slot].as_mut() {
                    v.marked = false;
                    let id = v.id;
                    self.event(MixerEvent::Stolen(id));
                }
            }
        }
        if let (Some(entry), Some(limit)) = (start_entry, voice.instances) {
            self.starts[entry] = (limit.key, Some(frame));
        }
        voice.order = self.order;
        self.order += 1;
        voice.start(frame);
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
                let position = old.source_position();
                self.status.publish(
                    slot,
                    id,
                    VoiceStatus {
                        state: VoiceState::Finished,
                        position,
                    },
                );
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
        let master_paused = self.buses[0].paused;
        for v in self.voices.iter_mut().flatten() {
            // A voice paused by itself, by its bus or by the master bus keeps still.
            if v.paused() || self.buses[v.bus].paused || master_paused {
                // A stopping voice that becomes frozen cannot finish its fade: end it,
                // as a stop that arrives during a pause does.
                if v.state == State::Stopping {
                    v.state = State::Finished;
                } else {
                    v.freeze(frames as u64);
                }
                continue;
            }
            let buffer = &mut self.buses[v.bus].buffer[..frames * ch];
            v.render(first, frames, buffer, ch);
        }
        self.retire_done();
        let end = first + frames as u64;
        if self.buses.iter().any(|bus| bus.effects.is_some()) {
            self.mix_effects(first, out);
        } else {
            let (master, master_pause) = (self.buses[0].gain, self.buses[0].pause);
            for (b, bus) in self.buses.iter().enumerate() {
                let constant = !bus.gain.changing(first)
                    && !bus.pause.changing(first)
                    && !master.changing(first)
                    && !master_pause.changing(first);
                let gain_at = |frame| {
                    let pause = if b == 0 {
                        master_pause.value(frame)
                    } else {
                        bus.pause.value(frame) * master_pause.value(frame)
                    };
                    bus_gain(b, &bus.gain, &master, frame) * pause
                };
                let fixed = gain_at(first);
                for j in 0..frames {
                    let g = if constant {
                        fixed
                    } else {
                        gain_at(first + j as u64)
                    };
                    for c in 0..ch {
                        out[j * ch + c] += (bus.buffer[j * ch + c] as f64 * g) as f32;
                    }
                }
            }
        }
        // Segments end where pause fades do, so a bus's voices freeze from here.
        for bus in &mut self.buses {
            if bus.pausing && bus.pause.end() <= end {
                bus.pausing = false;
                bus.paused = true;
            }
        }
    }

    /// Publishes every voice's state for [`MixerHandle::voice_status`].
    fn publish(&self) {
        let master_paused = self.buses[0].paused;
        for (slot, voice) in self.voices.iter().enumerate() {
            if let Some(v) = voice {
                let state = if v.state == State::Stopping {
                    VoiceState::Stopping
                } else if v.paused() || self.buses[v.bus].paused || master_paused {
                    VoiceState::Paused
                } else {
                    VoiceState::Playing
                };
                let position = v.source_position();
                self.status
                    .publish(slot, v.id, VoiceStatus { state, position });
            }
        }
    }

    fn retire_done(&mut self) {
        for slot in 0..self.voices.len() {
            if self.voices[slot].as_ref().is_some_and(|v| v.done()) {
                let voice = self.voices[slot].take().expect("slot is occupied");
                let position = voice.source_position();
                self.status.publish(
                    slot,
                    voice.id,
                    VoiceStatus {
                        state: VoiceState::Finished,
                        position,
                    },
                );
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
                #[cfg(feature = "music")]
                music: None,
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
                #[cfg(feature = "music")]
                music: None,
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
                    #[cfg(feature = "music")]
                    music: None,
                },
            );
            match self.returns.push(item) {
                Ok(()) => sent += 1,
                Err(PushError::Full(item)) => {
                    self.retired[sent] = item;
                    break;
                }
                Err(PushError::Disconnected(_)) => sent += 1,
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
    /// A voice's state and position as of the mixer's last render, read without
    /// blocking. `None` if the mixer has not started the voice yet, or if it ended and
    /// its slot went to another voice; [`MixerHandle::poll_event`] reports endings.
    pub fn voice_status(&self, voice: VoiceId) -> Option<VoiceStatus> {
        self.status.read(voice)
    }
    /// Pauses a voice, fading it out first. It keeps its position, fades and loop.
    pub fn pause(&mut self, voice: VoiceId, fade: Fade) -> Result<()> {
        self.now().pause(voice, fade)
    }
    /// Resumes a paused voice, fading it in.
    pub fn resume(&mut self, voice: VoiceId, fade: Fade) -> Result<()> {
        self.now().resume(voice, fade)
    }
    /// Pauses every voice routed through a bus, and only those; [`BusId::MASTER`]
    /// pauses every voice. Voices started on a paused bus wait for it to resume.
    pub fn pause_bus(&mut self, bus: BusId, fade: Fade) -> Result<()> {
        self.now().pause_bus(bus, fade)
    }
    pub fn resume_bus(&mut self, bus: BusId, fade: Fade) -> Result<()> {
        self.now().resume_bus(bus, fade)
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
    /// Set legacy pan. A point emitter uses its direction instead.
    pub fn set_pan(&mut self, voice: VoiceId, pan: f32, fade: Fade) -> Result<()> {
        self.now().set_pan(voice, pan, fade)
    }
    pub fn set_rate(&mut self, voice: VoiceId, rate: f32, fade: Fade) -> Result<()> {
        self.now().set_rate(voice, rate, fade)
    }
    pub fn set_pitch(&mut self, voice: VoiceId, semitones: f32, fade: Fade) -> Result<()> {
        self.now().set_pitch(voice, semitones, fade)
    }
    /// Moves a voice to `position` relative to the listener, ramping its pan and
    /// distance gain over the fade so a moving sound does not zipper. A fade of about
    /// one game frame suits positions updated every frame.
    /// A 3D emitter ignores 2D pan/position commands; update it with `set_emitter`.
    pub fn set_position(&mut self, voice: VoiceId, position: [f32; 2], fade: Fade) -> Result<()> {
        self.now().set_position(voice, position, fade)
    }
    /// Enable/update permanent point-source mode. Stereo collapses immediately to
    /// (L+R)/2; fade ramps direction, attenuation and Doppler, not that conversion.
    pub fn set_emitter(&mut self, voice: VoiceId, emitter: Emitter, fade: Fade) -> Result<()> {
        self.now().set_emitter(voice, emitter, fade)
    }
    /// Schedule a global listener update; placement is recomputed on application.
    pub fn set_listener(&mut self, listener: Listener, fade: Fade) -> Result<()> {
        self.now().set_listener(listener, fade)
    }
    /// Output frames by which the master limiter's look-ahead delays the sound: a
    /// change scheduled at frame `f` is heard at `f + latency()`. 0 without a limiter.
    pub fn latency(&self) -> u64 {
        self.latency
    }
    /// The lowest gain the master limiter applied during the last render, from 0 to 1
    /// (1 when it did not reduce), for meters. `None` without a limiter.
    pub fn limiter_gain(&self) -> Option<f32> {
        self.limited
            .then(|| f32::from_bits(self.limiter_gain.load(Ordering::Relaxed)))
    }
    /// The 2D space that positions use.
    pub fn space(&self) -> Space2d {
        self.space
    }
    fn place(&self, position: [f32; 2]) -> Result<(f64, f64)> {
        if !position.iter().all(|c| c.is_finite()) {
            return Err(Error::invalid("A position must be finite"));
        }
        let (pan, gain) = self.space.place(position);
        Ok((pan as f64, gain as f64))
    }
    /// Restarts the generator of variations from `seed`.
    pub fn reseed(&mut self, seed: u64) {
        self.random = SplitMix(seed);
    }
    pub fn set_bus_gain(&mut self, bus: BusId, gain: f32, fade: Fade) -> Result<()> {
        self.now().set_bus_gain(bus, gain, fade)
    }
    /// Returns the next event, freeing finished voices and retired effect chains
    /// on this thread. Voice and effect queues are independent; their combined
    /// polling order is not a chronological ordering of both event kinds.
    pub fn poll_event(&mut self) -> Option<MixerEvent> {
        if let Some(event) = self.effects.poll() {
            return Some(event);
        }
        while let Some(returned) = self.returns.pop() {
            drop(returned.voice);
            #[cfg(feature = "music")]
            drop(returned.music);
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
        match self.commands.push(command) {
            Ok(()) => Ok(()),
            Err(PushError::Full(_)) => Err(Error::limit("The mixer's command queue is full")),
            Err(PushError::Disconnected(_)) => Err(Error::invalid("The mixer was dropped")),
        }
    }
}

/// Commands applied at one moment: see [`MixerHandle::now`] and [`MixerHandle::at`].
pub struct Commands<'a> {
    handle: &'a mut MixerHandle,
    frame: Option<u64>,
}
impl MixerHandle {
    /// Build a voice on the control thread, before publishing any command.
    fn prepare_voice(&mut self, sound: Sound, play: &Play) -> Result<Box<Voice>> {
        let handle = self;
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
        if let Some(emitter) = play.emitter {
            emitter.validate()?;
            if play.position.is_some() {
                return Err(Error::invalid("Choose 2D position or a 3D emitter"));
            }
        }
        check_rate(play.rate)?;
        check_pitch(play.pitch)?;
        let Variation {
            pitch: vary_pitch,
            gain_db: vary_gain,
        } = play.variation;
        if !(0.0..=24.0).contains(&vary_pitch) || !(0.0..=24.0).contains(&vary_gain) {
            return Err(Error::invalid("Variation ranges must be 0..=24"));
        }
        if let Loop::Region { start, end } = play.looping {
            if start >= end || frames.is_some_and(|n| end > n) {
                return Err(Error::invalid(
                    "A loop region needs start < end <= the sound's frames",
                ));
            }
        }
        let fade_in = handle.frames(play.fade_in)?;
        let start = start_frame(play.start, rate, frames)?;
        let step = (((rate as u128) << 32) + handle.rate as u128 / 2) / handle.rate as u128;
        let step = step as u64;
        let table = (step != ONE).then(|| handle.table(rate));
        let base_ratio = rate as f64 / handle.rate as f64;
        let mut gain = play.gain as f64;
        let mut pitch = play.pitch as f64;
        if vary_pitch > 0.0 {
            pitch += handle.random.symmetric() * vary_pitch as f64;
        }
        if vary_gain > 0.0 {
            let db = handle.random.symmetric() * vary_gain as f64;
            gain *= crate::math::exp2(db * std::f64::consts::LOG2_10 / 20.0);
        }
        let id = VoiceId(handle.next_voice);
        handle.next_voice += 1;
        let feed = match sound {
            Sound::Memory(audio) => Feed::Memory(audio),
            Sound::Stream(stream) => Feed::Stream(stream),
        };
        let bank = handle.bank.clone();
        let resampling = Resampling {
            step,
            base_ratio,
            table,
            bank,
        };
        let mut voice = Box::new(Voice::new(id, feed, channels, play.looping, resampling));
        voice.bus = play.bus.0;
        voice.priority = play.priority;
        voice.gain = Ramp::constant(gain);
        voice.rate = Ramp::constant(play.rate as f64);
        voice.pitch = Ramp::constant(pitch);
        voice.pan = Ramp::constant(play.pan.clamp(-1.0, 1.0) as f64);
        voice.emitter = play.emitter;
        if let Some(position) = play.position {
            let (pan, distance) = handle.place(position)?;
            voice.pan = Ramp::constant(pan);
            voice.distance = Ramp::constant(distance);
        }
        voice.fade_in = fade_in;
        voice.set_start(start)?;
        if let Some(limit) = play.instances {
            if !limit.min_gap.is_finite() || !(0.0..=MAX_GAP).contains(&limit.min_gap) {
                return Err(Error::invalid("An instance gap must be 0..=60 seconds"));
            }
            voice.instances = Some(voice::InstanceLimit {
                key: limit.key,
                max: limit.max,
                min_gap: (limit.min_gap * handle.rate as f64).round() as u64,
                policy: limit.policy,
            });
        }
        Ok(voice)
    }
}

impl Commands<'_> {
    /// Starts a voice and returns its id. Fails if the sound has more than two
    /// channels, a loop region is outside it, or the command queue is full.
    pub fn play(self, sound: impl Into<Sound>, play: &Play) -> Result<VoiceId> {
        let voice = self.handle.prepare_voice(sound.into(), play)?;
        let id = voice.id;
        self.handle.send(self.frame, Action::Play(voice))?;
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
    /// Changes a voice's playback rate, which changes speed and pitch together, over
    /// the fade. 1/16..=8.
    pub fn set_rate(self, voice: VoiceId, rate: f32, fade: Fade) -> Result<()> {
        check_rate(rate)?;
        let (len, curve) = self.handle.frames(fade)?;
        self.handle
            .send(self.frame, Action::Rate(voice, rate as f64, len, curve))
    }
    /// Changes a voice's pitch offset in semitones (fractions are cents) over the fade.
    /// -48..=48.
    pub fn set_pitch(self, voice: VoiceId, semitones: f32, fade: Fade) -> Result<()> {
        check_pitch(semitones)?;
        let (len, curve) = self.handle.frames(fade)?;
        self.handle.send(
            self.frame,
            Action::Pitch(voice, semitones as f64, len, curve),
        )
    }
    pub fn set_position(self, voice: VoiceId, position: [f32; 2], fade: Fade) -> Result<()> {
        let (pan, gain) = self.handle.place(position)?;
        let (len, curve) = self.handle.frames(fade)?;
        self.handle
            .send(self.frame, Action::Position(voice, pan, gain, len, curve))
    }
    pub fn pause(self, voice: VoiceId, fade: Fade) -> Result<()> {
        let (len, curve) = self.handle.frames(fade)?;
        self.handle
            .send(self.frame, Action::Pause(voice, len, curve))
    }
    /// Enable/update permanent point-source mode, using the listener at application
    /// time. Stereo collapses immediately; fade only ramps spatial parameters.
    pub fn set_emitter(self, voice: VoiceId, emitter: Emitter, fade: Fade) -> Result<()> {
        emitter.validate()?;
        let (len, curve) = self.handle.frames(fade)?;
        self.handle
            .send(self.frame, Action::Emitter(voice, emitter, len, curve))
    }
    /// Update every spatial voice, including paused voices and fading tails.
    pub fn set_listener(self, listener: Listener, fade: Fade) -> Result<()> {
        listener.validate()?;
        let (len, curve) = self.handle.frames(fade)?;
        self.handle
            .send(self.frame, Action::Listener(listener, len, curve))
    }
    pub fn resume(self, voice: VoiceId, fade: Fade) -> Result<()> {
        let (len, curve) = self.handle.frames(fade)?;
        self.handle
            .send(self.frame, Action::Resume(voice, len, curve))
    }
    pub fn pause_bus(self, bus: BusId, fade: Fade) -> Result<()> {
        if bus.0 > self.handle.buses.len() {
            return Err(Error::invalid("Unknown bus"));
        }
        let (len, curve) = self.handle.frames(fade)?;
        self.handle
            .send(self.frame, Action::PauseBus(bus.0, len, curve))
    }
    pub fn resume_bus(self, bus: BusId, fade: Fade) -> Result<()> {
        if bus.0 > self.handle.buses.len() {
            return Err(Error::invalid("Unknown bus"));
        }
        let (len, curve) = self.handle.frames(fade)?;
        self.handle
            .send(self.frame, Action::ResumeBus(bus.0, len, curve))
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

fn check_rate(rate: f32) -> Result<()> {
    if (1.0 / 16.0..=8.0).contains(&rate) {
        Ok(())
    } else {
        Err(Error::invalid("Playback rate must be 1/16..=8"))
    }
}
fn check_pitch(semitones: f32) -> Result<()> {
    if (-48.0..=48.0).contains(&semitones) {
        Ok(())
    } else {
        Err(Error::invalid("Pitch must be -48..=48 semitones"))
    }
}

/// The source frame where a voice starts.
fn start_frame(start: TimePosition, rate: u32, frames: Option<u64>) -> Result<u64> {
    let position = match start {
        TimePosition::Frames(n) => n as f64,
        TimePosition::Seconds(seconds) => seconds * rate as f64,
    };
    if !position.is_finite() {
        return Err(Error::invalid("The start must be finite"));
    }
    let position = position.round();
    let frame = if position < 0.0 {
        let total = frames.ok_or_else(|| {
            Error::invalid("A start counted from the end needs a sound of known length")
        })?;
        (total as f64 + position).max(0.0)
    } else {
        position
    };
    if frames.is_some_and(|total| frame > total as f64) {
        return Err(Error::invalid("The start is past the end of the sound"));
    }
    Ok(frame as u64)
}
