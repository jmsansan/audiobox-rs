//! Music that follows a game: transitions, cues and intensity layers. Requires `music`.
//!
//! A [`Music`] controller uses a fixed [`Transport`] on the mixer's output-frame
//! clock. Each operation is prepared on the control thread and sent as one command.
//! The audio thread resolves its timing when it receives the command, then applies
//! the whole operation at that frame. A full command queue changes no playing voice.
//! With no room for all new layers, a transition is rejected without stopping the
//! previous track. Poll [`MixerEvent::MusicApplied`] / [`MixerEvent::MusicRejected`]
//! to distinguish application from submission; application starts a fade, not ends it.
//!
//! Every layer starts together, including silent ones. Intensity only changes gains,
//! so a layer becomes audible at its current source position. Multilayer tracks use
//! the same source sample rate and, where known, length. The caller supplies stems
//! with matching musical timing and compatible loops; unknown-length streams and
//! synthesizers must also maintain that timing. A cue is independent of transitions
//! and is played once (supply a finite source).
//!
//! Music uses ordinary mixer voices at priority 255, without stealing on admission.
//! Provision `max_voices` for the entire crossfade overlap and any cues/effects.
//! Other priority-255 sounds can still steal music under the mixer's normal rules.
//! With this feature the mixer reserves return storage for up to 16 layer voices per
//! queued command, so its queue setting also controls this preparation-time memory.
//! Poll events regularly to reclaim request containers and finished voices off audio.
//! As with ordinary mixer voices, exhausting both return buffers without polling
//! can free retired data on audio; the reservation covers a full render's worst case,
//! not an unlimited number of renders without a consumer.
//!
//! The transport follows output frames, including while a bus is paused; it does not
//! infer tempo from audio or follow per-voice rate/pitch changes. A shared bus pause
//! freezes all its stems together but does not stop the beat grid. Limiter lookahead
//! delays the audible result by [`MixerHandle::latency`] frames. Submit ahead of the
//! desired boundary: a command delayed by a full pending queue is quantized when it
//! is finally received. Operations at the same boundary follow submission order.
//!
//! ```
//! use audiobox::music::{Music, TimeSignature, Timing, Track, Transport};
//! use audiobox::mixer::{BusId, Fade, Mixer, MixerConfig};
//! use audiobox::Audio;
//! use std::sync::Arc;
//! let (mut mixer, mut handle) = Mixer::new(MixerConfig::default())?;
//! let transport = Transport::new(48_000, 120.0, TimeSignature::default(), 0)?;
//! let music = Music::new(&mut handle, transport, BusId::MASTER)?;
//! let source = Arc::new(Audio::new(vec![vec![0.1; 48_000]], 48_000)?);
//! music.transition(&mut handle, Track::new(source), 0.0, Timing::NOW, Fade::NONE)?;
//! music.cue(&mut handle, Arc::new(Audio::new(vec![vec![0.1; 2400]], 48_000)?),
//!     0.5, Timing::BEAT)?;
//! # mixer.render(&mut [0.0; 256]);
//! # Ok::<(), audiobox::Error>(())
//! ```
use super::{
    Action, BusId, Command, Fade, Mixer, MixerEvent, MixerHandle, Play, Returned, Sound, Voice,
};
use crate::{Error, ErrorCode, FadeCurve, Limits, Result};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

/// Most stems in one track. Silent layers count toward the voice limit too.
pub const MAX_LAYERS: usize = 16;

/// A notated bar: `numerator` beats of `1 / denominator` whole notes.
/// The transport accepts 1..=32 beats and denominators 1, 2, 4, 8, 16 or 32.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeSignature {
    pub numerator: u8,
    pub denominator: u8,
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

