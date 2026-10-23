//! File I/O and codec state owned exclusively by the reader thread.
use super::*;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub(crate) struct Window {
    file: File,
    data: Vec<u8>,
    start: usize,
    valid: usize,
    len: usize,
}
impl Window {
    fn open(path: &Path, capacity: usize, max_file_bytes: u64) -> Result<Self> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        if size > max_file_bytes {
            return Err(Error::limit("File exceeds encoded input limit"));
        }
        let len =
            usize::try_from(size).map_err(|_| Error::limit("File size exceeds address space"))?;
        Ok(Self {
            file,
            data: vec![0; capacity],
            start: 0,
            valid: 0,
            len,
        })
    }
    fn refill(&mut self, at: usize) -> Result<()> {
        self.file.seek(SeekFrom::Start(at as u64))?;
        self.start = at;
        self.valid = 0;
        let wanted = self.data.len().min(self.len.saturating_sub(at));
        while self.valid < wanted {
            let n = self.file.read(&mut self.data[self.valid..wanted])?;
            if n == 0 {
                return Err(Error::decode("File truncated while streaming"));
            }
            self.valid += n;
        }
        Ok(())
    }
    fn get(&mut self, at: usize, count: usize) -> Result<&[u8]> {
        if count > self.data.len() {
            return Err(Error::limit("Encoded block exceeds file window"));
        }
        let end = at
            .checked_add(count)
            .filter(|&end| end <= self.len)
            .ok_or_else(|| Error::decode("Truncated file block"))?;
        if at < self.start || end > self.start + self.valid {
            self.refill(at)?;
        }
        Ok(&self.data[at - self.start..end - self.start])
    }
    fn tail(&mut self, at: usize) -> Result<&[u8]> {
        self.get(at, 1)?;
        Ok(&self.data[at - self.start..self.valid])
    }
}

