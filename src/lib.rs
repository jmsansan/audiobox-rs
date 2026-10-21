//! Audio decoding, editing, analysis and encoding in safe Rust, with zero dependencies,
//! and the audio side of a game: a real-time mixer, an 8-bit synthesizer and
//! parametric sound effects.
//!
//! The crate uses only the standard library: no C libraries, FFmpeg, build scripts or
//! `unsafe`. It compiles to WebAssembly, and its synthesizer, effects and mixer give
//! the same samples on every platform.
//!
//! ```
//! use audiobox::{AudioFormat, NormalizeOptions, decode, encode};
//!
//! fn prepare(bytes: &[u8]) -> audiobox::Result<Vec<u8>> {
//!     let clip = decode(bytes)?
//!         .cut(1.5, 30.0)?
//!         .to_mono()
//!         .resample(16_000)?
//!         .normalize(NormalizeOptions::default())?;
//!     encode(&clip, AudioFormat::Flac)
//! }
//! # let wav = encode(&audiobox::Audio::new(vec![vec![0.1; 48_000 * 2]], 48_000)?, AudioFormat::Wav)?;
//! # prepare(&wav)?;
//! # Ok::<(), audiobox::Error>(())
//! ```
//!
//! # What is included
//!
//! | Area | Items | Feature |
//! | --- | --- | --- |
//! | Decoding and encoding | [`decode`], [`encode`], [`read_audio_file`], [`write_audio_file`] for WAV, AIFF, CAF, AU, FLAC and MP3 | always |
//! | Editing and DSP | [`Audio`]: cut, concat, mix, fades, filters, resampling, limiter, tempo and pitch | always |
//! | Analysis | [`Audio::loudness`], peaks, RMS, [`analyze::spectrum`], waveforms | always |
//! | Streaming | [`stream::Decoder`] for WAV, FLAC and MP3 a chunk at a time with seeking; raw PCM and resampling across chunks | always |
//! | Synthesizer | [`synth`]: text scores rendered in an 8-bit style | `synth` |
//! | Sound effects | [`sfx`]: parameters, text form and seeded presets | `sfx` |
//! | Game mixer | [`mixer`]: voices, buses, fades, loops and an allocation-free `render` | `mixer` |
//!
//! Enable the optional modules you use:
//!
//! ```toml
//! [dependencies]
//! audiobox = { version = "0.3", features = ["mixer", "synth", "sfx"] }
//! ```
//!
//! # Determinism
//!
//! These give the same samples on every platform, including WebAssembly: the
//! synthesizer, the sound effects and the mixer, and the offline transforms that render
//! audio — [`Audio::pan`], [`Audio::gain`] and [`Audio::gain_db`], [`Audio::fade`],
//! [`Audio::crossfade_to`], [`Audio::filter`], [`Audio::limit`], [`Audio::resample`],
//! [`Audio::tempo`], [`Audio::pitch`], [`Audio::trim_silence`], peak
//! [`Audio::normalize`], and [`analyze::from_db`]. They compute with the crate's own
//! sine and `2^x` series instead of the platform's `sin`, `cos`, `exp` and `powf`, whose
//! last bits can differ between systems; CI pins their output with checksums on Linux,
//! macOS, Windows and WebAssembly. Analysis that only reports numbers, such as
//! loudness, true peak and spectra, and normalization to LUFS or true peak, which
//! depend on it, use the platform's math.
//!
//! Errors are [`Error`] values with a stable [`ErrorCode`]. Decoding is bounded by
//! [`Limits`], checked before allocating. The [website](https://jmsansan.github.io/audiobox-rs/)
//! has a browser sandbox and a sound-effect and chiptune playground that run this crate
//! through WebAssembly.
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
pub mod analyze;
mod audio;
mod codec;
mod decoder;
mod dsp;
mod error;
mod flac;
mod formats;
mod io;
mod math;
#[cfg(feature = "mixer")]
#[cfg_attr(docsrs, doc(cfg(feature = "mixer")))]
pub mod mixer;
pub mod mp3;
pub mod pcm;
#[cfg(feature = "sfx")]
#[cfg_attr(docsrs, doc(cfg(feature = "sfx")))]
pub mod sfx;
pub mod stream;
#[cfg(feature = "synth")]
#[cfg_attr(docsrs, doc(cfg(feature = "synth")))]
pub mod synth;
mod types;
pub use audio::Audio;
pub use codec::{
    decode, decode_with_options, encode, encode_with_options, read_audio_file, sniff,
    write_audio_file,
};
pub use dsp::*;
pub use error::{Error, ErrorCode, Result};
pub use types::*;
