#![allow(clippy::needless_range_loop)]
use crate::io::{BitWriter, Bits, Reader};
use crate::{
    Audio, AudioFormat, DecodeInfo, DecodeOptions, EncodeOptions, Error, Result, SampleFormat,
};

fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in bytes {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 128 != 0 {
                (crc << 1) ^ 7
            } else {
                crc << 1
            };
        }
    }
    crc
}
fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in bytes {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}
/// Counts the zeros before the next one bit and consumes them and the one, up to 32
/// bits at a time. Errors as reading bit by bit would: truncated data before the
/// one, or more than 2^24 zeros, whichever comes first.
fn unary(br: &mut Bits<'_>) -> Result<u64> {
    let mut n = 0u64;
    loop {
        let take = br.remaining().min(32);
        if take == 0 {
            return Err(Error::decode("Truncated bitstream"));
        }
        let window = br.peek(take);
        let zeros = (window.leading_zeros() as usize)
            .saturating_sub(32 - take)
            .min(take);
        n += zeros as u64;
        if n > 1 << 24 {
            return Err(Error::decode("Runaway FLAC unary code"));
        }
        if zeros < take {
            br.pos += zeros + 1;
            return Ok(n);
        }
        br.pos += take;
    }
}
fn number(br: &mut Bits<'_>) -> Result<u64> {
    let first = br.read(8)? as u8;
    if first & 128 == 0 {
        return Ok(first as u64);
    }
    let count = first.leading_ones() as usize;
    if !(2..=7).contains(&count) {
        return Err(Error::decode("Invalid FLAC frame number"));
    }
    let mut n = (first & (0x7f >> count)) as u64;
    for _ in 1..count {
        let b = br.read(8)?;
        if b & 0xc0 != 0x80 {
            return Err(Error::decode("Invalid FLAC frame number continuation"));
        }
        n = (n << 6) | (b & 63) as u64;
    }
    Ok(n)
}
fn write_number(w: &mut BitWriter, n: u64) {
    if n < 128 {
        w.write(n, 8);
        return;
    }
    let mut count = 2;
    while count < 7 && n >= (1u64 << (5 * count + 1)) {
        count += 1;
    }
    let lead = (((0xffu16 << (8 - count)) & 255) as u64) | (n >> (6 * (count - 1)));
    w.write(lead, 8);
    for i in (0..count - 1).rev() {
        w.write(0x80 | ((n >> (6 * i)) & 63), 8);
    }
}
const FIXED: [&[i64]; 5] = [&[], &[1], &[2, -1], &[3, -3, 1], &[4, -6, 4, -1]];
fn residual(br: &mut Bits<'_>, out: &mut [i64], order: usize) -> Result<()> {
    let method = br.read(2)?;
    if method > 1 {
        return Err(Error::decode("Reserved FLAC residual method"));
    }
    let bits = if method == 0 { 4 } else { 5 };
    let escape = (1 << bits) - 1;
    let parts = 1usize << br.read(4)?;
    if out.len() % parts != 0 || out.len() / parts < order {
        return Err(Error::decode("Invalid FLAC residual partition"));
    }
    let count = out.len() / parts;
    let mut index = order;
    for p in 0..parts {
        let n = if p == 0 { count - order } else { count };
        let k = br.read(bits)?;
        if k == escape {
            let width = br.read(5)? as usize;
            for _ in 0..n {
                out[index] = br.signed(width)? as i64;
                index += 1;
            }
        } else {
            for _ in 0..n {
                let q = unary(br)?;
                let v = (q << k) | br.read(k as usize)? as u64;
                if v > u32::MAX as u64 * 2 {
                    return Err(Error::decode("FLAC residual overflow"));
                }
                out[index] = ((v >> 1) as i64) ^ -((v & 1) as i64);
                index += 1;
            }
        }
    }
    Ok(())
}
fn signed(br: &mut Bits<'_>, n: usize) -> Result<i64> {
    if n <= 32 {
        Ok(br.signed(n)? as i64)
    } else if n == 33 {
        let high = br.signed(1)? as i64;
        Ok((high << 32) | br.read(32)? as i64)
    } else {
        Err(Error::decode("Invalid FLAC bit depth"))
    }
}
fn subframe(br: &mut Bits<'_>, out: &mut [i64], bits: usize) -> Result<()> {
    let n = out.len();
    if br.read(1)? != 0 {
        return Err(Error::decode("Invalid FLAC subframe padding"));
    }
    let kind = br.read(6)?;
    let wasted = if br.read(1)? != 0 {
        unary(br)? as usize + 1
    } else {
        0
    };
    let effective = bits
        .checked_sub(wasted)
        .filter(|&n| n > 0 && n <= 33)
        .ok_or_else(|| Error::decode("Invalid FLAC wasted bits"))?;
    match kind {
        0 => {
            let v = signed(br, effective)?;
            out.fill(v);
        }
        1 => {
            for v in out.iter_mut() {
                *v = signed(br, effective)?;
            }
        }
        8..=12 | 32..=63 => {
            let order = if kind < 32 {
                (kind - 8) as usize
            } else {
                (kind - 31) as usize
            };
            if order > n {
                return Err(Error::decode("FLAC predictor exceeds block"));
            }
            for v in &mut out[..order] {
                *v = signed(br, effective)?;
            }
            let mut coeff = [0i64; 32];
            let shift = if kind < 32 {
                coeff[..order].copy_from_slice(FIXED[order]);
                0
            } else {
                let precision = br.read(4)? as usize + 1;
                if precision == 16 {
                    return Err(Error::decode("Reserved FLAC LPC precision"));
                }
                let shift = br.signed(5)?;
                if shift < 0 {
                    return Err(Error::decode("Negative FLAC LPC shift"));
                }
                for c in &mut coeff[..order] {
                    *c = br.signed(precision)? as i64;
                }
                shift as u32
            };
            residual(br, out, order)?;
            for i in order..n {
                let mut prediction = 0i64;
                for (j, &coef) in coeff[..order].iter().enumerate() {
                    prediction = prediction
                        .checked_add(
                            coef.checked_mul(out[i - j - 1])
                                .ok_or_else(|| Error::decode("FLAC prediction overflow"))?,
                        )
                        .ok_or_else(|| Error::decode("FLAC prediction overflow"))?;
                }
                out[i] = out[i]
                    .checked_add(prediction >> shift)
                    .ok_or_else(|| Error::decode("FLAC sample overflow"))?;
            }
        }
        _ => return Err(Error::decode("Reserved FLAC subframe")),
    }
    for s in out.iter_mut() {
        *s = s
            .checked_mul(1i64 << wasted)
            .ok_or_else(|| Error::decode("FLAC wasted bits overflow"))?;
    }
    Ok(())
}

