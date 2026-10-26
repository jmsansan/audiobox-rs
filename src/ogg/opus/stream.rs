//! Bounded audiobox packet assembly for Ogg Opus mapping families 0 and 1.
use super::{
    decoder::OpusDecoder,
    packet::{self, Packet},
};
use crate::{Error, ErrorCode, Limits, Result};

#[derive(Clone, Copy)]
pub(crate) struct Head {
    pub channels: usize,
    pub pre_skip: usize,
    streams: usize,
    coupled: usize,
    mapping: [u8; 8],
    gain: i16,
    family: u8,
}
impl Head {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 19 || !bytes.starts_with(b"OpusHead") {
            return Err(Error::decode("Truncated OpusHead"));
        }
        if bytes[8] > 15 {
            return Err(Error::new(
                ErrorCode::UnsupportedFormat,
                "Unsupported OpusHead version",
            ));
        }
        let channels = bytes[9] as usize;
        if channels == 0 {
            return Err(Error::decode("Opus has zero channels"));
        }
        if channels > 8 {
            return Err(Error::new(
                ErrorCode::UnsupportedFormat,
                "Opus supports at most eight output channels",
            ));
        }
        let mut mapping = [255; 8];
        let (streams, coupled) = match bytes[18] {
            0 => {
                if channels > 2 {
                    return Err(Error::decode("Opus family 0 needs one or two channels"));
                }
                mapping[0] = 0;
                if channels == 2 {
                    mapping[1] = 1;
                }
                (1, channels - 1)
            }
            1 => {
                if bytes.len() < 21 + channels {
                    return Err(Error::decode("Truncated Opus channel mapping"));
                }
                let streams = bytes[19] as usize;
                let coupled = bytes[20] as usize;
                if streams == 0 || streams > 8 || coupled > streams {
                    return Err(Error::decode("Invalid Opus stream counts"));
                }
                mapping[..channels].copy_from_slice(&bytes[21..21 + channels]);
                if mapping[..channels]
                    .iter()
                    .any(|&n| n != 255 && n as usize >= streams + coupled)
                {
                    return Err(Error::decode("Opus channel mapping out of range"));
                }
                (streams, coupled)
            }
            _ => {
                return Err(Error::new(
                    ErrorCode::UnsupportedFormat,
                    "Unsupported Opus mapping family",
                ));
            }
        };
        Ok(Self {
            channels,
            pre_skip: u16::from_le_bytes(bytes[10..12].try_into().unwrap()) as usize,
            streams,
            coupled,
            mapping,
            gain: i16::from_le_bytes(bytes[16..18].try_into().unwrap()),
            family: bytes[18],
        })
    }
}
type Parts = [Option<(Packet, usize, usize)>; 8];
pub(crate) struct Core {
    head: Head,
    decoders: Vec<OpusDecoder>,
    scratch: Vec<f32>,
    output: Vec<f32>,
    gain: f64,
}
impl Core {
    pub fn new(
        head: Head,
        limits: &Limits,
        packet_bytes: usize,
        metadata_bytes: usize,
    ) -> Result<Self> {
        limits.check(0, head.channels, 48000)?;
        let mut budget = crate::ogg::error::Budget {
            remaining: limits.max_decoded_bytes.min(64 * 1024 * 1024),
        };
        budget.charge(
            head.streams * 2 * 1024 * 1024
                + 5760 * head.channels * 4
                + 11520 * 4
                + packet_bytes
                + metadata_bytes,
        )?;
        let decoders = (0..head.streams)
            .map(|i| OpusDecoder::new(if i < head.coupled { 2 } else { 1 }))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Self {
            head,
            decoders,
            scratch: vec![0.0; 11520],
            output: vec![0.0; 5760 * head.channels],
            gain: 10f64.powf(head.gain as f64 / (256.0 * 20.0)),
        })
    }
    pub fn channels(&self) -> usize {
        self.head.channels
    }
    pub fn pre_skip(&self) -> usize {
        self.head.pre_skip
    }
    fn parts(&self, data: &[u8]) -> Result<(Parts, usize)> {
        let mut parts = [None; 8];
        let mut at = 0;
        let mut duration = None;
        for (i, slot) in parts.iter_mut().enumerate().take(self.head.streams) {
            let (packet, used) = packet::parse_delimited(&data[at..], i + 1 < self.head.streams)?;
            let frames = super::decoder::celt_frame_size(&packet) * packet.frame_count();
            if duration.is_some_and(|d| d != frames) {
                return Err(Error::decode("Opus multistream durations disagree"));
            }
            duration = Some(frames);
            *slot = Some((packet, at, at + used));
            at += used;
        }
        if at != data.len() {
            return Err(Error::decode("Trailing Opus multistream bytes"));
        }
        Ok((parts, duration.unwrap_or(0)))
    }
    pub fn duration(&self, data: &[u8]) -> Result<usize> {
        Ok(self.parts(data)?.1)
    }
    pub fn reset(&mut self) {
        for d in &mut self.decoders {
            d.reset();
        }
    }
    pub fn decode(&mut self, data: &[u8]) -> Result<&[f32]> {
        let (parts, frames) = self.parts(data)?;
        self.output[..frames * self.head.channels].fill(0.0);
        for (i, (decoder, part)) in self.decoders.iter_mut().zip(parts).enumerate() {
            let (packet, start, end) = part.unwrap();
            let channels = if i < self.head.coupled { 2 } else { 1 };
            let count = decoder.decode_packet(&packet, &data[start..end], &mut self.scratch)?;
            if count != frames {
                return Err(Error::decode("Opus decoder duration mismatch"));
            }
            for dest in 0..self.head.channels {
                let order = if self.head.family == 1 && self.head.channels > 2 {
                    const WAV: [[usize; 8]; 6] = [
                        [0, 2, 1, 0, 0, 0, 0, 0],
                        [0, 1, 2, 3, 0, 0, 0, 0],
                        [0, 2, 1, 3, 4, 0, 0, 0],
                        [0, 2, 1, 5, 3, 4, 0, 0],
                        [0, 2, 1, 6, 5, 3, 4, 0],
                        [0, 2, 1, 7, 5, 6, 3, 4],
                    ];
                    WAV[self.head.channels - 3][dest]
                } else {
                    dest
                };
                let map = self.head.mapping[order] as usize;
                let stream = if map < self.head.coupled * 2 {
                    map / 2
                } else {
                    self.head.coupled + map - self.head.coupled * 2
                };
                if map != 255 && stream == i {
                    let channel = if channels == 2 { map % 2 } else { 0 };
                    for f in 0..frames {
                        self.output[f * self.head.channels + dest] =
                            (self.scratch[f * channels + channel] as f64 * self.gain) as f32;
                    }
                }
            }
        }
        let out = &self.output[..frames * self.head.channels];
        if out.iter().any(|s| !s.is_finite()) {
            return Err(Error::decode("Nonfinite Opus PCM"));
        }
        Ok(out)
    }
}
