//! Explicit vocal events, musical notes and their preparation into frames.
use std::ops::Range;
use std::sync::Arc;

use super::{VoiceMode, VoiceOptions, VoiceProfile};
use crate::{Audio, Error, Result};

/// One of five vowel-like resonance targets, given as formant pole frequencies at
/// `resonance = 1` (see [`VoiceProfile`] for how bandwidth moves the peaks). They
/// are synthetic colours, not the vowels of a particular language.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Vowel {
    /// Open: F1 800, F2 1200 Hz.
    #[default]
    A,
    /// Mid front: F1 500, F2 1900 Hz.
    E,
    /// Close front: F1 300, F2 2250 Hz.
    I,
    /// Mid back: F1 500, F2 900 Hz.
    O,
    /// Close back: F1 330, F2 750 Hz.
    U,
}
impl Vowel {
    /// Every vowel, in order.
    pub const ALL: [Vowel; 5] = [Self::A, Self::E, Self::I, Self::O, Self::U];
    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

/// The small inventory of consonant-like gestures.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Articulation {
    /// Pitched sound through the vowel's resonances, with the profile's breath.
    #[default]
    Voiced,
    /// Aspiration noise through the vowel's resonances, without pitch: like "h".
    Breath,
    /// Bright noise at the profile's fricative frequency: like "s" or "sh".
    Fricative,
    /// Silence: a stop closure, a gap or a rest.
    Closure,
}

/// One gesture of an explicit plan.
///
/// `hz` is the fundamental at the start and end of the gesture, gliding
/// geometrically (evenly in cents) between them. It is used by
/// [`Articulation::Voiced`] gestures and must then be finite, 20..=4000 Hz and,
/// with the profile's vibrato and jitter, below 0.45 of the sample rate. Other
/// gestures hold the current pitch and ignore `hz`. `level` is 0..=1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VocalEvent {
    /// Duration in microseconds. A gesture shorter than half a frame rounds to
    /// nothing and is skipped.
    pub duration_us: u32,
    pub articulation: Articulation,
    /// The resonance target. Non-voiced gestures glide toward it too, which colours
    /// a breath and prepares the next vowel.
    pub vowel: Vowel,
    pub hz: [f64; 2],
    pub level: f64,
}
impl VocalEvent {
    /// A voiced vowel at a steady pitch, at full level.
    pub fn vowel(vowel: Vowel, duration_us: u32, hz: f64) -> Self {
        Self {
            duration_us,
            articulation: Articulation::Voiced,
            vowel,
            hz: [hz, hz],
            level: 1.0,
        }
    }
    /// Silence: a closure. Within an explicit plan or a text plan, closures fade
    /// at the profile's `attack` rate (articulation); only the gaps of a note list
    /// and the final tail fade at the `release` rate.
    pub fn rest(duration_us: u32) -> Self {
        Self {
            duration_us,
            articulation: Articulation::Closure,
            vowel: Vowel::A,
            hz: [0.0, 0.0],
            level: 0.0,
        }
    }
    /// A non-voiced gesture: `Breath`, `Fricative` or `Closure`.
    pub fn gesture(articulation: Articulation, vowel: Vowel, duration_us: u32) -> Self {
        Self {
            duration_us,
            articulation,
            vowel,
            hz: [0.0, 0.0],
            level: if articulation == Articulation::Closure {
                0.0
            } else {
                1.0
            },
        }
    }
    /// The same gesture gliding to `hz` by its end.
    pub fn glide_to(mut self, hz: f64) -> Self {
        self.hz[1] = hz;
        self
    }
    /// The same gesture at `level`.
    pub fn with_level(mut self, level: f64) -> Self {
        self.level = level;
        self
    }
}

/// What part of the source text a marker covers.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MarkerKind {
    /// A phrase, from its first letter (or opening `¿`/`¡`) to its closing
    /// punctuation.
    Phrase,
    /// A word.
    Word,
    /// One planned syllable: a group of letters.
    Syllable,
}

