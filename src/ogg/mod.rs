//! Bounded, single-link Ogg decoding. Packet DSP attribution is in NOTICE.md.
mod error;
mod framing;
mod opus;
mod vorbis;
use crate::{Audio, AudioFormat, DecodeInfo, DecodeOptions, Error, Limits, Result};
use framing::Cursor;
pub(crate) use framing::sniff;

enum Dsp {
    Vorbis(Box<vorbis::Core>),
    Opus(Box<opus::Core>),
}
impl Dsp {
    fn reset(&mut self) {
        match self {
            Self::Vorbis(c) => c.reset(),
            Self::Opus(c) => c.reset(),
        }
    }
    fn decode(&mut self, packet: &[u8]) -> Result<&[f32]> {
        match self {
            Self::Vorbis(c) => c.decode(packet),
            Self::Opus(c) => c.decode(packet),
        }
    }
}

pub(crate) struct Stream {
    cursor: Cursor,
    start: Cursor,
    packet: Vec<u8>,
    core: Dsp,
    pub channels: usize,
    pub rate: u32,
    pub frames: u64,
    pub capacity: usize,
    info: DecodeInfo,
    produced: u64,
    leading: usize,
    skip: usize,
    finished: bool,
}
impl Stream {
    pub fn open(bytes: &[u8], format: AudioFormat, limits: &Limits) -> Result<Self> {
        let layout = framing::inspect(bytes)?;
        if sniff(bytes) != Some(format) {
            return Err(Error::decode("Ogg codec does not match requested format"));
        }
        let mut cursor = Cursor::default();
        let mut packet = Vec::new();
        if layout.max_packet > limits.max_decoded_bytes {
            return Err(Error::limit("Ogg packet exceeds workspace byte limit"));
        }
        packet
            .try_reserve_exact(layout.max_packet)
            .map_err(|_| Error::limit("Cannot reserve Ogg packet buffer"))?;
        let (core, channels, rate, capacity, metadata) = match format {
            AudioFormat::Vorbis => {
                if layout.packets < 3 {
                    return Err(Error::decode("Ogg Vorbis needs three headers"));
                }
                let id: [u8; 30] = cursor
                    .next(bytes, &mut packet)?
                    .ok_or_else(|| Error::decode("Missing Vorbis identification"))?
                    .data
                    .try_into()
                    .map_err(|_| Error::decode("Vorbis identification must be 30 bytes"))?;
                let (channels, rate, block) = vorbis::Core::id(&id)?;
                limits.check(0, channels, rate)?;
                let comment = cursor
                    .next(bytes, &mut packet)?
                    .ok_or_else(|| Error::decode("Missing Vorbis comment"))?
                    .data;
                if !comment.starts_with(b"\x03vorbis") {
                    return Err(Error::decode("Invalid Vorbis comment header"));
                }
                if comment.len() > limits.max_decoded_bytes / 4 {
                    return Err(Error::limit("Vorbis metadata exceeds workspace byte limit"));
                }
                let metadata = comments(&comment[7..], true)?;
                let setup = cursor
                    .next(bytes, &mut packet)?
                    .ok_or_else(|| Error::decode("Missing Vorbis setup"))?
                    .data;
                let core = vorbis::Core::new(
                    &id,
                    setup,
                    limits,
                    layout.max_packet + comment_len_bytes(&metadata),
                )?;
                (
                    Dsp::Vorbis(Box::new(core)),
                    channels,
                    rate,
                    block / 2,
                    metadata,
                )
            }
            AudioFormat::Opus => {
                if layout.packets < 2 {
                    return Err(Error::decode("Ogg Opus needs two headers"));
                }
                let head = cursor
                    .next(bytes, &mut packet)?
                    .ok_or_else(|| Error::decode("Missing OpusHead"))?;
                let head = opus::Head::parse(head.data)?;
                limits.check(0, head.channels, 48000)?;
                let tags = cursor
                    .next(bytes, &mut packet)?
                    .ok_or_else(|| Error::decode("Missing OpusTags"))?
                    .data;
                if !tags.starts_with(b"OpusTags") {
                    return Err(Error::decode("Invalid OpusTags header"));
                }
                if tags.len() > limits.max_decoded_bytes / 4 {
                    return Err(Error::limit("Opus metadata exceeds workspace byte limit"));
                }
                let metadata = comments(&tags[8..], false)?;
                let core = opus::Core::new(
                    head,
                    limits,
                    layout.max_packet,
                    comment_len_bytes(&metadata),
                )?;
                (
                    Dsp::Opus(Box::new(core)),
                    head.channels,
                    48000,
                    5760,
                    metadata,
                )
            }
            _ => {
                return Err(Error::new(
                    crate::ErrorCode::UnsupportedFormat,
                    "Unsupported Ogg codec",
                ));
            }
        };
        let (frames, leading) = match &core {
            Dsp::Vorbis(core) => {
                timeline(bytes, cursor.clone(), &mut packet, core, layout.granule)?
            }
            Dsp::Opus(core) => {
                opus_timeline(bytes, cursor.clone(), &mut packet, core, layout.granule)?
            }
        };
        limits.check(frames, channels, rate)?;
        let tags = crate::Tags::from_native(&metadata);
        Ok(Self {
            start: cursor.clone(),
            cursor,
            packet,
            core,
            channels,
            rate,
            frames: frames as u64,
            capacity,
            info: DecodeInfo {
                format: Some(format),
                metadata,
                tags,
                ..Default::default()
            },
            produced: 0,
            leading,
            skip: leading,
            finished: false,
        })
    }
    pub fn rewind(&mut self) {
        self.cursor = self.start.clone();
        self.core.reset();
        self.produced = 0;
        self.skip = self.leading;
        self.finished = false;
    }
    pub fn refill(&mut self, bytes: &[u8], data: &mut [Vec<f32>]) -> Result<bool> {
        for c in data.iter_mut() {
            c.clear();
        }
        if self.finished {
            return Ok(false);
        }
        loop {
            let Some(packet) = self.cursor.next(bytes, &mut self.packet)? else {
                self.finished = true;
                if self.produced != self.frames {
                    return Err(Error::decode("Ogg granule exceeds decoded audio"));
                }
                return Ok(false);
            };
            let samples = self.core.decode(packet.data)?;
            let skip = self.skip.min(samples.len() / self.channels);
            self.skip -= skip;
            let samples = &samples[skip * self.channels..];
            let frames =
                (samples.len() / self.channels).min((self.frames - self.produced) as usize);
            if frames == 0 {
                continue;
            }
            for (c, out) in data.iter_mut().enumerate() {
                out.extend((0..frames).map(|i| samples[i * self.channels + c]));
            }
            self.produced += frames as u64;
            return Ok(true);
        }
    }
    /// Replay from the first audio packet; codec state and all scratch are reused.
    /// Returns the offset into the returned block at the requested frame.
    pub fn seek(&mut self, bytes: &[u8], frame: u64, data: &mut [Vec<f32>]) -> Result<usize> {
        self.rewind();
        for c in data.iter_mut() {
            c.clear();
        }
        let mut left = frame.min(self.frames);
        while left > 0 {
            if !self.refill(bytes, data)? {
                return Err(Error::decode("Ogg seek reached an early end"));
            }
            let n = data[0].len() as u64;
            if left < n {
                return Ok(left as usize);
            }
            left -= n;
        }
        for c in data.iter_mut() {
            c.clear();
        }
        Ok(0)
    }
}
/// Granules anchor packet durations, rather than controlling arbitrary trimming.
/// The first non-EOS audio anchor may specify an initial sample offset. Only the
/// last packet may be shortened by EOS; a forged small granule cannot hide work.
fn timeline(
    bytes: &[u8],
    mut cursor: Cursor,
    scratch: &mut Vec<u8>,
    core: &vorbis::Core,
    eos: u64,
) -> Result<(usize, usize)> {
    let mut previous = None;
    let mut nominal = 0u64;
    let mut origin = None;
    while let Some(packet) = cursor.next(bytes, scratch)? {
        let size = core.packet_size(packet.data)?;
        let span = previous.map_or(0, |p| (p + size) / 4);
        nominal = nominal
            .checked_add(span as u64)
            .ok_or_else(|| Error::limit("Vorbis duration overflow"))?;
        previous = Some(size);
        if let Some(granule) = packet.granule {
            if packet.eos {
                let origin = origin.unwrap_or(0i128);
                let upper = nominal as i128 + origin;
                if granule as i128 > upper || (granule as i128) < upper - span as i128 {
                    return Err(Error::decode("Vorbis EOS trim exceeds the final packet"));
                }
            } else {
                let offset = *origin.get_or_insert(granule as i128 - nominal as i128);
                if granule as i128 != nominal as i128 + offset {
                    return Err(Error::decode(
                        "Vorbis packet durations disagree with granules",
                    ));
                }
            }
        }
    }
    let origin = origin.unwrap_or(0);
    let frames = (eos as i128)
        .checked_sub(origin.max(0))
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| Error::decode("Invalid Vorbis sample origin"))?;
    let leading = usize::try_from((-origin).max(0))
        .map_err(|_| Error::limit("Vorbis initial trim overflow"))?;
    Ok((frames, leading))
}
fn comment_len_bytes(metadata: &crate::AudioMetadata) -> usize {
    metadata.iter().map(|(k, v)| 4 * (k.len() + v.len())).sum()
}
fn opus_timeline(
    bytes: &[u8],
    mut cursor: Cursor,
    scratch: &mut Vec<u8>,
    core: &opus::Core,
    eos: u64,
) -> Result<(usize, usize)> {
    let mut total = 0u64;
    let mut origin = None;
    let mut page = None;
    let mut page_start = 0;
    while let Some(packet) = cursor.next(bytes, scratch)? {
        if page != Some(packet.page_end) {
            page = Some(packet.page_end);
            page_start = total;
        }
        let span = core.duration(packet.data)? as u64;
        total = total
            .checked_add(span)
            .ok_or_else(|| Error::limit("Opus duration overflow"))?;
        if let Some(granule) = packet.granule {
            if packet.eos {
                let upper = total
                    .checked_add(origin.unwrap_or(0))
                    .ok_or_else(|| Error::limit("Opus granule overflow"))?;
                let lower = page_start + origin.unwrap_or(0);
                if granule > upper || granule < lower {
                    return Err(Error::decode("Opus EOS trim exceeds the final page"));
                }
            } else {
                let offset =
                    *origin.get_or_insert(granule.checked_sub(total).ok_or_else(|| {
                        Error::decode("Opus initial granule precedes decoded audio")
                    })?);
                if total.checked_add(offset) != Some(granule) {
                    return Err(Error::decode(
                        "Opus packet durations disagree with granules",
                    ));
                }
            }
        }
    }
    let frames = eos
        .checked_sub(origin.unwrap_or(0))
        .and_then(|n| n.checked_sub(core.pre_skip() as u64))
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| Error::decode("Opus granule precedes pre-skip"))?;
    Ok((frames, core.pre_skip()))
}
fn comments(bytes: &[u8], framing_bit: bool) -> Result<crate::AudioMetadata> {
    let mut r = crate::io::Reader::new(bytes);
    let n = r.u32(true)? as usize;
    crate::metadata::utf8(r.take(n)?)?;
    let count = r.u32(true)? as usize;
    if count > crate::metadata::MAX_METADATA_FIELDS {
        return Err(Error::limit("Ogg comment field limit exceeded"));
    }
    let mut out = crate::AudioMetadata::new();
    for _ in 0..count {
        let n = r.u32(true)? as usize;
        let value = crate::metadata::utf8(r.take(n)?)?;
        let (key, value) = value
            .split_once('=')
            .ok_or_else(|| Error::decode("Ogg comment has no key/value separator"))?;
        if key.is_empty()
            || key
                .bytes()
                .any(|b| !(0x20..=0x7d).contains(&b) || b == b'=')
        {
            return Err(Error::decode("Invalid Ogg comment key"));
        }
        out.insert(key.to_ascii_lowercase(), value.into());
    }
    if framing_bit && r.take(1)?[0] & 1 == 0 {
        return Err(Error::decode("Missing Vorbis comment framing bit"));
    }
    Ok(out)
}
pub(crate) fn decode(bytes: &[u8], format: AudioFormat, options: &DecodeOptions) -> Result<Audio> {
    let mut s = Stream::open(bytes, format, &options.limits)?;
    let mut block: Vec<Vec<f32>> = (0..s.channels)
        .map(|_| Vec::with_capacity(s.capacity))
        .collect();
    let mut output = vec![Vec::new(); s.channels];
    while s.refill(bytes, &mut block)? {
        let next = output[0].len() + block[0].len();
        options.limits.check(next, s.channels, s.rate)?;
        for (out, data) in output.iter_mut().zip(&block) {
            options.limits.reserve_output(
                out,
                next,
                s.channels,
                s.rate,
                s.frames as usize,
                "Cannot reserve Ogg PCM",
            )?;
            out.extend_from_slice(data);
        }
    }
    Audio::decoded(output, s.rate, s.info)
}
