//! Incremental decoding of WAV, FLAC and MP3 held in memory.
use crate::flac::{FlacFrame, FlacInfo, Md5};
use crate::formats::WavLayout;
use crate::mp3::{self, Header, Mp3State, Vbr};
use crate::pcm::{decode_ima_block, read_sample};
use crate::{Audio, AudioFormat, DecodeInfo, Endian, Error, ErrorCode, Limits, Result};

#[cfg(all(feature = "file-stream", not(target_family = "wasm")))]
pub(crate) mod file;

/// Options for [`Decoder`].
#[derive(Clone, Debug)]
pub struct DecoderOptions {
    /// The input's format, or `None` to detect it from the bytes.
    pub format: Option<AudioFormat>,
    /// The limits of whole decoding, checked against the declared stream when it
    /// opens. `max_decoded_bytes` also bounds the decoder's own buffers.
    pub limits: Limits,
    /// Most frames that [`Decoder::next_chunk`] returns at once. Default 4096.
    pub chunk_frames: usize,
}
impl Default for DecoderOptions {
    fn default() -> Self {
        Self {
            format: None,
            limits: Limits::default(),
            chunk_frames: 4096,
        }
    }
}

/// Decodes WAV, FLAC, MP3 or Ogg Vorbis bytes held in memory a chunk at a time.
///
/// The bytes can be anything that holds them: `&'static [u8]` from
/// `include_bytes!`, a `Vec<u8>` read from a file, or an `Arc<[u8]>` shared with
/// other decoders. Apart from them, the decoder's memory is bounded: it holds at
/// most one block of the input format (a FLAC block, an MP3 frame, an IMA ADPCM block
/// or `chunk_frames` of PCM) and the MP3 bit reservoir, all allocated when it opens.
/// [`Decoder::read`] allocates nothing, so it can run on an audio thread.
///
/// The concatenated output equals what [`decode`](crate::decode) gives for the same
/// bytes, sample for sample.
///
/// [`Decoder::seek`] moves to any frame. WAV seeks directly. FLAC starts from the
/// nearest SEEKTABLE point at or before the frame, or decodes forward from the start
/// or the current position when there is no table. MP3 restarts a few frames earlier,
/// enough to refill the bit reservoir and the filter state, so the samples after a
/// seek equal those of a whole decode. [`Decoder::rewind`] returns to the start at
/// once, for looping music.
/// Ogg Vorbis validates page framing/granules at open and prepares bounded packet
/// DSP storage. Seeking replays from the first audio packet with existing storage;
/// it is exact but O(distance from the start). Chained/multiplexed links are rejected.
///
/// ```
/// use audiobox::{Audio, AudioFormat, encode, stream::Decoder};
/// let tone: Vec<f32> = (0..10_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
/// let bytes = encode(&Audio::new(vec![tone], 44_100)?, AudioFormat::Flac)?;
///
/// let mut decoder = Decoder::new(bytes)?;
/// assert_eq!((decoder.sample_rate(), decoder.channels()), (44_100, 1));
/// let mut buffer = [0.0f32; 1024];
/// let mut frames = 0;
/// loop {
///     let n = decoder.read(&mut buffer)?;
///     if n == 0 {
///         break;
///     }
///     frames += n;
/// }
/// assert_eq!(frames, 10_000);
/// decoder.rewind()?;
/// assert_eq!(decoder.position(), 0);
/// # Ok::<(), audiobox::Error>(())
/// ```
pub struct Decoder<B> {
    bytes: B,
    format: AudioFormat,
    channels: usize,
    rate: u32,
    frames: Option<u64>,
    chunk_frames: usize,
    position: u64,
    block: Block,
    inner: Inner,
    failed: Option<(ErrorCode, String)>,
}

/// Decoded samples waiting to be read: `data[channel][start..]`.
struct Block {
    data: Vec<Vec<f32>>,
    start: usize,
}
impl Block {
    fn new(channels: usize, capacity: usize) -> Self {
        Self {
            data: (0..channels)
                .map(|_| Vec::with_capacity(capacity))
                .collect(),
            start: 0,
        }
    }
    fn clear(&mut self) {
        for c in &mut self.data {
            c.clear();
        }
        self.start = 0;
    }
    fn available(&self) -> usize {
        self.data[0].len() - self.start
    }
}