/// A span of the source text and the plan events that sound it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Marker {
    pub kind: MarkerKind,
    /// UTF-8 byte range in the text, on character boundaries.
    pub bytes: Range<usize>,
    /// Index range of the plan's events.
    pub events: Range<usize>,
}

/// A marker with its position in the rendered source, in output frames from the
/// utterance's start. These are source frames, never device presentation times:
/// follow a mixer voice's source position to time animation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedMarker {
    pub kind: MarkerKind,
    pub bytes: Range<usize>,
    /// Frames from the first frame of the marker's first event to the end of its
    /// last event.
    pub frames: Range<u64>,
}

/// An ordered list of vocal events, with optional text markers.
///
/// Build one from explicit events with [`UtterancePlan::new`], or from dialogue
/// with [`plan_gibberish`](super::plan_gibberish). Validation happens in
/// [`PreparedUtterance::new`], which knows the sample rate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UtterancePlan {
    events: Vec<VocalEvent>,
    markers: Vec<Marker>,
    text: Option<Arc<str>>,
}
impl UtterancePlan {
    /// A plan of explicit events, without text or markers.
    pub fn new(events: Vec<VocalEvent>) -> Self {
        Self {
            events,
            markers: Vec::new(),
            text: None,
        }
    }
    pub(crate) fn with_text(events: Vec<VocalEvent>, markers: Vec<Marker>, text: Arc<str>) -> Self {
        Self {
            events,
            markers,
            text: Some(text),
        }
    }
    pub fn events(&self) -> &[VocalEvent] {
        &self.events
    }
    /// Markers sorted by byte start, then by kind (phrase, word, syllable).
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }
    /// The dialogue text the plan was made from.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
    /// The sum of the event durations, in microseconds.
    pub fn duration_us(&self) -> u64 {
        self.events.iter().map(|e| e.duration_us as u64).sum()
    }
}

/// A consonant-like attack for a sung note.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Onset {
    /// The vowel starts directly.
    #[default]
    None,
    /// A breath before the vowel.
    Breath,
    /// A fricative before the vowel.
    Fricative,
    /// A closure then a short fricative burst: a stop.
    Plosive,
}

/// An explicitly timed note for one monophonic voice. Studio and other callers
/// own bars, tempo and arrangement; this is the rendering primitive.
///
/// Notes of one voice must be in order and must not overlap. Gaps become rests
/// that release at the profile's `release` rate; the tail after the last note is
/// added beyond its end.
///
/// * A note that starts exactly where the previous one ends re-articulates:
///   its first 12 ms (at most a quarter of the note) is a closure, unless it has
///   an onset, which separates the notes instead.
/// * A `legato` note that starts exactly where the previous one ends keeps the
///   envelope and source phase, ignores its onset, and glides from the previous
///   note's end pitch over the profile's `glide` time (at most half the note).
///   A legato note after a gap, or as the first note, is an ordinary note.
/// * An onset takes the first 35 ms of a note, at most a third of it; a plosive
///   spends 60% of that in closure and 40% in a burst.
/// * Notes shorter than the attack never reach full level; nothing is
///   lengthened. The source phase never restarts within an utterance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VocalNote {
    /// First frame of the note.
    pub start: u64,
    /// Length in frames until note-off. A zero-length note is skipped.
    pub frames: u64,
    /// Pitch at the start of the note, after any legato glide.
    pub hz: f64,
    /// Pitch at note-off; equal to `hz` for a steady note.
    pub end_hz: f64,
    /// Linear gain 0..=1.
    pub velocity: f64,
    pub vowel: Vowel,
    pub onset: Onset,
    pub legato: bool,
}
impl VocalNote {
    /// A steady note at full velocity.
    pub fn new(start: u64, frames: u64, hz: f64, vowel: Vowel) -> Self {
        Self {
            start,
            frames,
            hz,
            end_hz: hz,
            velocity: 1.0,
            vowel,
            onset: Onset::None,
            legato: false,
        }
    }
    pub fn with_velocity(mut self, velocity: f64) -> Self {
        self.velocity = velocity;
        self
    }
    pub fn with_onset(mut self, onset: Onset) -> Self {
        self.onset = onset;
        self
    }
    /// Glides to `hz` by note-off.
    pub fn glide_to(mut self, hz: f64) -> Self {
        self.end_hz = hz;
        self
    }
    pub fn legato(mut self) -> Self {
        self.legato = true;
        self
    }
}

