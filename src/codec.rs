use crate::{Audio, AudioFormat, DecodeOptions, EncodeOptions, Error, ErrorCode, Result};
use std::path::Path;

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
    match format {
        AudioFormat::Wav => crate::formats::encode_wav(audio, options),
        AudioFormat::Aiff => crate::formats::encode_aiff(audio, options),
        AudioFormat::Caf => crate::formats::encode_caf(audio, options),
        AudioFormat::Au => crate::formats::encode_au(audio, options),
        AudioFormat::Flac => crate::flac::encode(audio, options),
        AudioFormat::Mp3 => crate::mp3::encode(audio, options),
        AudioFormat::Raw => crate::pcm::encode_pcm(audio, options.sample_format, options.endian),
    }
}
pub fn read_audio_file(path: impl AsRef<Path>) -> Result<Audio> {
    decode(&std::fs::read(path)?)
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