/// The STREAMINFO shape and the parts of the metadata that decoding needs.
pub(crate) struct FlacInfo {
    pub rate: u32,
    pub channels: usize,
    pub bits: usize,
    /// Total frames, or 0 if unknown.
    pub total: usize,
    /// Largest block size.
    pub max: usize,
    pub md5: [u8; 16],
    pub metadata: crate::AudioMetadata,
    pub cover: Option<crate::CoverArt>,
    /// Offset of the first audio frame.
    pub first_frame: usize,
    /// Byte range of the SEEKTABLE block's points, if any.
    pub seek_table: Option<std::ops::Range<usize>>,
}
impl FlacInfo {
    pub fn sample_format(&self) -> SampleFormat {
        if self.bits <= 16 {
            SampleFormat::S16
        } else if self.bits <= 24 {
            SampleFormat::S24
        } else {
            SampleFormat::S32
        }
    }
}

pub(crate) fn parse_info(bytes: &[u8], limits: &crate::Limits) -> Result<FlacInfo> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != b"fLaC" {
        return Err(Error::decode("Invalid FLAC magic"));
    }
    let mut stream = None;
    let mut metadata = crate::AudioMetadata::new();
    let mut cover = None;
    let mut metadata_budget = crate::metadata::Budget::default();
    let mut md5 = [0u8; 16];
    let mut seek_table = None;
    loop {
        let header = r.take(4)?;
        let last = header[0] & 128 != 0;
        let kind = header[0] & 127;
        let n = ((header[1] as usize) << 16) | ((header[2] as usize) << 8) | header[3] as usize;
        let at = r.pos;
        let b = r.take(n)?;
        if kind == 0 {
            if b.len() != 34 || stream.is_some() {
                return Err(Error::decode("Invalid FLAC STREAMINFO"));
            }
            let max = u16::from_be_bytes(b[2..4].try_into().unwrap()) as usize;
            let mut br = Bits::new(&b[10..18]);
            let rate = br.read(20)?;
            let channels = br.read(3)? as usize + 1;
            let bits = br.read(5)? as usize + 1;
            let frames = ((br.read(4)? as u64) << 32) | br.read(32)? as u64;
            let frames =
                usize::try_from(frames).map_err(|_| Error::limit("FLAC sample count overflow"))?;
            limits.check(frames, channels, rate)?;
            if !(4..=32).contains(&bits) || max == 0 {
                return Err(Error::decode("Invalid FLAC stream shape"));
            }
            md5.copy_from_slice(&b[18..34]);
            stream = Some((rate, channels, bits, frames, max));
        } else if kind == 3 {
            seek_table = Some(at..at + n - n % 18);
        } else if kind == 4 {
            metadata_budget.add(b.len())?;
            let mut tags = Reader::new(b);
            let vendor = tags.u32(true)? as usize;
            tags.take(vendor)?;
            let count = tags.u32(true)?;
            for _ in 0..count {
                metadata_budget.field()?;
                let n = tags.u32(true)? as usize;
                let tag = crate::metadata::utf8(tags.take(n)?)?;
                if let Some((key, value)) = tag.split_once('=') {
                    metadata.insert(key.to_ascii_lowercase(), value.to_string());
                }
            }
            if tags.pos != b.len() {
                return Err(Error::decode("Trailing FLAC comment data"));
            }
        } else if kind == 6 {
            metadata_budget.add(b.len())?;
            let image = crate::metadata::read_flac_picture(b)?;
            if cover.is_none() {
                cover = image;
            }
        }
        if last {
            break;
        }
    }
    let (rate, channels, bits, total, max) =
        stream.ok_or_else(|| Error::decode("Missing FLAC STREAMINFO"))?;
    Ok(FlacInfo {
        rate,
        channels,
        bits,
        total,
        max,
        md5,
        metadata,
        cover,
        first_frame: r.pos,
        seek_table,
    })
}