/// A fixed-tempo grid in output frames. BPM counts quarter notes per minute.
/// Tempo is rounded to 0.001 BPM on construction; every boundary is computed
/// directly with integer arithmetic, rounded to the nearest frame (halves up).
#[derive(Clone, Copy, Debug)]
pub struct Transport {
    rate: u32,
    milli_bpm: u32,
    signature: TimeSignature,
    origin: u64,
}

impl Transport {
    /// `sample_rate` uses the crate's supported rate limits, BPM is finite 1..=1000,
    /// and `origin` is the first bar's first output frame.
    pub fn new(sample_rate: u32, bpm: f64, signature: TimeSignature, origin: u64) -> Result<Self> {
        Limits::default().check(0, 1, sample_rate)?;
        if !bpm.is_finite() || !(1.0..=1000.0).contains(&bpm) {
            return Err(Error::invalid("Music tempo must be 1..=1000 BPM"));
        }
        if !(1..=32).contains(&signature.numerator)
            || !signature.denominator.is_power_of_two()
            || signature.denominator > 32
        {
            return Err(Error::invalid(
                "Music needs 1..=32 beats and a power-of-two beat unit up to 32",
            ));
        }
        Ok(Self {
            rate: sample_rate,
            milli_bpm: (bpm * 1000.0).round() as u32,
            signature,
            origin,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
    pub fn bpm(&self) -> f64 {
        self.milli_bpm as f64 / 1000.0
    }
    pub fn signature(&self) -> TimeSignature {
        self.signature
    }
    pub fn origin(&self) -> u64 {
        self.origin
    }

    /// Immediate returns `frame`. Beat/Bar returns the first grid boundary strictly
    /// after `frame`, or `origin` when it is still in the future. An exact boundary
    /// therefore advances one more beat/bar. Overflow returns `LimitExceeded`.
    pub fn boundary_after(&self, frame: u64, quantize: Quantize) -> Result<u64> {
        self.boundary(frame, quantize)
            .ok_or_else(|| Error::limit("Music boundary exceeds the output clock"))
    }

    // The renderer uses this fallible arithmetic without constructing an Error/String.
    fn boundary(&self, frame: u64, quantize: Quantize) -> Option<u64> {
        if quantize == Quantize::Immediate {
            return Some(frame);
        }
        if frame < self.origin {
            return Some(self.origin);
        }
        let beats = if quantize == Quantize::Bar {
            self.signature.numerator as u128
        } else {
            1
        };
        let numerator = self.rate as u128 * 240_000 * beats;
        let denominator = self.milli_bpm as u128 * self.signature.denominator as u128;
        let elapsed = (frame - self.origin) as u128;
        let mut index = elapsed * denominator / numerator + 1;
        loop {
            let offset = (index * numerator + denominator / 2) / denominator;
            let boundary = u64::try_from(self.origin as u128 + offset).ok()?;
            if boundary > frame {
                return Some(boundary);
            }
            index += 1;
        }
    }
}

/// Where an operation lands on a transport. Beat means the signature's beat unit;
/// in 6/8 it is an eighth note, while BPM always counts quarter notes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Quantize {
    #[default]
    Immediate,
    Beat,
    Bar,
}

/// Resolve at audio-thread receipt, using `max(received_frame, not_before)`.
/// Beat/Bar are strictly after that reference, even if it is already a boundary.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timing {
    pub not_before: u64,
    pub quantize: Quantize,
}

impl Timing {
    pub const NOW: Self = Self {
        not_before: 0,
        quantize: Quantize::Immediate,
    };
    pub const BEAT: Self = Self {
        not_before: 0,
        quantize: Quantize::Beat,
    };
    pub const BAR: Self = Self {
        not_before: 0,
        quantize: Quantize::Bar,
    };
}

/// One stem of a track. It keeps playing even when its intensity gain is zero.
pub struct Layer {
    pub sound: Sound,
    /// Full-level linear gain, finite and nonnegative. Default 1.
    pub gain: f32,
    /// Intensity at which gain starts rising and reaches full level, 0..=1 in order.
    /// Equal endpoints give a step (on at/above the threshold). Default [0, 0], always on.
    pub intensity: [f32; 2],
}

impl Layer {
    pub fn new(sound: impl Into<Sound>) -> Self {
        Self {
            sound: sound.into(),
            gain: 1.0,
            intensity: [0.0, 0.0],
        }
    }
}

/// 1..=16 synchronized stems, with one loop policy. All source rates must match;
/// all known frame lengths must match. Unknown lengths/timing remain the caller's
/// responsibility. Single-stem tracks may use any supported source rate.
pub struct Track {
    pub layers: Vec<Layer>,
    pub looping: super::Loop,
}

impl Track {
    pub fn new(sound: impl Into<Sound>) -> Self {
        Self {
            layers: vec![Layer::new(sound)],
            looping: super::Loop::Off,
        }
    }
}

/// Identifies a submitted operation in mixer events, not a voice or a controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MusicId(u64);

/// A controller bound to one mixer and one fixed transport. It does not own sounds
/// or mutate voices on the control thread; future operations target whichever track
/// is active when they actually execute. Dropping it does not stop its track.
pub struct Music {
    owner: u64,
    clock: Arc<AtomicU64>,
    transport: Transport,
    bus: BusId,
}

impl Music {
    pub fn new(handle: &mut MixerHandle, transport: Transport, bus: BusId) -> Result<Self> {
        if transport.rate != handle.rate || bus.0 > handle.buses.len() {
            return Err(Error::invalid(
                "Music transport rate and bus must match its mixer",
            ));
        }
        let owner = handle.next_voice;
        handle.next_voice += 1;
        Ok(Self {
            owner,
            clock: handle.clock.clone(),
            transport,
            bus,
        })
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }

    /// Replace this controller's track. All new stems start at source frame zero,
    /// fading in while the previous track fades out over the same duration/curve.
    /// Preparation/queue errors change no voice; audio capacity failures are events.
    pub fn transition(
        &self,
        handle: &mut MixerHandle,
        track: Track,
        intensity: f32,
        timing: Timing,
        fade: Fade,
    ) -> Result<MusicId> {
        self.check(handle)?;
        validate_intensity(intensity)?;
        if track.layers.is_empty() || track.layers.len() > MAX_LAYERS {
            return Err(Error::invalid("A music track needs 1..=16 layers"));
        }
        let (len, curve) = handle.frames(fade)?;
        let mut rate = None;
        let mut length = None;
        let mut voices = Vec::with_capacity(track.layers.len());
        for layer in track.layers {
            let [low, high] = layer.intensity;
            validate_intensity(low)?;
            validate_intensity(high)?;
            if low > high || !layer.gain.is_finite() || layer.gain < 0.0 {
                return Err(Error::invalid(
                    "A layer needs ordered intensity bounds and finite nonnegative gain",
                ));
            }
            let (source_rate, frames) = shape(&layer.sound);
            if rate.is_some_and(|rate| rate != source_rate)
                || frames.zip(length).is_some_and(|(a, b)| a != b)
            {
                return Err(Error::invalid(
                    "Music layers must share a source rate and known length",
                ));
            }
            rate = Some(source_rate);
            if frames.is_some() {
                length = frames;
            }
            let state = LayerState {
                owner: self.owner,
                active: true,
                gain: layer.gain as f64,
                intensity: layer.intensity,
            };
            let mut voice = handle.prepare_voice(
                layer.sound,
                &Play {
                    bus: self.bus,
                    priority: 255,
                    gain: state.gain_at(intensity) as f32,
                    fade_in: fade,
                    looping: track.looping,
                    ..Play::default()
                },
            )?;
            voice.music = Some(state);
            voices.push(Some(voice));
        }
        self.submit(
            handle,
            timing,
            len,
            curve,
            Operation::Transition,
            intensity,
            voices,
        )
    }

