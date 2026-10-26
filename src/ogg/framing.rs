//! Single-link Ogg framing. Opening validates every page before codec preparation;
//! playback reassembles into one preallocated packet, without a packet/page index.
use crate::{Error, Result};

pub(super) const MAX_PACKET: usize = 1024 * 1024;
const fn table() -> [u32; 256] {
    let mut out = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut v = (i as u32) << 24;
        let mut j = 0;
        while j < 8 {
            v = if v & 0x8000_0000 != 0 {
                (v << 1) ^ 0x04c1_1db7
            } else {
                v << 1
            };
            j += 1;
        }
        out[i] = v;
        i += 1;
    }
    out
}
const CRC: [u32; 256] = table();

#[derive(Clone, Copy)]
struct Page {
    flags: u8,
    serial: u32,
    sequence: u32,
    granule: i64,
    laces: usize,
    segments: usize,
    body: usize,
    end: usize,
}
impl Page {
    fn parse(bytes: &[u8], at: usize, checksum: bool) -> Result<Self> {
        let header = bytes
            .get(at..at.saturating_add(27))
            .ok_or_else(|| Error::decode("Truncated Ogg page"))?;
        if &header[..4] != b"OggS" || header[4] != 0 || header[5] & !7 != 0 {
            return Err(Error::decode("Invalid Ogg page header"));
        }
        let segments = header[26] as usize;
        let laces = at + 27;
        let body = laces + segments;
        let lengths = bytes
            .get(laces..body)
            .ok_or_else(|| Error::decode("Truncated Ogg lacing table"))?;
        let end = body
            .checked_add(lengths.iter().map(|&n| n as usize).sum::<usize>())
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| Error::decode("Truncated Ogg body"))?;
        if checksum {
            let mut crc = 0u32;
            for (i, &byte) in bytes[at..end].iter().enumerate() {
                let byte = if (22..26).contains(&i) { 0 } else { byte };
                crc = (crc << 8) ^ CRC[((crc >> 24) as u8 ^ byte) as usize];
            }
            if crc != u32::from_le_bytes(header[22..26].try_into().unwrap()) {
                return Err(Error::decode("Ogg page checksum mismatch"));
            }
        }
        let granule = i64::from_le_bytes(header[6..14].try_into().unwrap());
        if granule < -1 {
            return Err(Error::decode("Invalid Ogg granule"));
        }
        Ok(Self {
            flags: header[5],
            serial: u32::from_le_bytes(header[14..18].try_into().unwrap()),
            sequence: u32::from_le_bytes(header[18..22].try_into().unwrap()),
            granule,
            laces,
            segments,
            body,
            end,
        })
    }
}
pub(super) struct Layout {
    pub max_packet: usize,
    pub granule: u64,
    pub packets: usize,
}
pub(super) fn inspect(bytes: &[u8]) -> Result<Layout> {
    let mut at = 0;
    let mut serial = None;
    let mut sequence = 0u32;
    let mut continued = false;
    let mut packet = 0usize;
    let mut max_packet = 0;
    let mut packets = 0usize;
    let mut previous_granule = 0;
    let mut eos = false;
    while at < bytes.len() {
        if eos {
            return Err(Error::new(
                crate::ErrorCode::UnsupportedFormat,
                "Chained/multiplexed Ogg links are not supported",
            ));
        }
        let p = Page::parse(bytes, at, true)?;
        if let Some(id) = serial {
            if p.serial != id || p.flags & 2 != 0 {
                return Err(Error::new(
                    crate::ErrorCode::UnsupportedFormat,
                    "Chained/multiplexed Ogg links are not supported",
                ));
            }
        } else {
            if p.flags & 2 == 0 || p.flags & 1 != 0 || p.sequence != 0 {
                return Err(Error::decode("Missing Ogg BOS page"));
            }
            serial = Some(p.serial);
        }
        if p.sequence != sequence || (p.flags & 1 != 0) != continued {
            return Err(Error::decode("Ogg sequence/continuation mismatch"));
        }
        sequence = sequence.wrapping_add(1);
        let mut completed = 0;
        for &lace in &bytes[p.laces..p.body] {
            packet = packet
                .checked_add(lace as usize)
                .filter(|&n| n <= MAX_PACKET)
                .ok_or_else(|| Error::limit("Ogg packet exceeds 1 MiB limit"))?;
            continued = lace == 255;
            if !continued {
                max_packet = max_packet.max(packet);
                packet = 0;
                packets += 1;
                completed += 1;
            }
        }
        if completed == 0 && p.granule != -1 {
            return Err(Error::decode(
                "Ogg page without a completed packet needs granule -1",
            ));
        }
        if p.granule >= 0 {
            if p.granule < previous_granule {
                return Err(Error::decode("Ogg granules move backwards"));
            }
            previous_granule = p.granule;
        }
        eos = p.flags & 4 != 0;
        if eos && (continued || completed == 0 || p.granule < 0) {
            return Err(Error::decode("Incomplete Ogg EOS page"));
        }
        at = p.end;
    }
    if !eos || packets == 0 {
        return Err(Error::decode("Missing Ogg EOS/packets"));
    }
    Ok(Layout {
        max_packet,
        granule: previous_granule as u64,
        packets,
    })
}

#[derive(Clone, Default)]
pub(super) struct Cursor {
    page: Option<Page>,
    next: usize,
    lace: usize,
    body: usize,
}
pub(super) struct Packet<'a> {
    pub data: &'a [u8],
    pub granule: Option<u64>,
    pub eos: bool,
}
impl Cursor {
    pub fn next<'a>(
        &mut self,
        bytes: &[u8],
        scratch: &'a mut Vec<u8>,
    ) -> Result<Option<Packet<'a>>> {
        scratch.clear();
        loop {
            if self.page.is_none_or(|p| self.lace == p.segments) {
                if self.next == bytes.len() {
                    if !scratch.is_empty() {
                        return Err(Error::decode("Truncated Ogg packet"));
                    }
                    return Ok(None);
                }
                let p = Page::parse(bytes, self.next, false)?;
                self.next = p.end;
                self.body = p.body;
                self.lace = 0;
                self.page = Some(p);
                if p.segments == 0 {
                    continue;
                }
            }
            let p = self.page.unwrap();
            let n = bytes[p.laces + self.lace] as usize;
            if scratch.len() + n > scratch.capacity() {
                return Err(Error::limit("Ogg packet exceeds prepared capacity"));
            }
            scratch.extend_from_slice(&bytes[self.body..self.body + n]);
            self.body += n;
            self.lace += 1;
            if n < 255 {
                let last = !bytes[p.laces + self.lace..p.body].iter().any(|&n| n < 255);
                return Ok(Some(Packet {
                    data: scratch,
                    granule: (last && p.granule >= 0).then_some(p.granule as u64),
                    eos: p.flags & 4 != 0 && last,
                }));
            }
        }
    }
}

pub(crate) fn sniff(bytes: &[u8]) -> Option<crate::AudioFormat> {
    if bytes.get(..4)? != b"OggS" {
        return None;
    }
    let p = Page::parse(bytes, 0, false).ok()?;
    let body = bytes.get(p.body..p.end)?;
    if body.starts_with(b"\x01vorbis") {
        Some(crate::AudioFormat::Vorbis)
    } else if body.starts_with(b"OpusHead") {
        Some(crate::AudioFormat::Opus)
    } else {
        None
    }
}