enum Inner {
    Ogg(Box<crate::ogg::Stream>),
    Wav(WavStream),
    Flac(Box<FlacStream>),
    Mp3(Box<Mp3Stream>),
}

impl<B: AsRef<[u8]>> Decoder<B> {
    pub fn new(bytes: B) -> Result<Self> {
        Self::with_options(bytes, &DecoderOptions::default())
    }
    pub fn with_options(bytes: B, options: &DecoderOptions) -> Result<Self> {
        if options.chunk_frames == 0 {
            return Err(Error::invalid("chunk_frames must be at least 1"));
        }
        let input = bytes.as_ref();
        let format = options
            .format
            .or_else(|| crate::sniff(input))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::UnsupportedFormat,
                    "Unknown or unsupported audio format",
                )
            })?;
        let limits = &options.limits;
        let (inner, channels, rate, frames, capacity) = match format {
            AudioFormat::Vorbis => {
                let s = crate::ogg::Stream::open(input, format, limits)?;
                let (channels, rate, frames, capacity) = (s.channels, s.rate, s.frames, s.capacity);
                (
                    Inner::Ogg(Box::new(s)),
                    channels,
                    rate,
                    Some(frames),
                    capacity,
                )
            }
            AudioFormat::Wav => {
                let layout = crate::formats::parse_wav(input, limits)?;
                let (channels, rate, frames) = (layout.channels, layout.rate, layout.frames);
                let stream = WavStream::new(layout, options.chunk_frames);
                let capacity = stream.block_frames();
                (
                    Inner::Wav(stream),
                    channels,
                    rate,
                    Some(frames as u64),
                    capacity,
                )
            }
            AudioFormat::Flac => {
                let info = crate::flac::parse_info(input, limits)?;
                let (channels, rate) = (info.channels, info.rate);
                let frames = (info.total != 0).then_some(info.total as u64);
                let capacity = info.max;
                // The frame buffers hold two i64 per sample besides the f32 output.
                limits.check(capacity, channels * 5, rate)?;
                let stream = FlacStream::new(info);
                (
                    Inner::Flac(Box::new(stream)),
                    channels,
                    rate,
                    frames,
                    capacity,
                )
            }
            AudioFormat::Mp3 => {
                let stream = Mp3Stream::open(input, limits)?;
                let (channels, rate) = (stream.channels, stream.rate);
                let frames = (stream.end_raw - stream.start_raw) as u64;
                (
                    Inner::Mp3(Box::new(stream)),
                    channels,
                    rate,
                    Some(frames),
                    1152,
                )
            }
            _ => {
                return Err(Error::new(
                    ErrorCode::UnsupportedFormat,
                    "Incremental decoding supports WAV, FLAC and MP3",
                ));
            }
        };
        limits.check(capacity, channels, rate)?;
        Ok(Self {
            bytes,
            format,
            channels,
            rate,
            frames,
            chunk_frames: options.chunk_frames,
            position: 0,
            block: Block::new(channels, capacity),
            inner,
            failed: None,
        })
    }
    pub fn format(&self) -> AudioFormat {
        self.format
    }
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    /// The stream's length in frames, when the container declares it or the decoder
    /// can count it on opening (always for WAV and MP3, and for FLAC with a sample count).
    pub fn frames(&self) -> Option<u64> {
        self.frames
    }
    /// The index of the next frame that [`Decoder::read`] returns.
    pub fn position(&self) -> u64 {
        self.position
    }
    /// Returns the bytes the decoder was created with.
    pub fn into_inner(self) -> B {
        self.bytes
    }
    /// Fills `out` with interleaved frames and returns how many frames were written:
    /// `out.len() / channels` unless the stream ends first, and 0 at its end. A
    /// remainder of `out` shorter than a frame is left untouched.
    ///
    /// After an error, every later call returns the same error until a seek or rewind.
    pub fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        if let Some((code, message)) = &self.failed {
            return Err(Error::new(*code, message.clone()));
        }
        let wanted = out.len() / self.channels;
        let mut written = 0;
        while written < wanted {
            if self.block.available() == 0 {
                match self.refill() {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(e) => {
                        self.position += written as u64;
                        self.failed = Some((e.code, e.message.clone()));
                        return Err(e);
                    }
                }
            }
            let n = self.block.available().min(wanted - written);
            let start = self.block.start;
            for (c, data) in self.block.data.iter().enumerate() {
                for (i, &s) in data[start..start + n].iter().enumerate() {
                    out[(written + i) * self.channels + c] = s;
                }
            }
            self.block.start += n;
            written += n;
        }
        self.position += written as u64;
        Ok(written)
    }
    /// Returns up to `chunk_frames` frames, or `None` at the end of the stream.
    pub fn next_chunk(&mut self) -> Result<Option<Audio>> {
        let mut data: Vec<Vec<f32>> = vec![Vec::with_capacity(self.chunk_frames); self.channels];
        let mut buffer = vec![0.0; self.chunk_frames.min(4096) * self.channels];
        while data[0].len() < self.chunk_frames {
            let want = (self.chunk_frames - data[0].len()).min(4096);
            let n = self.read(&mut buffer[..want * self.channels])?;
            if n == 0 {
                break;
            }
            for (c, channel) in data.iter_mut().enumerate() {
                channel.extend((0..n).map(|i| buffer[i * self.channels + c]));
            }
        }
        if data[0].is_empty() {
            return Ok(None);
        }
        let info = DecodeInfo {
            format: Some(self.format),
            ..Default::default()
        };
        Audio::decoded(data, self.rate, info).map(Some)
    }
    /// Moves to `frame`, so that the next read starts there. A frame past the end
    /// moves to the end.
    pub fn seek(&mut self, frame: u64) -> Result<()> {
        self.failed = None;
        let frame = self.frames.map_or(frame, |n| frame.min(n));
        let bytes = self.bytes.as_ref();
        let result = match &mut self.inner {
            Inner::Ogg(s) => s.seek(bytes, frame, &mut self.block.data).map(|offset| {
                self.block.start = offset;
                frame
            }),
            Inner::Wav(s) => s.seek(bytes, frame, &mut self.block),
            Inner::Flac(s) => s.seek(bytes, frame, self.position, &mut self.block),
            Inner::Mp3(s) => s.seek(bytes, frame, &mut self.block),
        };
        match result {
            Ok(position) => {
                self.position = position;
                Ok(())
            }
            Err(e) => {
                self.failed = Some((e.code, e.message.clone()));
                Err(e)
            }
        }
    }
    /// Returns to the first frame without decoding anything.
    pub fn rewind(&mut self) -> Result<()> {
        self.failed = None;
        self.block.clear();
        match &mut self.inner {
            Inner::Ogg(s) => s.rewind(),
            Inner::Wav(s) => s.rewind(),
            Inner::Flac(s) => s.rewind(),
            Inner::Mp3(s) => s.rewind(),
        }
        self.position = 0;
        Ok(())
    }
    fn refill(&mut self) -> Result<bool> {
        let bytes = self.bytes.as_ref();
        self.block.clear();
        match &mut self.inner {
            Inner::Ogg(s) => s.refill(bytes, &mut self.block.data),
            Inner::Wav(s) => s.refill(bytes, &mut self.block),
            Inner::Flac(s) => s.refill(bytes, &mut self.block),
            Inner::Mp3(s) => s.refill(bytes, &mut self.block),
        }
    }
}

