//! MPEG-1/2/2.5 Layer III decoding, probing and MPEG-1 CBR encoding.
mod decoder;
mod encoder;
mod tables;
mod transform;
use crate::{Audio, DecodeOptions, EncodeOptions, Error, Result};
pub(crate) use decoder::decode;
pub(crate) use encoder::encode;

#[derive(Clone, Copy, Debug)]
pub struct Mp3Info {
    pub sample_rate: u32,
    pub channels: usize,
    pub frames: usize,
    pub duration: f64,
    pub bitrate: u32,
    pub mpeg_version: u8,
    pub encoder_delay: usize,
    pub encoder_padding: usize,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Header {
    pub version: u8,
    pub rate: u32,
    pub channels: usize,
    pub length: usize,
    pub side: usize,
    pub crc: usize,
    pub mode: u8,
    pub extension: u8,
    pub bitrate: u32,
}
pub(super) const BITRATES: [u32; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];
pub(super) fn header(bytes: &[u8], at: usize) -> Option<Header> {
    let b = bytes.get(at..at.checked_add(4)?)?;
    if b[0] != 255 || b[1] & 224 != 224 || b[1] & 6 != 2 {
        return None;
    }
    let version = match (b[1] >> 3) & 3 {
        3 => 1,
        2 => 2,
        0 => 25,
        _ => return None,
    };
    let index = (b[2] >> 4) as usize;
    let rate_index = ((b[2] >> 2) & 3) as usize;
    if index == 0 || index == 15 || rate_index == 3 {
        return None;
    }
    let rate = [44100, 48000, 32000][rate_index]
        / if version == 1 {
            1
        } else if version == 2 {
            2
        } else {
            4
        };
    let bitrate = if version == 1 {
        BITRATES[index]
    } else {
        [
            0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
        ][index]
    };
    let mode = b[3] >> 6;
    let channels = if mode == 3 { 1 } else { 2 };
    let length = (if version == 1 { 144 } else { 72 }) * bitrate as usize * 1000 / rate as usize
        + ((b[2] >> 1) & 1) as usize;
    let side = if version == 1 {
        if channels == 1 { 17 } else { 32 }
    } else if channels == 1 {
        9
    } else {
        17
    };
    let crc = if b[1] & 1 == 0 { 2 } else { 0 };
    if length < 4 + side + crc {
        return None;
    }
    Some(Header {
        version,
        rate,
        channels,
        length,
        side,
        crc,
        mode,
        extension: (b[3] >> 4) & 3,
        bitrate,
    })
}
pub(super) fn tag_size(bytes: &[u8]) -> usize {
    if !bytes.starts_with(b"ID3") || bytes.len() < 10 {
        return 0;
    }
    10 + ((bytes[6] as usize & 127) << 21)
        + ((bytes[7] as usize & 127) << 14)
        + ((bytes[8] as usize & 127) << 7)
        + (bytes[9] as usize & 127)
        + if bytes[5] & 16 != 0 { 10 } else { 0 }
}
pub(super) fn find(bytes: &[u8], start: usize) -> Option<(usize, Header)> {
    for i in start..bytes.len().saturating_sub(3) {
        if let Some(h) = header(bytes, i) {
            if i + h.length > bytes.len() {
                continue;
            }
            if i + h.length + 4 <= bytes.len()
                && bytes.get(i + h.length..i + h.length + 3) != Some(b"TAG")
            {
                let next = header(bytes, i + h.length);
                if next.is_none_or(|n| n.rate != h.rate || n.version != h.version) {
                    continue;
                }
            }
            return Some((i, h));
        }
    }
    None
}
pub(crate) fn has_frame(bytes: &[u8]) -> bool {
    find(&bytes[..bytes.len().min(8192)], 0).is_some()
}
#[derive(Clone, Copy, Default)]
pub(super) struct Vbr {
    pub frames: usize,
    pub delay: usize,
    pub padding: usize,
}
pub(super) fn vbr(bytes: &[u8], at: usize, h: Header) -> Result<Option<Vbr>> {
    let start = at + 4 + h.crc + h.side;
    let end = at + h.length;
    let read = |p: usize| -> Result<u32> {
        Ok(u32::from_be_bytes(
            bytes
                .get(p..p + 4)
                .filter(|_| p + 4 <= end)
                .ok_or_else(|| Error::decode("Truncated Xing tag"))?
                .try_into()
                .unwrap(),
        ))
    };
    if bytes.get(start..start + 4) == Some(b"Xing") || bytes.get(start..start + 4) == Some(b"Info")
    {
        let flags = read(start + 4)?;
        let mut p = start + 8;
        let mut info = Vbr::default();
        if flags & 1 != 0 {
            info.frames = read(p)? as usize;
            p += 4;
        }
        if flags & 2 != 0 {
            read(p)?;
            p += 4;
        }
        if flags & 4 != 0 {
            p += 100;
        }
        if flags & 8 != 0 {
            p += 4;
        }
        if p + 24 <= end && bytes[p..p + 9].iter().all(|&c| (32..=126).contains(&c)) {
            let d = (bytes[p + 21] as usize) << 4 | (bytes[p + 22] as usize) >> 4;
            let pad = ((bytes[p + 22] as usize & 15) << 8) | bytes[p + 23] as usize;
            if d <= 2304 && pad <= 3456 {
                info.delay = d;
                info.padding = pad;
            }
        }
        Ok(Some(info))
    } else if bytes.get(at + 36..at + 40) == Some(b"VBRI") {
        Ok(Some(Vbr {
            frames: read(at + 50)? as usize,
            ..Default::default()
        }))
    } else {
        Ok(None)
    }
}
pub fn probe_mp3(bytes: &[u8]) -> Result<Mp3Info> {
    let (at, h) =
        find(bytes, tag_size(bytes)).ok_or_else(|| Error::decode("No MPEG Layer III frame"))?;
    let tag = vbr(bytes, at, h)?;
    let mut pos = at;
    let mut count = 0usize;
    let mut size = 0;
    while let Some(h2) = header(bytes, pos) {
        if h2.rate != h.rate || h2.channels != h.channels || pos + h2.length > bytes.len() {
            break;
        }
        size += h2.length;
        count += 1;
        pos += h2.length;
    }
    let audio_frames = count.saturating_sub(usize::from(tag.is_some()));
    let tag = tag.unwrap_or_default();
    let samples = audio_frames * if h.version == 1 { 1152 } else { 576 };
    let samples = samples.saturating_sub(tag.delay + tag.padding);
    let duration = samples as f64 / h.rate as f64;
    Ok(Mp3Info {
        sample_rate: h.rate,
        channels: h.channels,
        frames: audio_frames,
        duration,
        bitrate: if duration > 0.0 {
            (size as f64 * 8.0 / duration).round() as u32
        } else {
            h.bitrate * 1000
        },
        mpeg_version: h.version,
        encoder_delay: tag.delay,
        encoder_padding: tag.padding,
    })
}
/// Decode MP3 using the default safety limits.
pub fn decode_mp3(bytes: &[u8]) -> Result<Audio> {
    decode(bytes, &DecodeOptions::default())
}
pub fn encode_mp3(audio: &Audio, options: &EncodeOptions) -> Result<Vec<u8>> {
    encode(audio, options)
}