    /// Fade the active track's layer gains without restarting or pausing any source.
    pub fn set_intensity(
        &self,
        handle: &mut MixerHandle,
        intensity: f32,
        timing: Timing,
        fade: Fade,
    ) -> Result<MusicId> {
        self.check(handle)?;
        validate_intensity(intensity)?;
        let (len, curve) = handle.frames(fade)?;
        self.submit(
            handle,
            timing,
            len,
            curve,
            Operation::Intensity,
            intensity,
            Vec::new(),
        )
    }

    /// Layer a one-shot cue over music. Track transitions/stops do not stop cues.
    pub fn cue(
        &self,
        handle: &mut MixerHandle,
        sound: impl Into<Sound>,
        gain: f32,
        timing: Timing,
    ) -> Result<MusicId> {
        self.check(handle)?;
        let voice = handle.prepare_voice(
            sound.into(),
            &Play {
                bus: self.bus,
                gain,
                priority: 255,
                ..Play::default()
            },
        )?;
        self.submit(
            handle,
            timing,
            0,
            FadeCurve::Linear,
            Operation::Cue,
            0.0,
            vec![Some(voice)],
        )
    }

    /// Stop all this controller's track voices, including earlier crossfade tails.
    /// Independently layered cues keep playing.
    pub fn stop(&self, handle: &mut MixerHandle, timing: Timing, fade: Fade) -> Result<MusicId> {
        self.check(handle)?;
        let (len, curve) = handle.frames(fade)?;
        self.submit(handle, timing, len, curve, Operation::Stop, 0.0, Vec::new())
    }