/// A gesture at absolute frames, the form the renderer reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Segment {
    pub start: u64,
    pub end: u64,
    pub articulation: Articulation,
    pub vowel: Vowel,
    pub hz: [f64; 2],
    pub level: f64,
    /// Falls toward this segment's level use the release rate.
    pub release: bool,
}

/// Formant frequencies (Hz) of each vowel, F1..F4, at resonance 1.
pub(crate) const FORMANT_HZ: [[f64; 4]; 5] = [
    [800.0, 1200.0, 2500.0, 3500.0],
    [500.0, 1900.0, 2550.0, 3500.0],
    [300.0, 2250.0, 2950.0, 3600.0],
    [500.0, 900.0, 2450.0, 3400.0],
    [330.0, 750.0, 2350.0, 3300.0],
];
/// Formant bandwidths (Hz) at bandwidth 1 and resonance 1.
pub(crate) const FORMANT_BW: [f64; 4] = [70.0, 90.0, 140.0, 200.0];
/// Highest formant centre allowed, as a fraction of the sample rate.
pub(crate) const FORMANT_LIMIT: f64 = 0.42;
/// Highest allowed pitch, as a fraction of the sample rate.
const PITCH_LIMIT: f64 = 0.45;
/// Ring-out after the release, in seconds.
pub(crate) const RINGOUT: f64 = 0.04;

/// An utterance validated and timed for one sample rate and channel count. It is
/// immutable and cheap to clone: its plan, markers and text are shared.
#[derive(Clone, Debug)]
pub struct PreparedUtterance {
    pub(crate) profile: VoiceProfile,
    pub(crate) rate: u32,
    pub(crate) channels: usize,
    pub(crate) seed: u64,
    pub(crate) segments: Arc<[Segment]>,
    pub(crate) markers: Arc<[PreparedMarker]>,
    pub(crate) text: Option<Arc<str>>,
    pub(crate) body: u64,
    pub(crate) total: u64,
    /// Which of F1..F4 fit below [`FORMANT_LIMIT`] for every vowel.
    pub(crate) formants: [bool; 4],
    pub(crate) initial_hz: f64,
}

fn check_hz(hz: f64, top: f64, what: &str) -> Result<()> {
    if !(hz.is_finite() && (20.0..=4000.0).contains(&hz)) {
        return Err(Error::invalid(format!(
            "{what} pitch must be 20..=4000 Hz, not {hz}"
        )));
    }
    if hz * top >= PITCH_LIMIT {
        return Err(Error::invalid(format!(
            "{what} pitch {hz} Hz with the profile's modulation exceeds 0.45 of the sample rate"
        )));
    }
    Ok(())
}

