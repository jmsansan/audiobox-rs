use crate::{Audio, AudioFormat, DecodeOptions, EncodeOptions, Error, ErrorCode, Result};
use std::io::Read;
use std::path::Path;

/// Limits for the whole-file convenience reader. Encoded bytes and decoded PCM
/// have separate budgets; codec-specific limits still apply during decoding.
#[derive(Clone, Debug)]
pub struct ReadAudioOptions {
    /// Maximum encoded file bytes; exact equality is accepted. Defaults to 512 MiB.
    pub max_encoded_bytes: usize,
    /// Decoder format override and decoded PCM/shape limits.
    pub decode: DecodeOptions,
}
impl Default for ReadAudioOptions {
    fn default() -> Self {
        Self {
            max_encoded_bytes: 512 * 1024 * 1024,
            decode: DecodeOptions::default(),
        }
    }
}

/// Detect a supported format from its bytes, independent of the extension.
pub fn sniff(bytes: &[u8]) -> Option<AudioFormat> {
    if bytes.len() < 4 {
        return None;
    }
    match &bytes[..4] {
        b"RIFF" | b"RF64" | b"BW64" if bytes.get(8..12) == Some(b"WAVE") => Some(AudioFormat::Wav),
        b"FORM" if bytes.get(8..12) == Some(b"AIFF") || bytes.get(8..12) == Some(b"AIFC") => {
            Some(AudioFormat::Aiff)
        }
        b"caff" => Some(AudioFormat::Caf),
        b".snd" => Some(AudioFormat::Au),
        b"fLaC" => Some(AudioFormat::Flac),
        b"OggS" => crate::ogg::sniff(bytes),
        _ if bytes.starts_with(b"ID3") || crate::mp3::has_frame(bytes) => Some(AudioFormat::Mp3),
        _ => None,
    }
}
pub fn decode(bytes: &[u8]) -> Result<Audio> {
    decode_with_options(bytes, &DecodeOptions::default())
}
pub fn decode_with_options(bytes: &[u8], options: &DecodeOptions) -> Result<Audio> {
    let format = options.format.or_else(|| sniff(bytes)).ok_or_else(|| {
        Error::new(
            ErrorCode::UnsupportedFormat,
            "Unknown or unsupported audio format",
        )
    })?;
    match format {
        AudioFormat::Wav => crate::formats::decode_wav(bytes, options),
        AudioFormat::Aiff => crate::formats::decode_aiff(bytes, options),
        AudioFormat::Caf => crate::formats::decode_caf(bytes, options),
        AudioFormat::Au => crate::formats::decode_au(bytes, options),
        AudioFormat::Flac => crate::flac::decode(bytes, options),
        AudioFormat::Mp3 => crate::mp3::decode(bytes, options),
        AudioFormat::Vorbis | AudioFormat::Opus => crate::ogg::decode(bytes, format, options),
        AudioFormat::Raw => Err(Error::invalid(
            "Raw PCM requires pcm::decode_pcm with an explicit shape",
        )),
    }
}
pub fn encode(audio: &Audio, format: AudioFormat) -> Result<Vec<u8>> {
    encode_with_options(audio, format, &EncodeOptions::default())
}
pub fn encode_with_options(
    audio: &Audio,
    format: AudioFormat,
    options: &EncodeOptions,
) -> Result<Vec<u8>> {
    if options.dither != crate::Dither::None {
        if format == AudioFormat::Mp3 {
            return Err(Error::invalid("Dither is not supported for MP3 output"));
        }
        crate::pcm::validate_dither(options.sample_format, options.dither)?;
    }
    crate::metadata::validate_options(options, format)?;
    match format {
        AudioFormat::Wav => crate::formats::encode_wav(audio, options),
        AudioFormat::Aiff => crate::formats::encode_aiff(audio, options),
        AudioFormat::Caf => crate::formats::encode_caf(audio, options),
        AudioFormat::Au => crate::formats::encode_au(audio, options),
        AudioFormat::Flac => crate::flac::encode(audio, options),
        AudioFormat::Mp3 => crate::mp3::encode(audio, options),
        AudioFormat::Vorbis | AudioFormat::Opus => Err(Error::new(
            ErrorCode::UnsupportedFormat,
            "Ogg encoding is not supported",
        )),
        AudioFormat::Raw => crate::pcm::encode_pcm_with_dither(
            audio,
            options.sample_format,
            options.endian,
            options.dither,
        ),
    }
}
/// Decode a regular file using a 512 MiB encoded-byte cap and default decoded
/// [`crate::Limits`]. See [`read_audio_file_with_options`] for path and error policy.
pub fn read_audio_file(path: impl AsRef<Path>) -> Result<Audio> {
    read_audio_file_with_options(path, &ReadAudioOptions::default())
}
/// Read a bounded regular file, then decode the same buffer without copying it.
///
/// Follows symlinks to regular files, including outside any project directory;
/// containment is the caller's responsibility. Static directories, FIFOs, devices
/// and other nonregular targets are rejected **before opening** with
/// `InvalidArgument`. The opened handle is checked again. This assumes a stable
/// filesystem path: portable safe std I/O does not atomically protect the open
/// against hostile replacement by a FIFO or another target between checks.
///
/// Both metadata size and actual bytes are bounded. Reads at most cap+1 bytes
/// (the extra byte detects growth), stores at most cap, and accepts exact-cap
/// files. Oversize or failed buffer reservation returns `LimitExceeded`; I/O
/// errors return `Io`. Invalid encoded data/decoded limits retain decoder errors.
/// There is no unrestricted `fs::read` or second encoded-buffer allocation.
pub fn read_audio_file_with_options(
    path: impl AsRef<Path>,
    options: &ReadAudioOptions,
) -> Result<Audio> {
    let path = path.as_ref();
    let check = |metadata: std::fs::Metadata| {
        if !metadata.is_file() {
            return Err(Error::invalid("Audio input must be a regular file"));
        }
        if metadata.len() as u128 > options.max_encoded_bytes as u128 {
            return Err(Error::limit("Encoded file exceeds byte limit"));
        }
        Ok(())
    };
    check(std::fs::metadata(path)?)?;
    let file = std::fs::File::open(path)?;
    check(file.metadata()?)?;
    decode_with_options(
        &read_bounded(file, options.max_encoded_bytes)?,
        &options.decode,
    )
}