/// Decodes one audio frame at a time into buffers sized for the largest block.
pub(crate) struct FlacFrame {
    /// Samples of each channel, `max` long; the first `n` are valid after a decode.
    pub samples: Vec<Vec<i64>>,
    /// Little-endian PCM of the last frame, for the MD5 check.
    pub raw: Vec<u8>,
}
impl FlacFrame {
    pub fn new(info: &FlacInfo) -> Self {
        Self {
            samples: vec![vec![0; info.max]; info.channels],
            raw: Vec::with_capacity(info.max * info.channels * info.bits.div_ceil(8)),
        }
    }
    /// Decodes the frame at `start`, checking both CRCs and the stream shape.
    /// Returns its block size and the offset of the next frame.
    pub fn decode(
        &mut self,
        bytes: &[u8],
        start: usize,
        info: &FlacInfo,
    ) -> Result<(usize, usize)> {
        let (rate, channels, bits, max) = (info.rate, info.channels, info.bits, info.max);
        let mut br = Bits::new(bytes.get(start..).unwrap_or_default());
        if br.read(14)? != 0x3ffe || br.read(1)? != 0 {
            return Err(Error::decode("Invalid FLAC frame sync"));
        }
        br.read(1)?;
        let bc = br.read(4)?;
        let rc = br.read(4)?;
        let assign = br.read(4)?;
        let sc = br.read(3)?;
        if br.read(1)? != 0 || assign > 10 || sc == 3 {
            return Err(Error::decode("Reserved FLAC frame field"));
        }
        number(&mut br)?;
        let n = match bc {
            1 => 192,
            2..=5 => 576usize << (bc - 2),
            6 => br.read(8)? as usize + 1,
            7 => br.read(16)? as usize + 1,
            8..=15 => 256usize << (bc - 8),
            _ => return Err(Error::decode("Reserved FLAC block size")),
        };
        let frame_rate = match rc {
            0 => rate,
            1 => 88200,
            2 => 176400,
            3 => 192000,
            4 => 8000,
            5 => 16000,
            6 => 22050,
            7 => 24000,
            8 => 32000,
            9 => 44100,
            10 => 48000,
            11 => 96000,
            12 => br.read(8)? * 1000,
            13 => br.read(16)?,
            14 => br.read(16)? * 10,
            _ => return Err(Error::decode("Reserved FLAC sample rate")),
        };
        let frame_bits = if sc == 0 {
            bits
        } else {
            [0, 8, 12, 0, 16, 20, 24, 32][sc as usize]
        };
        let frame_channels = if assign < 8 { assign as usize + 1 } else { 2 };
        if frame_rate != rate || frame_channels != channels || frame_bits != bits || n > max {
            return Err(Error::decode("FLAC frame differs from STREAMINFO"));
        }
        let header_len = br.pos / 8;
        let checksum = br.read(8)? as u8;
        if crc8(&bytes[start..start + header_len]) != checksum {
            return Err(Error::decode("FLAC header CRC mismatch"));
        }
        for (c, samples) in self.samples.iter_mut().enumerate() {
            let extra =
                (assign == 8 && c == 1) || (assign == 9 && c == 0) || (assign == 10 && c == 1);
            subframe(&mut br, &mut samples[..n], bits + usize::from(extra))?;
        }
        if assign >= 8 {
            let (left, right) = self.samples.split_at_mut(1);
            let (left, right) = (&mut left[0], &mut right[0]);
            for i in 0..n {
                let a = left[i];
                let b = right[i];
                match assign {
                    8 => right[i] = a - b,
                    9 => left[i] = a + b,
                    10 => {
                        let mid = (a << 1) | (b & 1);
                        left[i] = (mid + b) >> 1;
                        right[i] = (mid - b) >> 1;
                    }
                    _ => unreachable!(),
                }
            }
        }
        br.align();
        let end = br.pos / 8;
        let crc = br.read(16)? as u16;
        if crc16(&bytes[start..start + end]) != crc {
            return Err(Error::decode("FLAC frame CRC mismatch"));
        }
        self.raw.clear();
        let width = bits.div_ceil(8);
        for i in 0..n {
            for c in 0..channels {
                let s = self.samples[c][i];
                if s < -(1i64 << (bits - 1)) || s >= (1i64 << (bits - 1)) {
                    return Err(Error::decode("FLAC sample exceeds bit depth"));
                }
                // The low `width` bytes, little-endian: what MD5 hashes.
                self.raw.extend_from_slice(&s.to_le_bytes()[..width]);
            }
        }
        Ok((n, start + br.pos / 8))
    }
}

