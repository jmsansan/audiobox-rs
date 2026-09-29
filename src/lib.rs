//! Audio decoding, editing and encoding in Rust, with zero dependencies.
//!
//! ```
//! use audiobox::{Audio, AudioFormat, decode, encode};
//! let original = Audio::new(vec![vec![0.0; 4800]], 48_000)?;
//! let bytes = encode(&original, AudioFormat::Wav)?;
//! let restored = decode(&bytes)?;
//! assert_eq!(restored.frames(), original.frames());
//! # Ok::<(), audiobox::Error>(())
//! ```
#![forbid(unsafe_code)]
pub mod analyze;
mod audio;
mod codec;
mod dsp;
mod error;
mod flac;
mod formats;
mod io;
pub mod mp3;
pub mod pcm;
pub mod stream;
mod types;
pub use audio::Audio;
pub use codec::{
    decode, decode_with_options, encode, encode_with_options, read_audio_file, sniff,
    write_audio_file,
};
pub use dsp::*;
pub use error::{Error, ErrorCode, Result};
pub use types::*;