impl<B> std::fmt::Debug for Decoder<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder")
            .field("format", &self.format)
            .field("channels", &self.channels)
            .field("sample_rate", &self.rate)
            .field("frames", &self.frames)
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

struct WavStream {
    layout: WavLayout,
    chunk_frames: usize,
    /// Offset of the next undecoded byte in the concatenated payload.
    next: usize,
    /// Frames decoded before `next`.
    produced: usize,
    scratch: Vec<u8>,
}
impl WavStream {
    fn new(layout: WavLayout, chunk_frames: usize) -> Self {
        let bytes = if layout.code == 17 {
            layout.align
        } else {
            chunk_frames * layout.align
        };
        Self {
            layout,
            chunk_frames,
            next: 0,
            produced: 0,
            scratch: Vec::with_capacity(bytes),
        }
    }
    fn block_frames(&self) -> usize {
        if self.layout.code == 17 {
            self.layout.samples_per_block
        } else {
            self.chunk_frames
        }
    }
    fn rewind(&mut self) {
        self.next = 0;
        self.produced = 0;
    }
    /// Copies `len` payload bytes from offset `self.next` into `scratch`, across data chunks.
    fn gather(&mut self, bytes: &[u8], len: usize) {
        self.scratch.clear();
        let mut offset = self.next;
        for range in &self.layout.payloads {
            let size = range.end - range.start;
            if offset >= size {
                offset -= size;
                continue;
            }
            let take = (size - offset).min(len - self.scratch.len());
            let from = range.start + offset;
            self.scratch.extend_from_slice(&bytes[from..from + take]);
            offset = 0;
            if self.scratch.len() == len {
                break;
            }
        }
    }
    fn refill(&mut self, bytes: &[u8], block: &mut Block) -> Result<bool> {
        let remaining = self.layout.frames - self.produced;
        if remaining == 0 {
            return Ok(false);
        }
        let layout = &self.layout;
        if layout.code == 17 {
            let align = layout.align;
            self.gather(bytes, align);
            decode_ima_block(
                &self.scratch,
                self.layout.samples_per_block,
                &mut block.data,
            )?;
            let n = self.layout.samples_per_block.min(remaining);
            for c in &mut block.data {
                c.truncate(n);
            }
            self.next += align;
            self.produced += n;
        } else {
            let n = self.chunk_frames.min(remaining);
            let (align, format) = (layout.align, layout.format);
            self.gather(bytes, n * align);
            let width = format.bytes();
            for frame in self.scratch.chunks_exact(align) {
                for (c, sample) in frame.chunks_exact(width).enumerate() {
                    block.data[c].push(read_sample(sample, format, Endian::Little)?);
                }
            }
            self.next += n * align;
            self.produced += n;
        }
        Ok(true)
    }
    fn seek(&mut self, bytes: &[u8], frame: u64, block: &mut Block) -> Result<u64> {
        block.clear();
        let frame = frame as usize;
        if self.layout.code == 17 {
            let spb = self.layout.samples_per_block;
            let index = frame / spb;
            self.next = index * self.layout.align;
            self.produced = index * spb;
            if frame > self.produced {
                self.refill(bytes, block)?;
                block.start = frame - index * spb;
            }
        } else {
            self.next = frame * self.layout.align;
            self.produced = frame;
        }
        Ok(frame as u64)
    }
}