pub(crate) fn to_float(sample: i64, bits: usize) -> f32 {
    (sample as f64 / (1u64 << (bits - 1)) as f64) as f32
}

pub(crate) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Audio> {
    let info = parse_info(bytes, &options.limits)?;
    let (rate, channels, total) = (info.rate, info.channels, info.total);
    let mut frame = FlacFrame::new(&info);
    let mut out = vec![Vec::<f32>::new(); channels];
    let mut pcm_hash = Md5::new();
    let mut written = 0usize;
    let mut pos = info.first_frame;
    while pos < bytes.len() {
        let (n, next_pos) = frame.decode(bytes, pos, &info)?;
        let next = written
            .checked_add(n)
            .ok_or_else(|| Error::limit("FLAC frame count overflow"))?;
        options.limits.check(next, channels, rate)?;
        if total != 0 && next > total {
            return Err(Error::decode("FLAC exceeds declared sample count"));
        }
        // A declared total is a hard bound (more samples are an error above), so
        // growth stops there: such a stream ends with an exactly sized output.
        let cap = if total != 0 { total } else { usize::MAX };
        for (c, samples) in out.iter_mut().zip(&frame.samples) {
            options.limits.reserve_output(
                c,
                next,
                channels,
                rate,
                cap,
                "Cannot allocate FLAC output",
            )?;
            c.extend(samples[..n].iter().map(|&s| to_float(s, info.bits)));
        }
        pcm_hash.update(&frame.raw);
        written = next;
        pos = next_pos;
    }
    if total != 0 && written != total {
        return Err(Error::decode("FLAC sample count mismatch"));
    }
    if info.md5 != [0; 16] && pcm_hash.digest() != info.md5 {
        return Err(Error::decode("FLAC PCM MD5 mismatch"));
    }
    let sample_format = info.sample_format();
    let mut tags = crate::Tags::from_native(&info.metadata);
    tags.cover = info.cover;
    Audio::decoded(
        out,
        rate,
        DecodeInfo {
            format: Some(AudioFormat::Flac),
            sample_format: Some(sample_format),
            metadata: info.metadata,
            tags,
        },
    )
}
fn zigzag(n: i64) -> u64 {
    ((n << 1) ^ (n >> 63)) as u64
}
fn sub_writer(samples: &[i64], bits: usize, level: u8, w: &mut BitWriter) {
    // Repeat bit-exact selection without inserting alignment between channels.
    if samples.iter().all(|&s| s == samples[0]) {
        w.write(0, 8);
        w.write(samples[0] as u64, bits);
        return;
    }
    let raw = 8 + samples.len() * bits;
    let mut best = (raw, 0, 0, Vec::new());
    for (order, coef) in FIXED
        .iter()
        .enumerate()
        .take(if level == 0 { 1 } else { 5 })
    {
        if order >= samples.len() {
            continue;
        }
        let residual: Vec<_> = (order..samples.len())
            .map(|i| {
                zigzag(
                    samples[i]
                        - coef
                            .iter()
                            .enumerate()
                            .map(|(j, &c)| c * samples[i - j - 1])
                            .sum::<i64>(),
                )
            })
            .collect();
        for k in 0..=14 {
            let cost = 8
                + order * bits
                + 10
                + residual
                    .iter()
                    .map(|&r| (r >> k) as usize + 1 + k)
                    .sum::<usize>();
            if cost < best.0 {
                best = (cost, order, k, residual.clone());
            }
        }
    }
    if best.0 >= raw {
        w.write(2, 8);
        for &s in samples {
            w.write(s as u64, bits);
        }
    } else {
        let (_, order, k, residual) = best;
        w.write((8 + order) as u64 * 2, 8);
        for &s in &samples[..order] {
            w.write(s as u64, bits);
        }
        w.write(0, 2);
        w.write(0, 4);
        w.write(k as u64, 4);
        for r in residual {
            for _ in 0..r >> k {
                w.write(0, 1);
            }
            w.write(1, 1);
            w.write(r, k);
        }
    }
}
pub(crate) fn encode(audio: &Audio, options: &EncodeOptions) -> Result<Vec<u8>> {
    let comments = crate::metadata::flac_comments(options)?;
    let picture = options
        .tags
        .cover
        .as_ref()
        .map(crate::metadata::flac_picture)
        .transpose()?;
    let mut metadata_budget = crate::metadata::Budget::default();
    for block in comments.iter().chain(picture.iter()) {
        metadata_budget.add(block.len())?;
    }
    let bits = match options.sample_format {
        SampleFormat::U8 | SampleFormat::S8 => 8,
        SampleFormat::S16 => 16,
        SampleFormat::S24 => 24,
        SampleFormat::S32 => 32,
        _ => return Err(Error::encode("FLAC requires integer PCM")),
    };
    if audio.channels() > 8 || audio.rate > 0xfffff || options.compression_level > 8 {
        return Err(Error::encode(
            "Invalid FLAC channel count, rate or compression level",
        ));
    }
    let scale = (1u64 << (bits - 1)) as f64;
    let samples: Vec<Vec<i64>> = audio
        .data
        .iter()
        .map(|c| {
            c.iter()
                .map(|&s| (s as f64 * scale).round().clamp(-scale, scale - 1.0) as i64)
                .collect()
        })
        .collect();
    let block = 4096usize;
    let mut stream = BitWriter::new();
    stream.write(block as u64, 16);
    stream.write(block as u64, 16);
    stream.write(0, 24);
    stream.write(0, 24);
    stream.write(audio.rate as u64, 20);
    stream.write((audio.channels() - 1) as u64, 3);
    stream.write((bits - 1) as u64, 5);
    stream.write(audio.frames() as u64, 36);
    let mut hash = Md5::new();
    let mut frames = Vec::new();
    for (number, start) in (0..audio.frames()).step_by(block).enumerate() {
        let n = block.min(audio.frames() - start);
        let mut w = BitWriter::new();
        w.write(0x3ffe, 14);
        w.write(0, 2);
        w.write(7, 4);
        w.write(0, 4);
        w.write((audio.channels() - 1) as u64, 4);
        w.write(0, 4);
        write_number(&mut w, number as u64);
        w.write((n - 1) as u64, 16);
        let crc = crc8(&w.bytes);
        w.write(crc as u64, 8);
        for c in &samples {
            sub_writer(
                &c[start..start + n],
                bits,
                options.compression_level,
                &mut w,
            );
        }
        w.align();
        let crc = crc16(&w.bytes);
        w.write(crc as u64, 16);
        frames.extend_from_slice(&w.bytes);
        let mut raw = Vec::new();
        for i in start..start + n {
            for c in &samples {
                for j in 0..bits.div_ceil(8) {
                    raw.push((c[i] >> (8 * j)) as u8);
                }
            }
        }
        hash.update(&raw);
    }
    stream.bytes.extend_from_slice(&hash.digest());
    let mut out = b"fLaC".to_vec();
    out.push(if comments.is_none() && picture.is_none() {
        128
    } else {
        0
    });
    out.extend_from_slice(&[0, 0, 34]);
    out.extend_from_slice(&stream.bytes);
    if let Some(tags) = comments {
        out.push(if picture.is_none() { 132 } else { 4 });
        out.extend_from_slice(&[
            (tags.len() >> 16) as u8,
            (tags.len() >> 8) as u8,
            tags.len() as u8,
        ]);
        out.extend_from_slice(&tags);
    }
    if let Some(picture) = picture {
        out.push(134);
        out.extend_from_slice(&[
            (picture.len() >> 16) as u8,
            (picture.len() >> 8) as u8,
            picture.len() as u8,
        ]);
        out.extend_from_slice(&picture);
    }
    out.extend_from_slice(&frames);
    Ok(out)
}