impl PreparedUtterance {
    /// Validates `profile`, `plan` and `options` and times the plan in frames.
    ///
    /// Errors: [`ErrorCode::InvalidArgument`](crate::ErrorCode::InvalidArgument)
    /// for an out-of-range profile field, sample rate, channel count, pitch or
    /// level; [`ErrorCode::LimitExceeded`](crate::ErrorCode::LimitExceeded) when the
    /// plan has more events or markers than [`VoiceOptions::limits`] allow, or
    /// would last longer than `max_seconds` with its tail.
    pub fn new(
        profile: &VoiceProfile,
        plan: &UtterancePlan,
        options: &VoiceOptions,
    ) -> Result<Self> {
        options.check()?;
        profile.validate(options.sample_rate)?;
        let limits = options.limits;
        if plan.events.len() > limits.max_events {
            return Err(Error::limit(format!(
                "A voice plan holds at most {} events",
                limits.max_events
            )));
        }
        if plan.markers.len() > limits.max_markers {
            return Err(Error::limit(format!(
                "A voice plan holds at most {} markers",
                limits.max_markers
            )));
        }
        if let Some(text) = &plan.text {
            if text.len() > limits.max_text_bytes {
                return Err(Error::limit("Voice plan text exceeds the text limit"));
            }
        }
        let rate = options.sample_rate;
        let top = crate::math::exp2(profile.modulation_cents() / 1200.0) / rate as f64;
        for event in &plan.events {
            if !(event.level.is_finite() && (0.0..=1.0).contains(&event.level)) {
                return Err(Error::invalid(format!(
                    "Vocal event level must be 0..=1, not {}",
                    event.level
                )));
            }
            if event.articulation == Articulation::Voiced {
                check_hz(event.hz[0], top, "Vocal event")?;
                check_hz(event.hz[1], top, "Vocal event")?;
            }
        }
        let total_us = plan.duration_us();
        let frame_of =
            |us: u64| -> u64 { ((us as u128 * rate as u128 + 500_000) / 1_000_000) as u64 };
        check_length(profile, rate, frame_of(total_us), limits.max_seconds)?;
        let mut segments = Vec::with_capacity(plan.events.len());
        let mut bounds = Vec::with_capacity(plan.events.len());
        let mut elapsed = 0u64;
        for event in &plan.events {
            let start = frame_of(elapsed);
            elapsed += event.duration_us as u64;
            let end = frame_of(elapsed);
            bounds.push((start, end));
            if end > start {
                segments.push(Segment {
                    start,
                    end,
                    articulation: event.articulation,
                    vowel: event.vowel,
                    hz: event.hz,
                    level: event.level,
                    release: false,
                });
            }
        }
        let mut markers = Vec::with_capacity(plan.markers.len());
        let text_len = plan.text.as_ref().map_or(0, |t| t.len());
        for marker in &plan.markers {
            let ok = marker.events.start < marker.events.end
                && marker.events.end <= plan.events.len()
                && marker.bytes.start <= marker.bytes.end
                && marker.bytes.end <= text_len;
            if !ok {
                return Err(Error::invalid("Voice marker is out of range"));
            }
            markers.push(PreparedMarker {
                kind: marker.kind,
                bytes: marker.bytes.clone(),
                frames: bounds[marker.events.start].0..bounds[marker.events.end - 1].1,
            });
        }
        Self::finish(profile, options, segments, markers, plan.text.clone())
    }