struct FlacStream {
    info: FlacInfo,
    frame: FlacFrame,
    /// Offset of the next undecoded frame.
    pos: usize,
    /// Frames decoded before `pos`.
    written: usize,
    md5: Md5,
    /// Whether every frame since the start went through `md5`.
    from_start: bool,
}
impl FlacStream {
    fn new(info: FlacInfo) -> Self {
        Self {
            frame: FlacFrame::new(&info),
            pos: info.first_frame,
            info,
            written: 0,
            md5: Md5::new(),
            from_start: true,
        }
    }
    fn rewind(&mut self) {
        self.pos = self.info.first_frame;
        self.written = 0;
        self.md5 = Md5::new();
        self.from_start = true;
    }
    fn refill(&mut self, bytes: &[u8], block: &mut Block) -> Result<bool> {
        let total = self.info.total;
        if self.pos >= bytes.len() {
            if self.from_start {
                if total != 0 && self.written != total {
                    return Err(Error::decode("FLAC sample count mismatch"));
                }
                let md5 = std::mem::replace(&mut self.md5, Md5::new());
                if self.info.md5 != [0; 16] && md5.digest() != self.info.md5 {
                    return Err(Error::decode("FLAC PCM MD5 mismatch"));
                }
                self.from_start = false;
            }
            return Ok(false);
        }
        let (n, next_pos) = self.frame.decode(bytes, self.pos, &self.info)?;
        let next = self
            .written
            .checked_add(n)
            .ok_or_else(|| Error::limit("FLAC frame count overflow"))?;
        if total != 0 && next > total {
            return Err(Error::decode("FLAC exceeds declared sample count"));
        }
        let bits = self.info.bits;
        for (c, samples) in block.data.iter_mut().zip(&self.frame.samples) {
            c.extend(samples[..n].iter().map(|&s| crate::flac::to_float(s, bits)));
        }
        if self.from_start {
            self.md5.update(&self.frame.raw);
        }
        self.written = next;
        self.pos = next_pos;
        Ok(true)
    }
    /// The SEEKTABLE point with the largest sample at or before `frame`.
    fn seek_point(&self, bytes: &[u8], frame: u64) -> Option<(usize, usize)> {
        let table = &bytes[self.info.seek_table.clone()?];
        let mut best = None;
        for point in table.chunks_exact(18) {
            let sample = u64::from_be_bytes(point[..8].try_into().unwrap());
            let offset = u64::from_be_bytes(point[8..16].try_into().unwrap());
            if sample == u64::MAX || sample > frame {
                continue;
            }
            let pos = usize::try_from(offset)
                .ok()
                .and_then(|o| o.checked_add(self.info.first_frame))
                .filter(|&p| p < bytes.len())?;
            if best.is_none_or(|(s, _)| sample as usize > s) {
                best = Some((sample as usize, pos));
            }
        }
        best
    }
    fn seek(&mut self, bytes: &[u8], frame: u64, position: u64, block: &mut Block) -> Result<u64> {
        let block_start = position - block.start as u64;
        if frame >= block_start && frame < block_start + block.data[0].len() as u64 {
            block.start = (frame - block_start) as usize;
            return Ok(frame);
        }
        block.clear();
        if frame < self.written as u64 || frame == 0 {
            self.rewind();
        }
        if let Some((sample, pos)) = self.seek_point(bytes, frame) {
            if sample > self.written {
                self.pos = pos;
                self.written = sample;
                self.from_start = false;
            }
        }
        while (self.written as u64) <= frame && frame != 0 {
            let frame_start = self.written;
            block.clear();
            if !self.refill(bytes, block)? {
                return Ok(self.written as u64);
            }
            if self.written as u64 > frame {
                block.start = (frame - frame_start as u64) as usize;
            }
        }
        Ok(frame)
    }
}