const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// MD5 of the decoded PCM, for checking FLAC's STREAMINFO signature. Allocation-free.
pub(crate) struct Md5 {
    state: [u32; 4],
    pending: [u8; 64],
    filled: usize,
    length: u64,
}
impl Md5 {
    pub fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            pending: [0; 64],
            filled: 0,
            length: 0,
        }
    }
    pub fn update(&mut self, mut bytes: &[u8]) {
        self.length += bytes.len() as u64;
        if self.filled > 0 {
            let n = (64 - self.filled).min(bytes.len());
            self.pending[self.filled..self.filled + n].copy_from_slice(&bytes[..n]);
            self.filled += n;
            bytes = &bytes[n..];
            if self.filled < 64 {
                return;
            }
            let block = self.pending;
            self.block(&block);
            self.filled = 0;
        }
        // Whole blocks straight from the input, without staging them.
        let mut blocks = bytes.chunks_exact(64);
        for block in &mut blocks {
            self.block(block.try_into().unwrap());
        }
        let rest = blocks.remainder();
        self.pending[..rest.len()].copy_from_slice(rest);
        self.filled = rest.len();
    }
    fn block(&mut self, chunk: &[u8; 64]) {
        let mut m = [0u32; 16];
        for (i, b) in chunk.chunks_exact(4).enumerate() {
            m[i] = u32::from_le_bytes(b.try_into().unwrap());
        }
        let [mut a, mut b, mut c, mut d] = self.state;
        // The 64 steps of RFC 1321, unrolled so that every rotation, message index and
        // constant is fixed: the same arithmetic as the loop form, in the same order.
        macro_rules! step {
            ($f:expr, $i:expr, $g:expr, $s:expr) => {{
                let next = b.wrapping_add(
                    a.wrapping_add($f)
                        .wrapping_add(MD5_K[$i])
                        .wrapping_add(m[$g])
                        .rotate_left($s),
                );
                a = d;
                d = c;
                c = b;
                b = next;
            }};
        }
        macro_rules! round {
            ($f:expr, $g:expr, [$s0:expr, $s1:expr, $s2:expr, $s3:expr], $base:expr) => {
                step!($f, $base, $g($base), $s0);
                step!($f, $base + 1, $g($base + 1), $s1);
                step!($f, $base + 2, $g($base + 2), $s2);
                step!($f, $base + 3, $g($base + 3), $s3);
            };
        }
        let g0 = |i: usize| i;
        let g1 = |i: usize| (5 * i + 1) % 16;
        let g2 = |i: usize| (3 * i + 5) % 16;
        let g3 = |i: usize| (7 * i) % 16;
        for base in [0, 4, 8, 12] {
            round!((b & c) | (!b & d), g0, [7, 12, 17, 22], base);
        }
        for base in [16, 20, 24, 28] {
            round!((d & b) | (!d & c), g1, [5, 9, 14, 20], base);
        }
        for base in [32, 36, 40, 44] {
            round!(b ^ c ^ d, g2, [4, 11, 16, 23], base);
        }
        for base in [48, 52, 56, 60] {
            round!(c ^ (b | !d), g3, [6, 10, 15, 21], base);
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }
    pub fn digest(mut self) -> [u8; 16] {
        let bits = self.length * 8;
        let n = if self.filled < 56 {
            56 - self.filled
        } else {
            120 - self.filled
        };
        let mut pad = [0u8; 72];
        pad[0] = 128;
        pad[n..n + 8].copy_from_slice(&bits.to_le_bytes());
        let length = self.length;
        self.update(&pad[..n + 8]);
        self.length = length;
        let mut out = [0; 16];
        for (i, s) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{Md5, unary};
    use crate::io::Bits;

    /// The bit-at-a-time unary reader of 0.4.0: zeros before a one, with its errors.
    fn reference_unary(bytes: &[u8], pos: &mut usize) -> Result<u64, &'static str> {
        let mut n = 0u64;
        loop {
            if *pos >= bytes.len() * 8 {
                return Err("Truncated bitstream");
            }
            let bit = (bytes[*pos / 8] >> (7 - *pos % 8)) & 1;
            *pos += 1;
            if bit == 1 {
                return Ok(n);
            }
            n += 1;
            if n > 1 << 24 {
                return Err("Runaway FLAC unary code");
            }
        }
    }

    #[test]
    fn unary_codes_match_bit_by_bit_reading_and_its_errors() {
        let mut seed = 0x9e37_79b9u32;
        let mut inputs: Vec<Vec<u8>> = (0..300)
            .map(|k| {
                (0..k % 40)
                    .map(|_| {
                        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        // Mostly zero bits, so that codes run across bytes and windows.
                        let b = (seed >> 24) as u8;
                        if b < 200 { 0 } else { b }
                    })
                    .collect()
            })
            .collect();
        // Runaway codes: exactly 2^24 zeros is a valid code, one more is an error.
        let mut at_limit = vec![0u8; (1 << 24) / 8];
        at_limit.push(0x80);
        inputs.push(at_limit);
        inputs.push(vec![0u8; (1 << 24) / 8 + 1]);
        inputs.push(vec![0u8; (1 << 24) / 8 + 64]);
        for bytes in &inputs {
            for start in 0..bytes.len().min(3) * 8 + 1 {
                let mut br = Bits::new(bytes);
                br.pos = start;
                let mut pos = start;
                loop {
                    let got = unary(&mut br).map_err(|e| e.message.clone());
                    let want = reference_unary(bytes, &mut pos).map_err(String::from);
                    assert_eq!(got, want, "{} bytes from bit {start}", bytes.len());
                    if got.is_err() {
                        break;
                    }
                    assert_eq!(br.pos, pos);
                }
            }
        }
    }

    fn hex(digest: [u8; 16]) -> String {
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn md5_matches_rfc_1321_for_every_split() {
        // The RFC 1321 test suite, A.5.
        let suite = [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (text, expected) in suite {
            // Every way of feeding the input: whole, byte by byte, and in uneven
            // pieces crossing the 64-byte block boundary.
            for piece in 1..=130 {
                let mut md5 = Md5::new();
                for chunk in text.as_bytes().chunks(piece) {
                    md5.update(chunk);
                }
                assert_eq!(hex(md5.digest()), expected, "{text:?} in pieces of {piece}");
            }
        }
        // A long input fed in mixed piece sizes, against the same input in one piece.
        let data: Vec<u8> = (0..10_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let mut whole = Md5::new();
        whole.update(&data);
        let mut mixed = Md5::new();
        let mut rest = &data[..];
        for n in (1..).map(|k| k * 7 % 150 + 1) {
            if rest.is_empty() {
                break;
            }
            let n = n.min(rest.len());
            mixed.update(&rest[..n]);
            rest = &rest[n..];
        }
        assert_eq!(whole.digest(), mixed.digest());
    }
}
