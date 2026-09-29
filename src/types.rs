use crate::{Error, Result};
use std::{collections::BTreeMap, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioFormat {
    Wav,
    Aiff,
    Caf,
    Au,
    Flac,
    Mp3,
    Raw,
}
impl FromStr for AudioFormat {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "wav" | "wave" => Ok(Self::Wav),
            "aiff" | "aif" | "aifc" => Ok(Self::Aiff),
            "caf" => Ok(Self::Caf),
            "au" | "snd" => Ok(Self::Au),
            "flac" => Ok(Self::Flac),
            "mp3" => Ok(Self::Mp3),
            "raw" | "pcm" => Ok(Self::Raw),
            _ => Err(Error::new(
                crate::ErrorCode::UnsupportedFormat,
                format!("Unsupported format: {s}"),
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SampleFormat {
    U8,
    S8,
    #[default]
    S16,
    S24,
    S32,
    F32,
    F64,
    Alaw,
    Ulaw,
}
impl SampleFormat {
    pub fn bytes(self) -> usize {
        match self {
            Self::U8 | Self::S8 | Self::Alaw | Self::Ulaw => 1,
            Self::S16 => 2,
            Self::S24 => 3,
            Self::S32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }
    pub fn bits(self) -> u16 {
        (self.bytes() * 8) as u16
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Endian {
    #[default]
    Little,
    Big,
}

/// Allocation and duration limits checked before allocating decoded PCM.
#[derive(Clone, Debug)]
pub struct Limits {
    pub max_decoded_bytes: usize,
    pub max_duration_seconds: f64,
    pub max_channels: usize,
    pub min_sample_rate: u32,
    pub max_sample_rate: u32,
    pub max_frames: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_decoded_bytes: 512 * 1024 * 1024,
            max_duration_seconds: 21600.0,
            max_channels: 64,
            min_sample_rate: 1000,
            max_sample_rate: 768000,
            max_frames: u32::MAX as usize,
        }
    }
}
impl Limits {
    pub fn check(&self, frames: usize, channels: usize, rate: u32) -> Result<()> {
        if !self.max_duration_seconds.is_finite() || self.max_duration_seconds < 0.0 {
            return Err(Error::invalid(
                "Duration limit must be finite and non-negative",
            ));
        }
        if rate == 0 || rate < self.min_sample_rate || rate > self.max_sample_rate {
            return Err(Error::limit("Sample rate exceeds decode limits"));
        }
        if channels == 0 || channels > self.max_channels {
            return Err(Error::limit("Channel count exceeds decode limits"));
        }
        if frames > self.max_frames {
            return Err(Error::limit("Frame count exceeds decode limits"));
        }
        if frames as f64 / rate as f64 > self.max_duration_seconds {
            return Err(Error::limit("Duration exceeds decode limits"));
        }
        let bytes = frames
            .checked_mul(channels)
            .and_then(|v| v.checked_mul(4))
            .ok_or_else(|| Error::limit("Decoded size overflow"))?;
        if bytes > self.max_decoded_bytes {
            return Err(Error::limit("Decoded bytes exceed decode limits"));
        }
        Ok(())
    }
}

pub type AudioMetadata = BTreeMap<String, String>;
#[derive(Clone, Debug, Default)]
pub struct DecodeInfo {
    pub format: Option<AudioFormat>,
    pub sample_format: Option<SampleFormat>,
    pub metadata: AudioMetadata,
}
#[derive(Clone, Debug, Default)]
pub struct DecodeOptions {
    pub format: Option<AudioFormat>,
    pub limits: Limits,
}
#[derive(Clone, Debug)]
pub struct EncodeOptions {
    pub sample_format: SampleFormat,
    pub endian: Endian,
    pub metadata: AudioMetadata,
    /// MP3 bitrate in kbit/s.
    pub bitrate: u32,
    /// FLAC compression level, 0 through 8.
    pub compression_level: u8,
}
impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            sample_format: SampleFormat::S16,
            endian: Endian::Little,
            metadata: AudioMetadata::new(),
            bitrate: 192,
            compression_level: 5,
        }
    }
}

/// Seconds, an exact frame position, or a parsed timestamp. Negative positions count from the end.
#[derive(Clone, Copy, Debug)]
pub enum TimePosition {
    Seconds(f64),
    Frames(i64),
}
impl From<f64> for TimePosition {
    fn from(value: f64) -> Self {
        Self::Seconds(value)
    }
}
impl FromStr for TimePosition {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        let text = value.trim().to_ascii_lowercase();
        let parse = |s: &str| {
            s.trim()
                .parse::<f64>()
                .map_err(|_| Error::invalid("Invalid time position"))
        };
        let seconds = if let Some(s) = text.strip_suffix("ms") {
            parse(s)? / 1000.0
        } else if let Some(s) = text
            .strip_suffix("seconds")
            .or_else(|| text.strip_suffix("second"))
            .or_else(|| text.strip_suffix("sec"))
            .or_else(|| text.strip_suffix('s'))
        {
            parse(s)?
        } else if text.contains(':') {
            let parts: Vec<_> = text.split(':').collect();
            if parts.len() > 3 {
                return Err(Error::invalid("Too many timestamp components"));
            }
            let mut total = 0.0;
            for part in parts {
                let n = parse(part)?;
                if n < 0.0 {
                    return Err(Error::invalid("Negative timestamp component"));
                }
                total = total * 60.0 + n;
            }
            total
        } else {
            parse(&text)?
        };
        if !seconds.is_finite() {
            return Err(Error::invalid("Time must be finite"));
        }
        Ok(Self::Seconds(seconds))
    }
}
impl TimePosition {
    pub(crate) fn index(self, rate: u32, frames: usize) -> Result<usize> {
        let n = match self {
            Self::Frames(n) => n as f64,
            Self::Seconds(s) => s * rate as f64,
        };
        if !n.is_finite() {
            return Err(Error::invalid("Time must be finite"));
        }
        let n = n.round();
        Ok((if n < 0.0 { frames as f64 + n } else { n }).clamp(0.0, frames as f64) as usize)
    }
    pub(crate) fn duration(self, rate: u32) -> Result<usize> {
        let n = match self {
            Self::Frames(n) => n as f64,
            Self::Seconds(s) => s * rate as f64,
        };
        if !n.is_finite() || n < 0.0 || n > usize::MAX as f64 {
            return Err(Error::invalid("Invalid duration"));
        }
        Ok(n.round() as usize)
    }
}
