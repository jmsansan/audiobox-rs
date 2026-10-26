//! An 8-bit style synthesizer that renders text scores. Requires the `synth` feature.
//!
//! A [`Score`] holds up to eight channels. Legacy channels play one note at a time with an
//! [`Instrument`]: a pulse, triangle, sawtooth or noise [`Waveform`], an ADSR
//! [`Envelope`], a volume, and slide, vibrato, arpeggio and duty-sweep effects.
//! Render it offline with [`Score::render`], or chunk by chunk with [`Synth`].
//! [`Instrument::drum`] selects finite synthesized percussion in two [`DrumKit`]s.
//! [`Instrument::synthwave`] adds bounded polyphony, unison, stereo spread, resonant
//! filters, LFOs and glide. [`Synth::new_stereo`] / [`Score::render_stereo`] preserve
//! stereo; existing constructors remain mono and legacy instruments retain their bits.
//!
//! For prepared fractional/changing tempo, [`TempoMap`] converts absolute ticks
//! once with checked rational nearest-frame rounding. [`FrameScore`] / [`FrameNote`]
//! use those positions through [`Synth::from_frames`] or its stereo counterpart.
//! This opt-in path keeps the Score syntax and its integer-tempo PCM unchanged.
//! Beat LFOs integrate the scheduled tempo clock without phase resets; tick macros
//! follow that clock relative to the note's start. Repeats/sections are expanded
//! by the caller, and a tie is one extended note. Multiple bounded sources align
//! through the existing mixer; no unrestricted Score channel expansion is needed.
//! [`Synth::prepare_controls`] adds opt-in, bounded source-frame value ramps on
//! supported parameters while retaining held-note DSP state. [`ControlParameter`]
//! defines capabilities/units; [`ControlTarget`] selects track/instrument or note.
//! Structural edits still belong in prepared instruments at note boundaries.
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
//! | `drum` | `kick`, `snare`, `closed-hat`, `open-hat`, `tom`, `clap`, `crash` | none |
//! | `kit` | `chiptune` or `electronic`; requires `drum` | `electronic` |
//! | `drum-pitch` | Base Hz at C4, 20..=20000 | preset |
//! | `drum-tone` / `drum-noise` | Independent finite envelope lengths, 0..=10000 ms | preset |
//! | `drum-sweep` | Initial cents above the base (0..=4800), sweep milliseconds (0..=1000) | preset |
//! | `drum-mix` | Noise percentage, 0..=100; tone uses the complement | preset |
//! | `drum-click` | Separate 3 ms noise transient level, 0..=100 percent | preset |
//! | `drum-seed` | Fixed noise seed, 0..=4294967295 | `1` |
//!
//! Drums allow `volume`, `kit` and `drum-*` keys; melodic waveform, ADSR and modulation
//! keys are rejected. Notes transpose pitched components relative to C4. Chiptune
//! kick/tom/snare use triangle tones and clocked 15-bit noise; electronic tones use
//! sines and filtered noise, with a six-oscillator metal bank for hats/cymbals.
//! These are synthetic characters, not emulations of a particular console.
//!
//! ## Tracker instruments (opt-in)
//!
//! [`Instrument::chip`] selects a [`ChipInstrument`]. It is `None` by default, so
//! old scores retain their sample bits. All new keys opt in, even `quantize=none`.
//! New note triggers reset chip source phase, noise seed, PCM position and macro
//! age. Ties continue them; a note carried across `L` restarts them on the next lap.
//! Source clocks advance at zero gain. Gate/ADSR still apply to every chip source.
//!
//! | Key | Value | Default |
//! | --- | --- | --- |
//! | `wavetable` | 2..=256 hex nibbles, one unsigned four-bit value each | none |
//! | `sample` | 1..=65536 hex byte pairs, one unsigned PCM value each | none |
//! | `sample-bits` / `sample-rate` | PCM depth 2..=8 / original rate 100..=32000 Hz | `4` / `8000` |
//! | `sample-root` / `sample-loop` | PCM root MIDI note 0..=127 / `on` or `off` | `60` / `off` |
//! | `noise-period` | NTSC noise period-table index 0..=15; requires `wave=noise` | none |
//! | `macro-volume` | 1..=64 comma-separated values, 0..=15 | none |
//! | `macro-duty` | 0..=3 selects 12.5/25/50/75%; pulse only | none |
//! | `macro-arpeggio` | Relative offsets -48..=48 semitones | none |
//! | `macro-pitch` | Relative offsets -1200..=1200 cents | none |
//! | `macro-LANE-step` | `frames,COUNT,HZ` (both 1..=1000) or `ticks,COUNT` (1..=960) | `frames,1,60` |
//! | `macro-LANE-loop` | Zero-based start index, or `hold` | `hold` |
//! | `quantize` | `none`, `volume`, `ntsc-pulse`, `ntsc-triangle` | `none` |
//!
//! Macro lane names are `volume`, `duty`, `arpeggio` and `pitch`. Each lane has its
//! own interval. Index zero starts at the note's first sample. Frame clocks are
//! explicit simulation clocks, independent of callbacks; tick clocks follow score
//! tempo/TPQ. Steps occur at the first sample at or after the exact rational time,
//! with no cumulative rounding; note-relative tick steps can be one sample later
//! than globally floor-rounded score note positions. A looping lane plays its
//! prefix once, then repeats from its loop index. A held lane keeps its last value
//! through release. Legacy `arpeggio`/duty sweeps cannot duplicate the same lane.
//!
//! Wavetables map 0..15 to -1..+1 and loop once per note period; PCM maps its bit
//! range to -1..+1 and advances at `sample-rate * note-Hz / root-Hz`. PCM requires
//! one byte pair per value even at four bits (`000f`, not `0f`); values exceeding
//! the selected depth are errors. Non-looping PCM stops early at its data end and
//! clears the envelope. Whole-sample loops continue until gate/release ends. These
//! sources use sample-and-hold and 32.32 phase steps, with no anti-aliasing. Wave
//! frequency is limited to Nyquist; PCM playback ratios preserve relative pitch.
//! Extreme PCM increments saturate (one-shot) or wrap (looping) without overflow.
//! Custom sources are mutually exclusive; PCM/table sources reject wave/duty/noise
//! keys. Sample controls without data and macro controls without values are errors.
//!
//! Period noise bypasses note pitch and the legacy frequency*93 noise clock. It uses
//! [`NTSC_CLOCK`] / [`NTSC_NOISE_PERIODS`], with the same seeded 15-bit long (32767)
//! or short (93) register. Pitch modulation is rejected for fixed noise. In the
//! band-limited mode register output is averaged within each audio sample.
//! `quantize=volume` rounds the combined ADSR, instrument/channel volume, velocity
//! and volume-macro gain to one of 16 levels before the master gain. NTSC models
//! additionally round the final pitch to an 11-bit timer: pulse divider 16 and timer
//! 8..=2047, triangle divider 32 and timer 0..=2047. Frequencies outside these ranges
//! clamp to the nearest timer. Each model requires the matching waveform. These
//! are period models, not cycle-accurate hardware or a model of hardware mute rules.
//! See [NESdev noise](https://www.nesdev.org/wiki/APU_Noise),
//! [pulse](https://www.nesdev.org/wiki/APU_Pulse) and
//! [triangle](https://www.nesdev.org/wiki/APU_Triangle) for the hardware tables.
//!
//! ### Per-note wavetable voices (opt-in)
//!
//! A wavetable instrument normally plays every note of its channel on one voice,
//! like tracker hardware: a new note restarts the table from its first value at
//! whatever level the envelope had reached, and replaces the previous note's
//! release. Adding `voices=N` (1..=16) gives the channel a pool of `N` voices for
//! that instrument instead, using the synthwave allocator described below:
//!
//! - Each note starts on a free voice: the table from its first value (phase zero)
//!   and the ADSR from silence, at the note's own velocity. The previous note keeps
//!   its voice and plays its release independently, to its end.
//! - Without a free voice, the oldest releasing voice is taken, then the oldest
//!   held one. Its note is cut and the new note attacks from silence, which can
//!   click.
//! - Note-off, gates, ties, velocity, macros, quantization and loop carries apply
//!   per voice as on the single voice. Chords (`{c e g}`) are allowed up to `N`
//!   notes. Switching to an instrument without voices releases the held pooled
//!   notes and lets their tails finish.
//! - A note that starts while nothing else sounds renders exactly as on the
//!   single voice, so only overlapping notes change.
//!
//! Only wavetable sources take `voices`, and no other synthwave setting:
//! other synthwave keys, the waveform source with macros, PCM and period noise
//! are rejected with a diagnostic. In [`Instrument`] terms, set the wavetable
//! [`ChipInstrument`] and `synthwave: Some(Synthwave { voices, ..Default::default() })`.
//! This is per-note articulation for keyboard-like parts, not a model of an
//! acoustic instrument. Without `voices`, rendering is unchanged.
//!
//! ```
//! use audiobox::synth::Score;
//! let score: Score = "
//! instrument table wavetable=0123456789abcdefedcba98765432100 macro-arpeggio=0,7,12 macro-arpeggio-step=ticks,6 macro-arpeggio-loop=0
//! instrument pulse wave=pulse quantize=ntsc-pulse macro-duty=0,1,2,3 macro-duty-loop=0
//! instrument noise wave=noise noise=short noise-period=6 macro-volume=15,8,2,0
//! instrument pcm sample=00040a0f080300 sample-rate=4000 sample-loop=on sample-bits=4
//! channel a=@table o5 c4
//! channel b=@pulse o3 c4
//! channel c=@noise c4
//! channel d=@pcm o3 c4
//! ".parse()?;
//! let audio = score.render(48_000)?;
//! # Ok::<(), audiobox::Error>(())
//! ```
//!
//! ## Synthwave instruments (opt-in)
//!
//! Any synthwave key, or `synthwave=on`, enables [`Synthwave`]. Sources are saw,
//! pulse/square and triangle, always band-limited, independently of legacy aliasing.
//! Use `unison` (1..=8), `detune` (0..=100 cents), `spread` (0..=100 percent),
//! `pan` (-100..=100 percent), `voices` (1..=16, default 8), `glide` (0..=60000 ms)
//! and `legato=on|off`. Glide/legato require `voices=1`.
//!
//! `filter=lowpass|highpass|bandpass` selects a per-note [`VoiceFilter`]; `cutoff`
//! is Hz, `resonance` is Q*100, `keytrack` is percent relative to C4, `filter-adsr`
//! is a separate ADSR, and `filter-env` its peak cutoff offset in cents. See the
//! type's fields for bounds/defaults. Modulated cutoff clamps to 1..=0.45*sample-rate
//! Hz. LP/HP slopes are 12 dB/octave; band-pass peak is normalized to 1. Resonance
//! may exceed full scale; the amplitude ADSR follows the filter to bound its tail.
//!
//! `lfo-pitch`, `lfo-cutoff`, `lfo-amplitude` and `lfo-pan` accept `millihertz,depth`
//! or `beats,numerator,denominator,depth`. [`Lfo`] documents units/bounds. These sine
//! LFOs follow total output frames (including loops) and reset only with the synth.
//! Chip/drum sources cannot combine with synthwave, except that a wavetable takes
//! `voices` alone (see per-note wavetable voices above). Each note normalizes its unison
//! oscillators by count and uses fixed initial phases and linear stereo panning.
//!
//! Chords use `{c e g}4!90`, sharing length/gate/velocity. Only notes, accidentals
//! and local `>`/`<` shifts are allowed inside braces. Members must be distinct,
//! at most 16 and within the instrument's voice limit; each counts against the
//! score's note bound. `^` extends the entire group; `&` joins single notes only.
//! The typed equivalent is [`ChannelWriter::chord`].
//!
//! Idle slots are reused first, then oldest releasing, then oldest held voices,
//! ties by slot index. Stealing is immediate and can click. Lowering the limit on an
//! instrument change retires higher slots immediately. Legacy/synthwave switches
//! release the other path's held notes. Chord carries retrigger every held member
//! at the loop point. Mono legato preserves phase, filter state and both envelopes
//! across adjacent full-gate notes of the same instrument. Gaps/short gates retrigger;
//! glide moves between active-note pitches in cents over its prepared duration.
//!
//! ```
//! use audiobox::synth::Score;
//! let score = Score::parse("tempo 110
//! instrument bass wave=saw voices=1 glide=80 legato=on filter=lowpass cutoff=500
//! instrument pad wave=saw unison=5 detune=18 spread=90 adsr=120,150,70,300
//! instrument lead voices=1 glide=60 legato=on lfo-pitch=5000,12
//! channel bass=@bass o2 l8 [c c >c< g]2
//! channel pad=@pad {c e g}1
//! channel lead=@lead o5 l8 [c e g >c<]2")?;
//! let stereo = score.render_stereo(48_000)?;
//! assert_eq!(stereo.channels(), 2);
//! # Ok::<(), audiobox::Error>(())
//! ```
//!
//! ## Commands
//!
//! Commands are written one after another; spaces between them are optional.
//!
//! | Command | Meaning |
//! | --- | --- |
//! | `c d e f g a b` | A note. Follow with `+` or `#` for sharp, `-` for flat, then an optional length. |
//! | `c16!96` | Per-note velocity 0..=127, independently of `v`. Omitted velocity is 127. Suffix follows the length/dots. |
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
//! Velocity is not persistent: `c!64 d` gives velocities 64 and 127. A tied
//! continuation cannot set velocity; it retains the first attack's velocity.
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
//! Drums are one-shots with finite squared decay and a 1 ms attack (the 3 ms click
//! has a one-sample attack). Snare tone and noise have separate envelopes; a clap
//! has bursts at 0, 10 and 20 ms and a final noise tail at 30 ms. Drum gates/ties
//! change score timing but do not stop the natural tail. A new note on that channel
//! replaces the previous sound, so a closed hat cuts an open one on the same channel.
//! Use separate channels for overlapping parts. Each trigger resets phase, filter
//! memory and the configured seed, giving identical hits at equal pitch/velocity.
//! Drums are not restarted by a loop carry without an actual written onset.
//! Their final tails are included in [`Synth::max_frames`]. A one-frame envelope
//! can be silent at low rates; presets use longer times. Metallic components may
//! alias and are not subject to the melodic oscillator's band-limiting guarantee.
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
mod chip;
mod controls;
pub use chip::{
    ChipInstrument, ChipSample, ChipSource, MAX_MACRO_VALUES, MAX_SAMPLE_VALUES, Macro, MacroStep,
    NTSC_CLOCK, NTSC_NOISE_PERIODS, PitchTable, Wavetable, noise_clock,
};
pub use controls::{
    ControlCurve, ControlEvent, ControlId, ControlParameter, ControlReport, ControlStatus,
    ControlTarget, MAX_SYNTH_CONTROLS,
};
mod drum;
pub use drum::{Drum, DrumKind, DrumKit};
mod parse;
mod prepared;
pub use prepared::{FrameNote, FrameScore, MAX_TEMPO_CHANGES, TempoChange, TempoMap};
mod render;
mod score;
mod synthwave;
mod wave;
pub use synthwave::{FilterMode, Lfo, LfoRate, Synthwave, VoiceFilter};

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
