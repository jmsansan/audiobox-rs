//! An 8-bit style synthesizer that renders text scores. Requires the `synth` feature.
//!
//! A [`Score`] holds up to eight channels. Each plays one note at a time with an
//! [`Instrument`]: a pulse, triangle, sawtooth or noise [`Waveform`], an ADSR
//! [`Envelope`], a volume, and slide, vibrato, arpeggio and duty-sweep effects.
//! Render it offline with [`Score::render`], or chunk by chunk with [`Synth`].
//!
//! ```
//! use audiobox::synth::Score;
//! let score: Score = "
//!     tempo 150
//!     instrument lead wave=pulse duty=25 adsr=5,60,70,40 vibrato=12,160,200
//!     instrument bass wave=triangle
//!     channel melody = @lead o5 l8 L [c e g >c< g e]2 c2
//!     channel bass   = @bass o3 l2 c g c1
//! ".parse()?;
//! let audio = score.render(44_100)?;
//! assert_eq!(audio.channels(), 1);
//! # Ok::<(), audiobox::Error>(())
//! ```
//!
//! # Score format
//!
//! A score is text, one directive per line. `;` starts a comment that runs to the
//! end of the line. Blank lines are ignored.
//!
//! | Directive | Meaning |
//! | --- | --- |
//! | `tempo 150` | Quarter notes per minute, 1..=999. Default 120. Once per score. |
//! | `ticks 48` | Time base: ticks per quarter note, 1..=960. Default 48. Once per score. |
//! | `instrument NAME key=value ...` | Defines an instrument. Keys are listed below. |
//! | `pattern NAME = ...` | Defines a named pattern of commands. |
//! | `channel NAME = ...` | Appends commands to a channel. Channels are created in order of first appearance. |
//!
//! Names use letters, digits, `_` and `-`. Repeating `pattern NAME =` or
//! `channel NAME =` continues the same pattern or channel, so long parts can
//! span several lines; a repeat may also open on one line and close on another.
//!
//! ## Instrument keys
//!
//! | Key | Value | Default |
//! | --- | --- | --- |
//! | `wave` | `pulse`, `triangle`, `saw` or `noise` | `pulse` |
//! | `duty` | `12.5`, `25`, `50` or `75`; a comma list starts a duty sweep from the first | `50` |
//! | `duty-step` | Milliseconds between duty-sweep steps; the last duty holds | `16` |
//! | `noise` | `long` (hiss) or `short` (metallic) | `long` |
//! | `adsr` | `attack,decay,sustain,release`: milliseconds, milliseconds, percent, milliseconds | `0,0,100,0` |
//! | `volume` | 0..=15 | `15` |
//! | `slide` | Pitch slide from each note's start, in cents per second (signed) | `0` |
//! | `vibrato` | `depth,period[,delay]`: cents (≤ 1200), milliseconds, milliseconds | none |
//! | `arpeggio` | Up to 16 semitone offsets, cycled from each note's start, such as `0,4,7` | none |
//! | `arpeggio-step` | Milliseconds per arpeggio step | `16` |
//!
//! ## Commands
//!
//! Commands are written one after another; spaces between them are optional.
//!
//! | Command | Meaning |
//! | --- | --- |
//! | `c d e f g a b` | A note. Follow with `+` or `#` for sharp, `-` for flat, then an optional length. |
//! | `r` | A rest, with an optional length. |
//! | length | `4` is a quarter, `8` an eighth, `3` a half-note triplet; dots add half: `4.`, `8..`. `%12` is 12 ticks. Without a number, the default length is used: `c.` dots it. |
//! | `^8` | Lengthens the previous note or rest. `c4^8` lasts a quarter and an eighth. |
//! | `&` | Ties two notes of the same pitch: `c4&c8`. |
//! | `o4` | Sets the octave, 0..=9. `o4 c` is middle C, `o4 a` is 440 Hz. Default 4. |
//! | `>` `<` | One octave up, one octave down. |
//! | `l8` | Default length, possibly dotted. Default `l4`. |
//! | `v12` | Channel volume, 0..=15. Default 15. |
//! | `q6` | Gate: the following notes sound for 6/8 of their length, then release. 1..=8, default 8. |
//! | `@NAME` | Selects an instrument. A channel needs one before its first note. |
//! | `[ ... ]3` | Repeats 3 times (default 2). In `[c d \| e]3`, the part after `\|` is skipped on the last time. |
//! | `$NAME` | Plays a pattern. Its octave, length, volume and instrument changes carry on after it. |
//! | `L` | The song's loop point. Channels that mark one must mark the same tick. |
//!
//! A length must be a whole number of ticks: at the default 48 ticks per quarter,
//! `64.` is rejected. Raise `ticks` for finer lengths.
//!
//! The song's length is that of its longest channel, including trailing rests. When
//! it ends, every channel continues from the loop point, or from the start if there
//! is none. A note that sounds across the loop point restarts there.
//!
//! ## Errors and limits
//!
//! [`Score::parse`] reports a [`ParseError`] with the line, the column and an
//! [`ErrorCode`](crate::ErrorCode): `Syntax` for malformed text, `InvalidArgument` for
//! values out of range, and `LimitExceeded` when the text, the nesting of repeats and
//! patterns, the number of channels, notes or expanded commands, or the song's duration
//! exceed its [`ScoreLimits`].
//!
//! # Sound
//!
//! Notes are equal-tempered with A4 at 440 Hz. Each channel's output is its oscillator
//! times its envelope, its instrument volume / 15 and its channel volume / 15. The
//! channels are summed and scaled by [`RenderOptions::gain`]. Oscillator phase
//! continues across notes, and each attack starts from the channel's current level,
//! so note changes do not click unless the attack is 0 ms.
//!
//! The noise channel's 15-bit shift register is clocked at 93 times the note's
//! frequency, so in the short mode, whose sequence repeats every 93 steps, it sounds
//! at the note's pitch. In the long mode, higher notes give brighter hiss.
//!
//! ## Aliasing
//!
//! [`Aliasing::BandLimited`], the default, corrects the pulse and sawtooth steps with
//! PolyBLEP and the triangle's corners with PolyBLAMP, and averages noise over each
//! sample. This removes most aliasing, though not all at very high notes.
//! [`Aliasing::Naive`] samples the raw waveforms for a harsher, period sound.
//!
//! ## Determinism
//!
//! The same score, sample rate and options give the same samples on every platform.
//! Pitch, vibrato and envelopes use only addition, subtraction, multiplication,
//! division and rounding, which IEEE 754 defines exactly; `sin`, `exp` and `powf` are
//! replaced by the crate's own series. Times in milliseconds are rounded to whole
//! samples, and note positions are computed from ticks with integer arithmetic.
mod parse;
mod render;
mod score;
mod wave;

pub use parse::ParseError;
pub use render::{RenderOptions, Synth};
pub use score::{
    Arpeggio, Channel, ChannelWriter, DutySweep, Envelope, Instrument, Length, MAX_CHANNELS,
    MAX_NESTING, Note, Pitch, Score, ScoreLimits, Vibrato,
};
pub use wave::{Aliasing, Duty, Lfsr, NoiseMode, Oscillator, Waveform};

impl Score {
    /// Parses score text with the default [`ScoreLimits`].
    pub fn parse(text: &str) -> Result<Score, ParseError> {
        parse::parse(text, &ScoreLimits::default())
    }
    pub fn parse_with_limits(text: &str, limits: &ScoreLimits) -> Result<Score, ParseError> {
        parse::parse(text, limits)
    }
}
impl std::str::FromStr for Score {
    type Err = crate::Error;
    fn from_str(text: &str) -> crate::Result<Self> {
        Ok(Self::parse(text)?)
    }
}