    /// Prepares the notes of one monophonic voice; see [`VocalNote`] for the rules.
    /// Harmony is several prepared voices, each with its own seed, mixed by the
    /// caller or played as separate mixer voices; they stay sample-aligned
    /// because every note is placed at an exact frame.
    ///
    /// Errors as [`PreparedUtterance::new`], plus
    /// [`ErrorCode::InvalidArgument`](crate::ErrorCode::InvalidArgument) for notes
    /// out of order or overlapping and a velocity outside 0..=1.
    pub fn from_notes(
        profile: &VoiceProfile,
        notes: &[VocalNote],
        options: &VoiceOptions,
    ) -> Result<Self> {
        options.check()?;
        profile.validate(options.sample_rate)?;
        let limits = options.limits;
        if notes.len() > limits.max_events {
            return Err(Error::limit(format!(
                "A voice holds at most {} notes",
                limits.max_events
            )));
        }
        let rate = options.sample_rate;
        let top = crate::math::exp2(profile.modulation_cents() / 1200.0) / rate as f64;
        let mut cursor = 0u64;
        for note in notes {
            if !(note.velocity.is_finite() && (0.0..=1.0).contains(&note.velocity)) {
                return Err(Error::invalid(format!(
                    "Note velocity must be 0..=1, not {}",
                    note.velocity
                )));
            }
            check_hz(note.hz, top, "Note")?;
            check_hz(note.end_hz, top, "Note")?;
            if note.frames == 0 {
                // Skipped: it neither orders nor lengthens the voice.
                continue;
            }
            if note.start < cursor {
                return Err(Error::invalid(
                    "Vocal notes must be in order and must not overlap",
                ));
            }
            cursor = note
                .start
                .checked_add(note.frames)
                .ok_or_else(|| Error::limit("Note end exceeds u64"))?;
        }
        check_length(profile, rate, cursor, limits.max_seconds)?;
        let ms = |ms: f64| (ms * rate as f64 / 1000.0 + 0.5).floor() as u64;
        let (restart, onset_len, glide) = (ms(12.0), ms(35.0), ms(profile.glide));
        let mut segments: Vec<Segment> = Vec::with_capacity(notes.len() * 3 + 1);
        let mut end = 0u64;
        let mut previous: Option<&VocalNote> = None;
        for note in notes.iter().filter(|n| n.frames > 0) {
            let push = |segments: &mut Vec<Segment>,
                        from: u64,
                        to: u64,
                        articulation,
                        hz: [f64; 2],
                        level,
                        release| {
                if to > from {
                    segments.push(Segment {
                        start: from,
                        end: to,
                        articulation,
                        vowel: note.vowel,
                        hz,
                        level,
                        release,
                    });
                }
            };
            if note.start > end {
                // A rest: release toward silence. The vowel prepares the next note.
                push(
                    &mut segments,
                    end,
                    note.start,
                    Articulation::Closure,
                    [note.hz; 2],
                    0.0,
                    true,
                );
            }
            let adjacent = previous.is_some() && note.start == end;
            let note_end = note.start + note.frames;
            let mut at = note.start;
            let level = note.velocity;
            if adjacent && note.legato {
                let from_hz = previous.map_or(note.hz, |p| p.end_hz);
                let g = glide.min(note.frames / 2);
                if g > 0 {
                    push(
                        &mut segments,
                        at,
                        at + g,
                        Articulation::Voiced,
                        [from_hz, note.hz],
                        level,
                        false,
                    );
                    at += g;
                }
            } else if note.onset != Onset::None {
                let o = onset_len.min(note.frames / 3);
                match note.onset {
                    Onset::Plosive => {
                        let closed = (o * 3 + 2) / 5;
                        push(
                            &mut segments,
                            at,
                            at + closed,
                            Articulation::Closure,
                            [note.hz; 2],
                            0.0,
                            false,
                        );
                        push(
                            &mut segments,
                            at + closed,
                            at + o,
                            Articulation::Fricative,
                            [note.hz; 2],
                            level,
                            false,
                        );
                    }
                    Onset::Breath => push(
                        &mut segments,
                        at,
                        at + o,
                        Articulation::Breath,
                        [note.hz; 2],
                        level,
                        false,
                    ),
                    Onset::Fricative => push(
                        &mut segments,
                        at,
                        at + o,
                        Articulation::Fricative,
                        [note.hz; 2],
                        level,
                        false,
                    ),
                    Onset::None => {}
                }
                at += o;
            } else if adjacent {
                let r = restart.min(note.frames / 4);
                push(
                    &mut segments,
                    at,
                    at + r,
                    Articulation::Closure,
                    [note.hz; 2],
                    0.0,
                    false,
                );
                at += r;
            }
            // The rest of the note glides from `hz` to `end_hz`; a glide segment
            // already reached `hz`.
            push(
                &mut segments,
                at,
                note_end,
                Articulation::Voiced,
                [note.hz, note.end_hz],
                level,
                false,
            );
            end = note_end;
            previous = Some(note);
        }
        Self::finish(profile, options, segments, Vec::new(), None)
    }

