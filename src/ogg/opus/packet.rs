// Adapted from TPT Cadence and the BSD-licensed Opus reference; see ../NOTICE.md.
//! Opus packet framing (RFC 6716 §3): TOC byte, frame packing codes 0–3,
//! padding, and the configuration tables.
//!
//! Parsing allocates nothing: frame boundaries are written into a fixed
//! 48-entry array (the RFC caps a packet at 48 frames), so this is safe to
//! use on a real-time thread.

use crate::ogg::opus::{CadenceError, Result};

/// Maximum number of frames in one Opus packet (2.5 ms frames, 120 ms).
pub const MAX_FRAMES_PER_PACKET: usize = 48;

/// Operating mode selected by the TOC configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// SILK-only (LP-based, speech-optimized).
    Silk,
    /// Hybrid SILK + CELT.
    Hybrid,
    /// CELT-only (MDCT-based, music-optimized).
    Celt,
}

/// Audio bandwidth selected by the TOC configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bandwidth {
    Narrowband,
    Mediumband,
    Wideband,
    Superwideband,
    Fullband,
}

/// Frame duration selected by the TOC configuration, in microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDuration {
    Ms2_5,
    Ms5,
    Ms10,
    Ms20,
    Ms40,
    Ms60,
}

impl FrameDuration {
    /// Duration in microseconds.
    pub fn micros(self) -> u64 {
        match self {
            FrameDuration::Ms2_5 => 2_500,
            FrameDuration::Ms5 => 5_000,
            FrameDuration::Ms10 => 10_000,
            FrameDuration::Ms20 => 20_000,
            FrameDuration::Ms40 => 40_000,
            FrameDuration::Ms60 => 60_000,
        }
    }
}

/// The decoded TOC byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toc {
    pub config: u8,
    pub stereo: bool,
    /// Frame count code (0–3), i.e. the low two bits.
    pub code: u8,
}

impl Toc {
    pub fn from_byte(b: u8) -> Self {
        Toc {
            config: b >> 3,
            stereo: b & 0x04 != 0,
            code: b & 0x03,
        }
    }

    pub fn to_byte(self) -> u8 {
        (self.config << 3) | ((self.stereo as u8) << 2) | self.code
    }

    /// Operating mode for this configuration.
    pub fn mode(&self) -> Mode {
        match self.config {
            0..=11 => Mode::Silk,
            12..=15 => Mode::Hybrid,
            _ => Mode::Celt,
        }
    }

    /// Audio bandwidth for this configuration.
    pub fn bandwidth(&self) -> Bandwidth {
        match self.config {
            0..=3 => Bandwidth::Narrowband,
            4..=7 => Bandwidth::Mediumband,
            8..=11 => Bandwidth::Wideband,
            12..=13 => Bandwidth::Superwideband,
            14..=15 => Bandwidth::Fullband,
            16..=19 => Bandwidth::Narrowband,
            20..=23 => Bandwidth::Wideband,
            24..=27 => Bandwidth::Superwideband,
            _ => Bandwidth::Fullband,
        }
    }

    /// Frame duration for this configuration.
    pub fn frame_duration(&self) -> FrameDuration {
        match self.config {
            // SILK: 10, 20, 40, 60 ms.
            0..=11 => match self.config % 4 {
                0 => FrameDuration::Ms10,
                1 => FrameDuration::Ms20,
                2 => FrameDuration::Ms40,
                _ => FrameDuration::Ms60,
            },
            // Hybrid: 10, 20 ms.
            12..=15 => {
                if self.config % 2 == 0 {
                    FrameDuration::Ms10
                } else {
                    FrameDuration::Ms20
                }
            }
            // CELT: 2.5, 5, 10, 20 ms.
            _ => match self.config % 4 {
                0 => FrameDuration::Ms2_5,
                1 => FrameDuration::Ms5,
                2 => FrameDuration::Ms10,
                _ => FrameDuration::Ms20,
            },
        }
    }
}

/// A parsed Opus packet: the TOC plus the byte range of each contained frame.
///
/// Frame ranges index into the original payload slice. A zero-length range
/// represents a DTX (discontinuous transmission) frame, which the decoder
/// treats as packet loss.
#[derive(Debug, Clone, Copy)]
pub struct Packet {
    pub toc: Toc,
    /// Count of padding bytes signalled by a code 3 packet.
    pub padding_bytes: usize,
    frame_ranges: [(usize, usize); MAX_FRAMES_PER_PACKET],
    frame_count: usize,
}

impl Packet {
    /// Number of frames in this packet (ranges may be empty for DTX).
    pub fn frame_count(&self) -> usize {
        self.frame_count
    }

    /// Returns the byte range of frame `i` in the original payload.
    pub fn frame_range(&self, i: usize) -> Option<(usize, usize)> {
        self.frame_ranges.get(i).copied()
    }

