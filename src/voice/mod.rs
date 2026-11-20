//! Procedural vocal sounds: robot dialogue blips, vowel-like gibberish and pitched
//! vocal notes. Requires the `voice` feature, which enables `synth`.
//!
//! This is synthesis of *invented* syllables, not text-to-speech: nothing here
//! pronounces words, and the output is not meant to be intelligible. Dialogue text
//! only steers rhythm, stress, pauses and intonation.
//!
//! The work splits into three stages, so that the audio thread only renders:
//!
//! 1. **Describe.** A [`VoiceProfile`] is a character's timbre and default delivery.
//!    An [`UtterancePlan`] is a list of [`VocalEvent`]s (a vowel, a breath, a
//!    fricative or a closure, with a duration, pitch glide and level).
//!    [`plan_gibberish`] compiles dialogue text into such a plan with source-span
//!    [`Marker`]s; music supplies [`VocalNote`]s at explicit frames instead.
//! 2. **Prepare.** [`PreparedUtterance::new`] or [`PreparedUtterance::from_notes`]
//!    validates everything against a sample rate and the [`VoiceLimits`], rounds
//!    every time to frames once, and stores an immutable plan. This step may
//!    allocate and returns stable [`ErrorCode`](crate::ErrorCode)s.
//! 3. **Render.** [`PreparedUtterance::render`] gives an [`Audio`](crate::Audio);
//!    [`VoiceSynth`] renders the same samples into caller-owned buffers in chunks of
//!    any size, without allocating, locking, I/O or text parsing. With `mixer` it is
//!    a [`Stream`](crate::mixer::Stream).
//!
//! ```
//! use audiobox::voice::{GibberishOptions, PreparedUtterance, VoiceOptions, VoicePreset,
//!     VoiceProfile, plan_gibberish};
//! let profile = VoiceProfile::preset(VoicePreset::SoftRounded);
//! let plan = plan_gibberish("¿Qué tal, viajero? ¡Vamos!", &profile, &GibberishOptions::default())?;
//! let voice = PreparedUtterance::new(&profile, &plan, &VoiceOptions::default())?;
//! let audio = voice.render()?;
//! assert_eq!(audio.frames() as u64, voice.frames());
//! // Word, syllable and phrase markers point back into the text.
//! let first = voice.markers()[0];
//! assert_eq!(&plan.text().unwrap()[first.bytes.clone()], "Qué tal, viajero?");
//! # Ok::<(), audiobox::Error>(())
//! ```
//!
//! # Contract
//!
//! * **Rates and channels.** 8000..=192000 Hz, mono or stereo. Stereo duplicates
//!   each mono frame into both channels.
//! * **Time.** [`VocalEvent`] durations are integer microseconds. Preparation sums
//!   them exactly and rounds each absolute boundary once to the nearest frame
//!   (halves up), so long plans never drift. [`VocalNote`]s are already in frames.
//! * **Tail.** After the last event the voice releases over the profile's
//!   `release` time, then rings out for 40 ms, the last 5 ms of which (at most 256
//!   frames) fade linearly to exact zero. [`PreparedUtterance::frames`] includes the
//!   tail; [`PreparedUtterance::body_frames`] does not.
//! * **Headroom.** Samples pass a fixed soft guard: magnitudes up to 0.9 are
//!   unchanged, larger ones bend smoothly toward 1.0, so output is always finite
//!   and within (-1, 1). [`VoiceSynth::peak_before_guard`] reports how close a
//!   render came.
//! * **Determinism.** The same profile, plan, options and seed give the same
//!   samples on every platform, including WebAssembly, independent of chunk sizes
//!   and of other voices. DSP uses only `+ - * /`, `floor`, `sqrt` and the crate's
//!   own series for `sin`, `cos`, `exp` and `log2`. Randomness (aspiration and
//!   frication noise, jitter, shimmer) comes from local streams derived from
//!   [`VoiceOptions::seed`]. The rendering, profile-format and planner algorithms
//!   carry the versions [`RENDER_VERSION`], [`PROFILE_FORMAT_VERSION`] and
//!   [`PLANNER_VERSION`]; a change to their output changes the version.
//! * **Prepared changes.** A prepared utterance is immutable. Gain, pan, rate and
//!   pitch changes during playback belong to the mixer; a different profile or
//!   plan means preparing again.
//! * **Empty input.** A plan without events prepares to a zero-frame utterance that
//!   renders nothing. Text with no letters plans no events.
//!
//! See `docs/voice.md` for units, ranges, the DSP design and measurements.
mod gibberish;
mod plan;
mod profile;
mod render;