    fn finish(
        profile: &VoiceProfile,
        options: &VoiceOptions,
        segments: Vec<Segment>,
        markers: Vec<PreparedMarker>,
        text: Option<Arc<str>>,
    ) -> Result<Self> {
        let rate = options.sample_rate;
        let body = segments.last().map_or(0, |s| s.end);
        let total = if body == 0 {
            0
        } else {
            body + tail_frames(profile, rate)
        };
        let limit = rate as f64 * FORMANT_LIMIT;
        let mut formants = [true; 4];
        for (k, enabled) in formants.iter_mut().enumerate() {
            *enabled = FORMANT_HZ.iter().all(|f| f[k] * profile.resonance <= limit);
        }
        if matches!(profile.mode, VoiceMode::Robot(_)) {
            formants = [false; 4];
        }
        let initial_hz = segments
            .iter()
            .find(|s| s.articulation == Articulation::Voiced)
            .map_or(profile.pitch, |s| s.hz[0]);
        Ok(Self {
            profile: *profile,
            rate,
            channels: options.channels,
            seed: options.seed,
            segments: segments.into(),
            markers: markers.into(),
            text,
            body,
            total,
            formants,
            initial_hz,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    pub fn seed(&self) -> u64 {
        self.seed
    }
    pub fn profile(&self) -> &VoiceProfile {
        &self.profile
    }
    /// The full length in frames, release tail included; 0 for an empty plan.
    pub fn frames(&self) -> u64 {
        self.total
    }
    /// The frames up to the end of the last event, without the tail.
    pub fn body_frames(&self) -> u64 {
        self.body
    }
    /// Markers in the order of the plan's markers, with frame positions.
    pub fn markers(&self) -> &[PreparedMarker] {
        &self.markers
    }
    /// The innermost marker of `kind` that contains source frame `frame`, if any.
    /// Allocation-free; safe to call on an audio thread.
    pub fn marker_at(&self, kind: MarkerKind, frame: u64) -> Option<&PreparedMarker> {
        let upto = self.markers.partition_point(|m| m.frames.start <= frame);
        self.markers[..upto]
            .iter()
            .rev()
            .find(|m| m.kind == kind && m.frames.contains(&frame))
    }
    /// The dialogue text, if the plan came from text.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
    /// Which of the four formants are active at this rate and resonance: a formant
    /// whose centre would exceed 0.42 of the sample rate for any vowel is left
    /// out for the whole utterance.
    pub fn active_formants(&self) -> [bool; 4] {
        self.formants
    }
    /// A renderer at the start of the utterance.
    pub fn synth(&self) -> super::VoiceSynth {
        super::VoiceSynth::new(self)
    }
    /// Renders the whole utterance, tail included.
    pub fn render(&self) -> Result<Audio> {
        let frames = usize::try_from(self.total).map_err(|_| Error::limit("Utterance too long"))?;
        let mut interleaved = vec![0.0f32; frames * self.channels];
        self.synth().fill(&mut interleaved);
        let mut data = vec![Vec::with_capacity(frames); self.channels];
        for frame in interleaved.chunks_exact(self.channels) {
            for (channel, &s) in data.iter_mut().zip(frame) {
                channel.push(s);
            }
        }
        if frames == 0 {
            // An empty utterance is a zero-frame sound.
            return Audio::new(vec![Vec::new(); self.channels], self.rate);
        }
        Audio::new(data, self.rate)
    }
}

pub(crate) fn tail_frames(profile: &VoiceProfile, rate: u32) -> u64 {
    let release = (profile.release * rate as f64 / 1000.0).ceil() as u64;
    let ringout = (RINGOUT * rate as f64).ceil() as u64;
    release + ringout
}

fn check_length(profile: &VoiceProfile, rate: u32, body: u64, max_seconds: f64) -> Result<()> {
    // An empty utterance has no tail.
    if body == 0 {
        return Ok(());
    }
    let total = body as f64 + tail_frames(profile, rate) as f64;

    if total > max_seconds * rate as f64 {
        return Err(Error::limit(format!(
            "A voice utterance lasts at most {max_seconds} s with its tail"
        )));
    }
    Ok(())
}