    fn check(&self, handle: &MixerHandle) -> Result<()> {
        if Arc::ptr_eq(&self.clock, &handle.clock) {
            Ok(())
        } else {
            Err(Error::invalid("Music controller belongs to another mixer"))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn submit(
        &self,
        handle: &mut MixerHandle,
        timing: Timing,
        len: u64,
        curve: FadeCurve,
        operation: Operation,
        intensity: f32,
        voices: Vec<Option<Box<Voice>>>,
    ) -> Result<MusicId> {
        // Validate the lower bound now; actual receipt can only move it later.
        self.transport
            .boundary_after(timing.not_before, timing.quantize)?
            .checked_add(len)
            .ok_or_else(|| Error::limit("Music fade exceeds the output clock"))?;
        let id = MusicId(handle.next_voice);
        handle.next_voice += 1;
        let request = Box::new(Request {
            id,
            owner: self.owner,
            transport: self.transport,
            timing,
            len,
            curve,
            operation,
            intensity,
            voices,
        });
        handle.send(None, Action::Music(request))?;
        Ok(id)
    }
}

fn validate_intensity(intensity: f32) -> Result<()> {
    if (0.0..=1.0).contains(&intensity) {
        Ok(())
    } else {
        Err(Error::invalid("Intensity must be finite and 0..=1"))
    }
}

fn shape(sound: &Sound) -> (u32, Option<u64>) {
    match sound {
        Sound::Memory(audio) => (audio.sample_rate(), Some(audio.frames() as u64)),
        Sound::Stream(stream) => (stream.sample_rate(), stream.frames()),
    }
}

#[derive(Clone, Copy)]
pub(super) struct LayerState {
    owner: u64,
    active: bool,
    gain: f64,
    intensity: [f32; 2],
}

impl LayerState {
    fn gain_at(self, value: f32) -> f64 {
        let [low, high] = self.intensity;
        let amount = if value >= high {
            1.0
        } else if value <= low {
            0.0
        } else {
            (value as f64 - low as f64) / (high as f64 - low as f64)
        };
        self.gain * amount
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Operation {
    Transition,
    Cue,
    Intensity,
    Stop,
}

pub(super) struct Request {
    id: MusicId,
    owner: u64,
    transport: Transport,
    timing: Timing,
    len: u64,
    curve: FadeCurve,
    operation: Operation,
    intensity: f32,
    voices: Vec<Option<Box<Voice>>>,
}

impl Mixer {
    pub(super) fn quantize_music(&mut self, mut command: Command) -> Option<Command> {
        if let Action::Music(request) = &command.action {
            let reference = self.frame.max(request.timing.not_before);
            let resolved = request
                .transport
                .boundary(reference, request.timing.quantize);
            match resolved.and_then(|frame| frame.checked_add(request.len).map(|_| frame)) {
                Some(frame) => command.frame = Some(frame),
                None => {
                    if let Action::Music(request) = command.action {
                        let id = request.id;
                        self.return_music(
                            request,
                            MixerEvent::MusicRejected(id, ErrorCode::LimitExceeded),
                        );
                    }
                    return None;
                }
            }
        }
        Some(command)
    }

    pub(super) fn apply_music(&mut self, frame: u64, mut request: Box<Request>) {
        self.retire_done();
        let replacing = matches!(request.operation, Operation::Transition | Operation::Stop);
        let targets = |layer: LayerState| {
            layer.owner == request.owner && (layer.active || request.operation == Operation::Stop)
        };
        let incoming = request.voices.len();
        let occupied = self.voices.iter().flatten().count();
        let removed = if replacing {
            self.voices
                .iter()
                .flatten()
                .filter(|v| {
                    v.music.is_some_and(targets)
                        && (request.len == 0
                            || v.paused()
                            || self.buses[v.bus].paused
                            || self.buses[0].paused)
                })
                .count()
        } else {
            0
        };
        if incoming > 0 && occupied - removed + incoming > self.max_voices {
            let id = request.id;
            return self.return_music(
                request,
                MixerEvent::MusicRejected(id, ErrorCode::LimitExceeded),
            );
        }
        if replacing {
            for slot in 0..self.voices.len() {
                if self.voices[slot]
                    .as_ref()
                    .is_some_and(|v| v.music.is_some_and(targets))
                {
                    self.voices[slot]
                        .as_mut()
                        .unwrap()
                        .music
                        .as_mut()
                        .unwrap()
                        .active = false;
                    self.stop_slot(slot, frame, request.len, request.curve);
                }
            }
            self.retire_done();
        }
        if request.operation == Operation::Intensity {
            for voice in self.voices.iter_mut().flatten() {
                if let Some(layer) = voice.music.filter(|m| m.owner == request.owner && m.active) {
                    voice.gain.retarget(
                        voice.time(frame),
                        layer.gain_at(request.intensity),
                        request.len,
                        request.curve,
                    );
                }
            }
        }
        for voice in &mut request.voices {
            if let Some(voice) = voice.take() {
                self.start(frame, voice);
            }
        }
        let id = request.id;
        self.return_music(request, MixerEvent::MusicApplied(id, frame));
    }

    fn return_music(&mut self, request: Box<Request>, event: MixerEvent) {
        if self.retired.len() < self.retired.capacity() {
            self.retired.push(Returned {
                event: Some(event),
                voice: None,
                music: Some(request),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_boundary_that_overflows_only_after_receipt_rejects_the_operation() {
        let (mut mixer, mut handle) = Mixer::new(super::super::MixerConfig::default()).unwrap();
        let transport = Transport::new(48_000, 120.0, TimeSignature::default(), 0).unwrap();
        let music = Music::new(&mut handle, transport, BusId::MASTER).unwrap();
        let request = music.stop(&mut handle, Timing::BEAT, Fade::NONE).unwrap();
        mixer.frame = u64::MAX;
        // Zero frames avoids advancing the exhausted clock. The arithmetic path
        // returns only Option/ErrorCode, never constructs an allocating Error.
        mixer.render(&mut []);
        assert_eq!(
            handle.poll_event(),
            Some(MixerEvent::MusicRejected(request, ErrorCode::LimitExceeded))
        );
        assert!(mixer.pending.is_empty());
    }
}