pub use gibberish::{Delivery, GibberishOptions, plan_gibberish};
pub use plan::{
    Articulation, Marker, MarkerKind, Onset, PreparedMarker, PreparedUtterance, UtterancePlan,
    VocalEvent, VocalNote, Vowel,
};
pub use profile::{VoiceMode, VoicePreset, VoiceProfile, VoiceSource};
pub use render::VoiceSynth;

use crate::{Error, Result};

/// Version of the rendering algorithm. Changes whenever a prepared utterance would
/// render different samples.
pub const RENDER_VERSION: u32 = 1;
/// Version of the profile text format and of the preset recipes.
pub const PROFILE_FORMAT_VERSION: u32 = 1;
/// Version of the text-to-gibberish planner: inventory, prosody rules and its
/// random number streams.
pub const PLANNER_VERSION: u32 = 1;

/// Lowest supported output rate in Hz.
pub const MIN_SAMPLE_RATE: u32 = 8_000;
/// Highest supported output rate in Hz.
pub const MAX_SAMPLE_RATE: u32 = 192_000;
/// Hard ceiling on the events of one plan.
pub const MAX_EVENTS: usize = 4096;
/// Hard ceiling on the markers of one plan.
pub const MAX_MARKERS: usize = 16_384;
/// Hard ceiling on dialogue text, in UTF-8 bytes.
pub const MAX_TEXT_BYTES: usize = 16 * 1024;
/// Hard ceiling on a prepared utterance's length, tail included, in seconds.
pub const MAX_SECONDS: f64 = 120.0;

/// Size limits checked during planning and preparation, before allocating.
///
/// The defaults are the hard ceilings ([`MAX_EVENTS`], [`MAX_MARKERS`],
/// [`MAX_TEXT_BYTES`], [`MAX_SECONDS`]); a caller may lower them, never raise them.
/// Exceeding one is [`ErrorCode::LimitExceeded`](crate::ErrorCode::LimitExceeded).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceLimits {
    pub max_events: usize,
    pub max_markers: usize,
    pub max_text_bytes: usize,
    /// Body plus tail, in seconds.
    pub max_seconds: f64,
}
impl Default for VoiceLimits {
    fn default() -> Self {
        Self {
            max_events: MAX_EVENTS,
            max_markers: MAX_MARKERS,
            max_text_bytes: MAX_TEXT_BYTES,
            max_seconds: MAX_SECONDS,
        }
    }
}
impl VoiceLimits {
    pub(crate) fn check(&self) -> Result<()> {
        if self.max_events > MAX_EVENTS
            || self.max_markers > MAX_MARKERS
            || self.max_text_bytes > MAX_TEXT_BYTES
            || !(self.max_seconds.is_finite() && (0.0..=MAX_SECONDS).contains(&self.max_seconds))
        {
            return Err(Error::invalid(
                "Voice limits may lower the hard ceilings, not raise them",
            ));
        }
        Ok(())
    }
}

/// How an utterance is prepared for output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceOptions {
    /// Output rate, 8000..=192000 Hz. Default 48000.
    pub sample_rate: u32,
    /// 1 or 2. Default 1.
    pub channels: usize,
    /// Root of the renderer's random streams. Default 0.
    pub seed: u64,
    pub limits: VoiceLimits,
}
impl Default for VoiceOptions {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 1,
            seed: 0,
            limits: VoiceLimits::default(),
        }
    }
}
impl VoiceOptions {
    pub(crate) fn check(&self) -> Result<()> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&self.sample_rate) {
            return Err(Error::invalid(format!(
                "Voice sample rate must be {MIN_SAMPLE_RATE}..={MAX_SAMPLE_RATE} Hz, not {}",
                self.sample_rate
            )));
        }
        if !(1..=2).contains(&self.channels) {
            return Err(Error::invalid("Voices render mono or stereo"));
        }
        self.limits.check()
    }
}

/// SplitMix64: one step of the generator used to derive independent streams.
pub(crate) fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// FNV-1a over bytes.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        hash = (hash ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// A small seeded generator: SplitMix64 as a stream.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rng(u64);
impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut x = self.0;
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^ (x >> 31)
    }
    /// Uniform in [0, 1), 53 bits.
    pub(crate) fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Uniform in [-1, 1).
    pub(crate) fn signed(&mut self) -> f64 {
        self.unit() * 2.0 - 1.0
    }
}