fn read_bounded(mut reader: impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let remaining = limit - bytes.len();
        let count = remaining.min(chunk.len()).max(1);
        let n = match reader.read(&mut chunk[..count]) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            value => value?,
        };
        if n == 0 {
            return Ok(bytes);
        }
        if n > remaining {
            return Err(Error::limit("Encoded file exceeds byte limit"));
        }
        let next = bytes.len() + n;
        if next > bytes.capacity() {
            let target = next.max(
                bytes
                    .capacity()
                    .saturating_add(bytes.capacity() / 2)
                    .min(limit),
            );
            bytes
                .try_reserve_exact(target - bytes.len())
                .map_err(|_| Error::limit("Cannot reserve encoded file buffer"))?;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
}
pub fn write_audio_file(
    path: impl AsRef<Path>,
    audio: &Audio,
    options: &EncodeOptions,
) -> Result<()> {
    let path = path.as_ref();
    let format = path
        .extension()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::invalid("Output path requires an audio extension"))?
        .parse()?;
    std::fs::write(path, encode_with_options(audio, format, options)?)?;
    Ok(())
}

#[cfg(test)]
mod bounded_read_tests {
    use super::*;

    #[test]
    fn actual_bytes_are_bounded_even_when_the_source_keeps_growing() {
        struct Growing {
            read: usize,
        }
        impl Read for Growing {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                bytes.fill(7);
                self.read += bytes.len();
                Ok(bytes.len())
            }
        }
        for limit in [0, 1, 8191, 8192, 8193, 20000] {
            let mut source = Growing { read: 0 };
            assert_eq!(
                read_bounded(&mut source, limit).unwrap_err().code,
                ErrorCode::LimitExceeded
            );
            assert_eq!(source.read, limit + 1);
            let bytes = vec![7; limit];
            assert_eq!(
                read_bounded(std::io::Cursor::new(&bytes), limit).unwrap(),
                bytes
            );
        }
    }

    #[test]
    fn interrupted_reads_are_retried_and_other_errors_are_io() {
        struct Interrupted(bool);
        impl Read for Interrupted {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    Err(std::io::ErrorKind::Interrupted.into())
                } else {
                    Ok(0)
                }
            }
        }
        assert!(read_bounded(Interrupted(false), 0).unwrap().is_empty());
        struct Failed;
        impl Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::PermissionDenied.into())
            }
        }
        assert_eq!(read_bounded(Failed, 10).unwrap_err().code, ErrorCode::Io);
    }
}