/// Bytes of earlier main data that an MP3 frame can refer to (main_data_begin is 9 bits).
const RESERVOIR: usize = 511;
/// Recent frames remembered while looking for where to restart before a seek.
const RECENT: usize = 64;

#[derive(Clone, Copy, Default)]
struct FrameMark {
    pos: usize,
    payload: usize,
    /// Raw index of the frame's first sample in a whole decode.
    raw: usize,
    /// Samples the frame contributes in a whole decode.
    counted: usize,
    /// Reservoir length of a whole decode before the frame.
    history: usize,
}

struct Mp3Stream {
    channels: usize,
    rate: u32,
    first: usize,
    tag: Option<Vbr>,
    /// Raw indices of the first output sample and one past the last.
    start_raw: usize,
    end_raw: usize,
    state: Mp3State,
    buffer: Box<[[f32; 1152]; 2]>,
    pos: usize,
    /// Reservoir length that a whole decode would have here, to count samples as it does.
    history: usize,
    /// Raw index of the next decoded sample.
    raw: usize,
    /// Raw samples before this index are decoded only to restore state after a seek.
    discard: usize,
    flushed: bool,
}
impl Mp3Stream {
    fn open(bytes: &[u8], limits: &Limits) -> Result<Self> {
        let (first, h) = mp3::find(bytes, mp3::tag_size(bytes))
            .ok_or_else(|| Error::decode("No readable MPEG Layer III frame"))?;
        let tag = mp3::vbr(bytes, first, h)?;
        let (channels, rate) = (h.channels, h.rate);
        limits.check(0, channels, rate)?;
        if let Some(tag) = tag {
            let frames = tag
                .frames
                .checked_mul(if h.version == 1 { 1152 } else { 576 })
                .ok_or_else(|| Error::limit("MP3 sample count overflow"))?;
            limits.check(frames, channels, rate)?;
        }
        let mut stream = Self {
            channels,
            rate,
            first,
            tag,
            start_raw: 0,
            end_raw: 0,
            state: Mp3State::new(channels, rate),
            buffer: Box::new([[0.0; 1152]; 2]),
            pos: first,
            history: 0,
            raw: 0,
            discard: 0,
            flushed: false,
        };
        let mut total = 0;
        stream.walk_until(bytes, |mark| {
            total = mark.raw + mark.counted;
            true
        })?;
        if total == 0 {
            return Err(Error::decode("No decodable MP3 frames"));
        }
        let raw_total = total + mp3::FLUSH;
        limits.check(raw_total, channels, rate)?;
        let tag = tag.unwrap_or_default();
        stream.start_raw = (tag.delay + mp3::DECODER_DELAY + mp3::FLUSH).min(raw_total);
        stream.end_raw = raw_total
            .saturating_sub(tag.padding.saturating_sub(mp3::DECODER_DELAY))
            .max(stream.start_raw);
        stream.discard = stream.start_raw;
        Ok(stream)
    }
    /// Calls `visit` for each frame that a whole decode reads, without decoding, and
    /// stops at the first frame `visit` rejects. Returns that frame, if any.
    fn walk_until(
        &self,
        bytes: &[u8],
        mut visit: impl FnMut(FrameMark) -> bool,
    ) -> Result<Option<FrameMark>> {
        let (mut pos, mut history, mut raw) = (self.first, 0, 0);
        while let Some(h) = mp3::header(bytes, pos) {
            if pos + h.length > bytes.len() || h.rate != self.rate || h.channels != self.channels {
                break;
            }
            let mark = self.mark(bytes, pos, h, history, raw)?;
            if !visit(mark) {
                return Ok(Some(mark));
            }
            history = (history + mark.payload).min(4096);
            raw += mark.counted;
            pos += h.length;
        }
        Ok(None)
    }
    fn mark(
        &self,
        bytes: &[u8],
        pos: usize,
        h: Header,
        history: usize,
        raw: usize,
    ) -> Result<FrameMark> {
        let output = !(pos == self.first && self.tag.is_some());
        let back = mp3::main_data_begin(bytes, pos, h)?;
        let granules = if h.version == 1 { 2 } else { 1 };
        Ok(FrameMark {
            pos,
            payload: mp3::payload_len(h),
            raw,
            counted: if output && back <= history {
                granules * 576
            } else {
                0
            },
            history,
        })
    }
    fn rewind(&mut self) {
        self.state.reset();
        self.pos = self.first;
        self.history = 0;
        self.raw = 0;
        self.discard = self.start_raw;
        self.flushed = false;
    }
    /// Moves the samples of raw indices `raw..raw + n` that belong to the output into `block`.
    fn emit(&self, raw: usize, n: usize, block: &mut Block) {
        let from = raw.max(self.discard).max(self.start_raw);
        let to = (raw + n).min(self.end_raw);
        if from < to {
            for (c, samples) in block.data.iter_mut().zip(self.buffer.iter()) {
                c.extend_from_slice(&samples[from - raw..to - raw]);
            }
        }
    }
    fn refill(&mut self, bytes: &[u8], block: &mut Block) -> Result<bool> {
        loop {
            if self.flushed {
                return Ok(false);
            }
            let Some(h) = mp3::header(bytes, self.pos) else {
                self.state.flush(&mut self.buffer);
                self.emit(self.raw, mp3::FLUSH, block);
                self.raw += mp3::FLUSH;
                self.flushed = true;
                return Ok(block.available() > 0);
            };
            if self.pos + h.length > bytes.len() {
                return Err(Error::decode("Truncated MP3 frame"));
            }
            if h.rate != self.rate || h.channels != self.channels {
                return Err(Error::decode("MP3 stream shape changed"));
            }
            let mark = self.mark(bytes, self.pos, h, self.history, self.raw)?;
            let output = !(self.pos == self.first && self.tag.is_some());
            let n = self
                .state
                .frame(bytes, self.pos, h, output, &mut self.buffer)?;
            self.pos += h.length;
            self.history = (self.history + mark.payload).min(4096);
            self.raw += mark.counted;
            if n > 0 && mark.counted > 0 {
                self.emit(mark.raw, n, block);
                if block.available() > 0 {
                    return Ok(true);
                }
            }
        }
    }
    fn seek(&mut self, bytes: &[u8], frame: u64, block: &mut Block) -> Result<u64> {
        block.clear();
        let target = (self.start_raw + frame as usize).min(self.end_raw);
        // Find the frame holding `target` while remembering the frames before it.
        let mut recent = [FrameMark::default(); RECENT];
        let mut seen = 0usize;
        self.walk_until(bytes, |mark| {
            if mark.counted > 0 && mark.raw + mark.counted > target {
                return false;
            }
            recent[seen % RECENT] = mark;
            seen += 1;
            true
        })?;
        // Restart at the last decoded frame before the target, and early enough before
        // it that the reservoir holds everything that frame and later ones refer to.
        let mut restart = None;
        let mut previous_decoded = None;
        let mut payload = 0;
        for back in 0..seen.min(RECENT) {
            let mark = recent[(seen - 1 - back) % RECENT];
            if previous_decoded.is_none() {
                if mark.counted > 0 {
                    previous_decoded = Some(back);
                }
                continue;
            }
            payload += mark.payload;
            if payload >= RESERVOIR {
                restart = Some(mark);
                break;
            }
        }
        // Without a decoded frame before the target, or without enough remembered
        // frames to refill the reservoir, decoding restarts from the first frame.
        self.rewind();
        if let (Some(_), Some(mark)) = (previous_decoded, restart) {
            self.pos = mark.pos;
            self.history = mark.history;
            self.raw = mark.raw;
        }
        self.discard = target.max(self.start_raw);
        Ok((target - self.start_raw) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mp3_seek_restarts_shortly_before_the_target() {
        let bytes = include_bytes!("../tests/fixtures/lame-stereo.mp3");
        let mut decoder = Decoder::new(bytes.as_slice()).unwrap();
        let total = decoder.frames().unwrap();
        decoder.seek(total * 3 / 4).unwrap();
        let Inner::Mp3(stream) = &decoder.inner else {
            unreachable!()
        };
        let target = stream.start_raw + (total * 3 / 4) as usize;
        // It restarts after the first frame, and early enough to cover the reservoir.
        assert!(stream.pos > stream.first, "restarted from the first frame");
        assert!(
            stream.raw < target && target - stream.raw < 20 * 1152,
            "{} vs {target}",
            stream.raw
        );
        assert!(stream.history > 0);
    }

    #[test]
    fn flac_seek_without_table_continues_forward() {
        let tone: Vec<f32> = (0..40_000).map(|i| (i as f32 * 0.02).sin() * 0.3).collect();
        let bytes =
            crate::encode(&Audio::new(vec![tone], 8000).unwrap(), AudioFormat::Flac).unwrap();
        let mut decoder = Decoder::new(bytes.as_slice()).unwrap();
        decoder.seek(20_000).unwrap();
        let Inner::Flac(stream) = &decoder.inner else {
            unreachable!()
        };
        assert!(stream.from_start, "decoding forward keeps the MD5 check");
        let written = stream.written;
        decoder.seek(30_000).unwrap();
        let Inner::Flac(stream) = &decoder.inner else {
            unreachable!()
        };
        assert!(stream.written > written && stream.written <= 30_000 + 4096);
    }
}