enum FileCodec {
    Wav(WavStream),
    Flac(Box<FlacStream>),
    Mp3(Box<Mp3Stream>),
}
pub(crate) struct FileDecoder {
    input: Window,
    codec: FileCodec,
    block: Block,
    pub(crate) channels: usize,
    pub(crate) rate: u32,
    pub(crate) frames: Option<u64>,
    pub(crate) format: AudioFormat,
    pub(crate) position: u64,
    output_limit: u64,
    failed: Option<ErrorCode>,
}
impl FileDecoder {
    pub(crate) fn open(path: &Path, options: &crate::stream::FileStreamOptions) -> Result<Self> {
        let mut input = Window::open(path, options.encoded_buffer_bytes, options.max_file_bytes)?;
        let sniff_len = input.len.min(input.data.len()).min(8192);
        let head = input.get(0, sniff_len)?;
        let format = options
            .decoder
            .format
            .or_else(|| crate::sniff(head))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::UnsupportedFormat,
                    "Unsupported disk stream format",
                )
            })?;
        let limits = &options.decoder.limits;
        let (codec, channels, rate, frames, capacity) = match format {
            AudioFormat::Wav => {
                let layout = wav_layout(&mut input, limits, options.max_header_chunks)?;
                let (channels, rate, frames) =
                    (layout.channels, layout.rate, Some(layout.frames as u64));
                if channels > 64 {
                    return Err(Error::limit("Disk streaming supports at most 64 channels"));
                }
                let capacity = if layout.code == 17 {
                    layout.samples_per_block
                } else {
                    options.decoder.chunk_frames
                };
                check_scratch(capacity, channels, 5, options.max_decoder_bytes)?;
                let bytes = if layout.code == 17 {
                    layout.align
                } else {
                    capacity
                        .checked_mul(layout.align)
                        .ok_or_else(|| Error::limit("WAVE scratch overflow"))?
                };
                if bytes > options.max_decoder_bytes || layout.align > options.encoded_buffer_bytes
                {
                    return Err(Error::limit("WAVE scratch/alignment exceeds budget"));
                }
                let stream = WavStream::new(layout, options.decoder.chunk_frames);
                let capacity = stream.block_frames();
                (FileCodec::Wav(stream), channels, rate, frames, capacity)
            }
            AudioFormat::Flac => {
                let info = flac_info(&mut input, limits, options.max_header_chunks)?;
                let (channels, rate, frames, capacity) = (
                    info.channels,
                    info.rate,
                    (info.total != 0).then_some(info.total as u64),
                    info.max,
                );
                check_scratch(capacity, channels, 5, options.max_decoder_bytes)?;
                (
                    FileCodec::Flac(Box::new(FlacStream::new(info))),
                    channels,
                    rate,
                    frames,
                    capacity,
                )
            }
            AudioFormat::Mp3 => {
                check_scratch(1152, 2, 5, options.max_decoder_bytes)?;
                let stream = mp3_open(&mut input, limits)?;
                let (channels, rate, frames) = (
                    stream.channels,
                    stream.rate,
                    Some((stream.end_raw - stream.start_raw) as u64),
                );
                (
                    FileCodec::Mp3(Box::new(stream)),
                    channels,
                    rate,
                    frames,
                    1152,
                )
            }
            _ => {
                return Err(Error::new(
                    ErrorCode::UnsupportedFormat,
                    "Disk streaming supports WAV, FLAC and MP3",
                ));
            }
        };
        limits.check(capacity, channels, rate)?;
        check_scratch(capacity, channels, 5, options.max_decoder_bytes)?;
        if let FileCodec::Wav(w) = &codec {
            if w.layout.align > options.encoded_buffer_bytes {
                return Err(Error::limit("WAVE alignment exceeds encoded window"));
            }
            let bytes = if w.layout.code == 17 {
                w.layout.align
            } else {
                capacity
                    .checked_mul(w.layout.align)
                    .ok_or_else(|| Error::limit("WAVE scratch overflow"))?
            };
            if bytes > options.max_decoder_bytes {
                return Err(Error::limit("WAVE scratch exceeds budget"));
            }
        }
        Ok(Self {
            input,
            codec,
            block: Block::new(channels, capacity),
            channels,
            rate,
            frames,
            format,
            position: 0,
            output_limit: frame_limit(limits, channels, rate) as u64,
            failed: None,
        })
    }
    pub(crate) fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        if let Some(code) = self.failed {
            return Err(Error {
                code,
                message: String::new(),
            });
        }
        let wanted = out.len() / self.channels;
        let mut written = 0;
        while written < wanted {
            if self.block.available() == 0 {
                self.block.clear();
                match self.refill() {
                    Ok(true) => (),
                    Ok(false) => break,
                    Err(e) => {
                        self.failed = Some(e.code);
                        if written == 0 {
                            return Err(e);
                        }
                        break;
                    }
                }
            }
            let remaining = self
                .output_limit
                .saturating_sub(self.position + written as u64);
            let n = self
                .block
                .available()
                .min(wanted - written)
                .min(remaining as usize);
            if n == 0 {
                self.failed = Some(ErrorCode::LimitExceeded);
                if written == 0 {
                    return Err(Error::limit("Stream output exceeds decode limits"));
                }
                break;
            }
            for c in 0..self.channels {
                for i in 0..n {
                    out[(written + i) * self.channels + c] =
                        self.block.data[c][self.block.start + i];
                }
            }
            self.block.start += n;
            written += n;
        }
        self.position += written as u64;
        Ok(written)
    }
    fn refill(&mut self) -> Result<bool> {
        let (input, block) = (&mut self.input, &mut self.block);
        match &mut self.codec {
            FileCodec::Wav(s) => {
                let remaining = s.layout.frames - s.produced;
                if remaining == 0 {
                    return Ok(false);
                }
                let n = if s.layout.code == 17 {
                    s.layout.samples_per_block.min(remaining)
                } else {
                    s.chunk_frames.min(remaining)
                };
                let bytes = if s.layout.code == 17 {
                    s.layout.align
                } else {
                    n * s.layout.align
                };
                s.scratch.clear();
                let mut offset = s.next;
                for range in &s.layout.payloads {
                    if offset >= range.len() {
                        offset -= range.len();
                        continue;
                    }
                    let mut from = range.start + offset;
                    let end = from + (range.end - from).min(bytes - s.scratch.len());
                    while from < end {
                        let count = (end - from).min(input.data.len());
                        s.scratch.extend_from_slice(input.get(from, count)?);
                        from += count;
                    }
                    offset = 0;
                    if s.scratch.len() == bytes {
                        break;
                    }
                }
                if s.layout.code == 17 {
                    decode_ima_block(&s.scratch, s.layout.samples_per_block, &mut block.data)?;
                    for c in &mut block.data {
                        c.truncate(n);
                    }
                } else {
                    for frame in s.scratch.chunks_exact(s.layout.align) {
                        for (c, sample) in frame.chunks_exact(s.layout.format.bytes()).enumerate() {
                            block.data[c].push(read_sample(
                                sample,
                                s.layout.format,
                                Endian::Little,
                            )?);
                        }
                    }
                }
                s.next += bytes;
                s.produced += n;
                Ok(true)
            }
            FileCodec::Flac(s) => {
                if s.pos >= input.len {
                    if s.from_start {
                        if s.info.total != 0 && s.written != s.info.total {
                            return Err(Error::decode("FLAC sample count mismatch"));
                        }
                        let md5 = std::mem::replace(&mut s.md5, Md5::new());
                        if s.info.md5 != [0; 16] && md5.digest() != s.info.md5 {
                            return Err(Error::decode("FLAC PCM MD5 mismatch"));
                        }
                        s.from_start = false;
                    }
                    return Ok(false);
                }
                let available = input.tail(s.pos)?.len();
                let mut result = s.frame.decode(input.tail(s.pos)?, 0, &s.info);
                // One bounded refill handles a frame split at the window boundary.
                // Persistent syntax/CRC errors remain errors; there is no resync scan.
                if result.is_err() && available < input.data.len() && s.pos + available < input.len
                {
                    input.refill(s.pos)?;
                    result = s.frame.decode(input.tail(s.pos)?, 0, &s.info);
                }
                let (n, consumed) = result?;
                let next = s
                    .written
                    .checked_add(n)
                    .ok_or_else(|| Error::limit("FLAC frame count overflow"))?;
                if s.info.total != 0 && next > s.info.total {
                    return Err(Error::decode("FLAC exceeds declared sample count"));
                }
                for (out, samples) in block.data.iter_mut().zip(&s.frame.samples) {
                    out.extend(
                        samples[..n]
                            .iter()
                            .map(|&v| crate::flac::to_float(v, s.info.bits)),
                    );
                }
                if s.from_start {
                    s.md5.update(&s.frame.raw);
                }
                s.written = next;
                s.pos += consumed;
                Ok(true)
            }
            FileCodec::Mp3(s) => loop {
                if s.flushed {
                    return Ok(false);
                }
                let h = if input.len.saturating_sub(s.pos) < 4 {
                    None
                } else {
                    mp3::header(input.get(s.pos, 4)?, 0)
                };
                let Some(h) = h else {
                    s.state.flush(&mut s.buffer);
                    s.emit(s.raw, mp3::FLUSH, block);
                    s.raw += mp3::FLUSH;
                    s.flushed = true;
                    return Ok(block.available() > 0);
                };
                if h.rate != s.rate || h.channels != s.channels {
                    return Err(Error::decode("MP3 stream shape changed"));
                }
                let data = input.get(s.pos, h.length)?;
                let output = !(s.pos == s.first && s.tag.is_some());
                let counted = mp3_count(data, h, output, s.history)?;
                let raw = s.raw;
                let n = s.state.frame(data, 0, h, output, &mut s.buffer)?;
                s.pos += h.length;
                s.history = (s.history + mp3::payload_len(h)).min(4096);
                s.raw += counted;
                if n > 0 && counted > 0 {
                    s.emit(raw, n, block);
                    if block.available() > 0 {
                        return Ok(true);
                    }
                }
            },
        }
    }
    /// Replay seeks run only on the reader thread, with cancellation between blocks.
    pub(crate) fn seek(&mut self, frame: u64, mut cancelled: impl FnMut() -> bool) -> Result<()> {
        self.block.clear();
        self.failed = None;
        self.position = 0;
        match &mut self.codec {
            FileCodec::Wav(s) => s.rewind(),
            FileCodec::Flac(s) => s.rewind(),
            FileCodec::Mp3(s) => s.rewind(),
        }
        let target = self.frames.map_or(frame, |n| frame.min(n));
        if target > self.output_limit {
            self.failed = Some(ErrorCode::LimitExceeded);
            return Err(Error::limit("Seek exceeds output limit"));
        }
        if let FileCodec::Wav(s) = &mut self.codec {
            let at = target as usize;
            let base = if s.layout.code == 17 {
                at / s.layout.samples_per_block * s.layout.samples_per_block
            } else {
                at
            };
            s.produced = base;
            s.next = if s.layout.code == 17 {
                base / s.layout.samples_per_block * s.layout.align
            } else {
                base * s.layout.align
            };
            self.position = base as u64;
        }
        while self.position < target {
            if cancelled() {
                return Ok(());
            }
            if self.block.available() == 0 {
                self.block.clear();
                if !self.refill()? {
                    break;
                }
            }
            let n = (target - self.position).min(self.block.available() as u64) as usize;
            self.block.start += n;
            self.position += n as u64;
        }
        Ok(())
    }
}
fn frame_limit(limits: &Limits, channels: usize, rate: u32) -> usize {
    let (mut low, mut high) = (
        0,
        limits
            .max_frames
            .min(limits.max_decoded_bytes / (4 * channels)),
    );
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if mid as f64 / rate as f64 <= limits.max_duration_seconds {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}
fn check_scratch(frames: usize, channels: usize, factor: usize, budget: usize) -> Result<()> {
    if frames
        .checked_mul(channels)
        .and_then(|n| n.checked_mul(4 * factor))
        .and_then(|n| n.checked_add(128 * 1024))
        .is_none_or(|n| n > budget)
    {
        return Err(Error::limit("Decoder scratch exceeds file-stream budget"));
    }
    Ok(())
}
fn wav_layout(input: &mut Window, limits: &Limits, max_chunks: usize) -> Result<WavLayout> {
    let head = input.get(0, 12)?;
    let rf64 = &head[..4] != b"RIFF";
    if ![b"RIFF".as_slice(), b"RF64", b"BW64"].contains(&&head[..4]) || &head[8..12] != b"WAVE" {
        return Err(Error::decode("Invalid WAVE header"));
    }
    let end = if rf64 {
        input.len
    } else {
        (u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize)
            .checked_add(8)
            .ok_or_else(|| Error::decode("RIFF size overflow"))?
    };
    if end > input.len {
        return Err(Error::decode("Truncated RIFF container"));
    }
    let (mut at, mut chunks) = (12, 0);
    let (mut desc, mut fact, mut data64) = (None, None, None);
    let mut payloads = Vec::new();
    while at < end {
        chunks += 1;
        if chunks > max_chunks {
            return Err(Error::limit("Too many WAVE header chunks"));
        }
        if end - at < 8 {
            return Err(Error::decode("Truncated WAVE chunk header"));
        }
        let h = input.get(at, 8)?;
        let id: [u8; 4] = h[..4].try_into().unwrap();
        let n = u32::from_le_bytes(h[4..8].try_into().unwrap());
        let n = if rf64 && &id == b"data" && n == u32::MAX {
            data64.ok_or_else(|| Error::decode("Missing RF64 ds64"))?
        } else {
            n as usize
        };
        at += 8;
        if n > end - at {
            return Err(Error::decode("WAVE chunk exceeds container"));
        }
        match &id {
            b"fmt " => desc = Some(crate::formats::wav_description(input.get(at, n.min(64))?)?),
            b"ds64" => {
                if n < 16 {
                    return Err(Error::decode("Truncated ds64"));
                }
                data64 = Some(
                    usize::try_from(u64::from_le_bytes(
                        input.get(at + 8, 8)?.try_into().unwrap(),
                    ))
                    .map_err(|_| Error::limit("RF64 size overflow"))?,
                );
            }
            b"fact" if n >= 4 => {
                fact = Some(u32::from_le_bytes(input.get(at, 4)?.try_into().unwrap()) as usize)
            }
            b"data" => payloads.push(at..at + n),
            _ => (),
        }
        at += n;
        if n % 2 != 0 && at < end {
            at += 1;
        }
    }
    crate::formats::wav_layout(desc, payloads, fact, crate::AudioMetadata::new(), limits)
}
fn flac_info(input: &mut Window, limits: &Limits, max_chunks: usize) -> Result<FlacInfo> {
    if input.get(0, 4)? != b"fLaC" {
        return Err(Error::decode("Invalid FLAC magic"));
    }
    let (mut at, mut stream) = (4, None);
    for _ in 0..max_chunks {
        let h = input.get(at, 4)?;
        let last = h[0] & 128 != 0;
        let kind = h[0] & 127;
        let n = ((h[1] as usize) << 16) | ((h[2] as usize) << 8) | h[3] as usize;
        at += 4;
        if n > input.len.saturating_sub(at) {
            return Err(Error::decode("Truncated FLAC metadata"));
        }
        if kind == 0 {
            if n != 34 || stream.is_some() {
                return Err(Error::decode("Invalid FLAC STREAMINFO"));
            }
            let mut header = b"fLaC\x80\0\0\x22".to_vec();
            header.extend_from_slice(input.get(at, 34)?);
            let max_frame =
                ((header[15] as usize) << 16) | ((header[16] as usize) << 8) | header[17] as usize;
            if max_frame > input.data.len() {
                return Err(Error::limit("FLAC maximum encoded frame exceeds window"));
            }
            stream = Some(crate::flac::parse_info(&header, limits)?);
        }
        at += n;
        if last {
            let mut info = stream.ok_or_else(|| Error::decode("Missing FLAC STREAMINFO"))?;
            info.first_frame = at;
            return Ok(info);
        }
    }
    Err(Error::limit("Too many FLAC metadata blocks"))
}
fn mp3_count(data: &[u8], h: Header, output: bool, history: usize) -> Result<usize> {
    Ok(if output && mp3::main_data_begin(data, 0, h)? <= history {
        if h.version == 1 { 1152 } else { 576 }
    } else {
        0
    })
}
fn mp3_open(input: &mut Window, limits: &Limits) -> Result<Mp3Stream> {
    let mut first = mp3::tag_size(input.get(0, input.len.min(10))?);
    let h = loop {
        if input.len.saturating_sub(first) < 4 {
            return Err(Error::decode("No readable MPEG Layer III frame"));
        }
        if let Some(h) = mp3::header(input.get(first, 4)?, 0) {
            if h.length <= input.len - first
                && (input.len - (first + h.length) < 4
                    || input.get(first + h.length, 3)? == b"TAG"
                    || mp3::header(input.get(first + h.length, 4)?, 0)
                        .is_some_and(|next| next.rate == h.rate && next.version == h.version))
            {
                break h;
            }
        }
        first += 1;
    };
    let tag = mp3::vbr(input.get(first, h.length)?, 0, h)?;
    limits.check(0, h.channels, h.rate)?;
    if let Some(tag) = tag {
        let frames = tag
            .frames
            .checked_mul(if h.version == 1 { 1152 } else { 576 })
            .ok_or_else(|| Error::limit("MP3 declared count overflow"))?;
        limits.check(frames, h.channels, h.rate)?;
    }
    let (mut at, mut history, mut total) = (first, 0, 0usize);
    while input.len.saturating_sub(at) >= 4 {
        let Some(next) = mp3::header(input.get(at, 4)?, 0) else {
            break;
        };
        if next.length > input.len - at || next.rate != h.rate || next.channels != h.channels {
            break;
        }
        let count = mp3_count(
            input.get(at, next.length)?,
            next,
            !(at == first && tag.is_some()),
            history,
        )?;
        total = total
            .checked_add(count)
            .ok_or_else(|| Error::limit("MP3 frame count overflow"))?;
        limits.check(total, h.channels, h.rate)?;
        history = (history + mp3::payload_len(next)).min(4096);
        at += next.length;
    }
    if total == 0 {
        return Err(Error::decode("No decodable MP3 frames"));
    }
    let raw_total = total
        .checked_add(mp3::FLUSH)
        .ok_or_else(|| Error::limit("MP3 frame count overflow"))?;
    limits.check(raw_total, h.channels, h.rate)?;
    let trim = tag.unwrap_or_default();
    let start_raw = (trim.delay + mp3::DECODER_DELAY + mp3::FLUSH).min(raw_total);
    let end_raw = raw_total
        .saturating_sub(trim.padding.saturating_sub(mp3::DECODER_DELAY))
        .max(start_raw);
    Ok(Mp3Stream {
        channels: h.channels,
        rate: h.rate,
        first,
        tag,
        start_raw,
        end_raw,
        state: Mp3State::new(h.channels, h.rate),
        buffer: Box::new([[0.0; 1152]; 2]),
        pos: first,
        history: 0,
        raw: 0,
        discard: start_raw,
        flushed: false,
    })
}