    /// Maximum number of frames allowed for this TOC's duration (120 ms cap).
    pub fn max_frames_for(toc: Toc) -> usize {
        let ms = toc.frame_duration().micros() / 1000;
        (120 / ms.max(1)) as usize
    }
}

fn read_frame_length(payload: &[u8], pos: &mut usize) -> Result<usize> {
    let b = *payload.get(*pos).ok_or_else(|| {
        CadenceError::CorruptData("packet ends inside a frame length".to_string())
    })?;
    *pos += 1;
    if b < 252 {
        Ok(b as usize)
    } else {
        let b2 = *payload.get(*pos).ok_or_else(|| {
            CadenceError::CorruptData("packet ends inside a two-byte frame length".to_string())
        })?;
        *pos += 1;
        Ok(b2 as usize * 4 + b as usize)
    }
}

/// RFC 6716 normal framing and Appendix B self-delimited multistream framing.
pub fn parse_packet(payload: &[u8]) -> Result<Packet> {
    Ok(parse_delimited(payload, false)?.0)
}
pub fn parse_delimited(payload: &[u8], self_delimited: bool) -> Result<(Packet, usize)> {
    let bad = || CadenceError::CorruptData("invalid Opus packet framing".into());
    let toc = Toc::from_byte(*payload.first().ok_or_else(bad)?);
    let mut pos = 1;
    let (count, vbr, padding) = match toc.code {
        0 => (1, false, 0),
        1 => (2, false, 0),
        2 => (2, true, 0),
        _ => {
            let flags = *payload.get(pos).ok_or_else(bad)?;
            pos += 1;
            let count = (flags & 63) as usize;
            let mut padding = 0usize;
            if flags & 64 != 0 {
                loop {
                    let byte = *payload.get(pos).ok_or_else(bad)?;
                    pos += 1;
                    padding = padding
                        .checked_add(if byte == 255 { 254 } else { byte as usize })
                        .ok_or_else(bad)?;
                    if byte != 255 {
                        break;
                    }
                }
            }
            (count, flags & 128 != 0, padding)
        }
    };
    if count == 0
        || count > MAX_FRAMES_PER_PACKET
        || count as u64 * toc.frame_duration().micros() > 120_000
    {
        return Err(bad());
    }
    let mut lengths = [0usize; MAX_FRAMES_PER_PACKET];
    if vbr {
        for length in &mut lengths[..count - 1] {
            *length = read_frame_length(payload, &mut pos)?;
        }
    }
    if self_delimited {
        let last = read_frame_length(payload, &mut pos)?;
        if vbr {
            lengths[count - 1] = last;
        } else {
            lengths[..count].fill(last);
        }
    } else {
        let data_len = payload
            .len()
            .checked_sub(pos)
            .and_then(|n| n.checked_sub(padding))
            .ok_or_else(bad)?;
        if vbr {
            let known = lengths[..count - 1].iter().sum::<usize>();
            lengths[count - 1] = data_len.checked_sub(known).ok_or_else(bad)?;
        } else {
            if data_len % count != 0 {
                return Err(bad());
            }
            lengths[..count].fill(data_len / count);
        }
    }
    let mut packet = Packet {
        toc,
        padding_bytes: padding,
        frame_ranges: [(0, 0); MAX_FRAMES_PER_PACKET],
        frame_count: count,
    };
    for (i, &length) in lengths[..count].iter().enumerate() {
        if length > 1275 {
            return Err(bad());
        }
        let end = pos
            .checked_add(length)
            .filter(|&n| n <= payload.len())
            .ok_or_else(bad)?;
        packet.frame_ranges[i] = (pos, end);
        pos = end;
    }
    let consumed = pos
        .checked_add(padding)
        .filter(|&n| n <= payload.len())
        .ok_or_else(bad)?;
    if !self_delimited && consumed != payload.len() {
        return Err(bad());
    }
    Ok((packet, consumed))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_codes_padding_dtx_and_self_delimiting() {
        for data in [
            vec![],
            vec![0x81, 1],
            vec![0x83, 0],
            vec![0x83, 49],
            vec![0x83, 48, 1],
            vec![0x83, 0x41, 255],
            vec![0x82, 9, 1],
            vec![0x80; 1277],
        ] {
            assert!(parse_packet(&data).is_err(), "{data:?}");
        }
        for (data, frames) in [
            (vec![0x80], 1),
            (vec![0x81], 2),
            (vec![0x82, 0], 2),
            (vec![0x83, 48], 48),
            (vec![0x83, 0x43, 1, 0, 0, 0, 0], 3),
        ] {
            let p = parse_packet(&data).unwrap();
            assert_eq!(p.frame_count(), frames);
        }
        let (p, n) = parse_delimited(&[0x81, 2, 1, 2, 3, 4, 0x80], true).unwrap();
        assert_eq!((p.frame_count(), n), (2, 6));
        assert_eq!(p.frame_range(1), Some((4, 6)));
        assert!(parse_delimited(&[0x80, 252], true).is_err());
    }
}
