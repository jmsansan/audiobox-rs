#![allow(clippy::needless_range_loop)]
use crate::io::{BitWriter, Bits, Reader, put32};
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
fn unary(br: &mut Bits<'_>) -> Result<u64> {
    let mut n = 0u64;
    while br.read(1)? == 0 {
        n += 1;
        if n > 1 << 24 {
            return Err(Error::decode("Runaway FLAC unary code"));
        }
    }
    Ok(n)
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
fn subframe(br: &mut Bits<'_>, n: usize, bits: usize) -> Result<Vec<i64>> {
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
    let mut out = vec![0i64; n];
    match kind {
        0 => {
            let v = signed(br, effective)?;
            out.fill(v);
        }
        1 => {
            for v in &mut out {
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
            let (coeff, shift) = if kind < 32 {
                (FIXED[order].to_vec(), 0)
            } else {
                let precision = br.read(4)? as usize + 1;
                if precision == 16 {
                    return Err(Error::decode("Reserved FLAC LPC precision"));
                }
                let shift = br.signed(5)?;
                if shift < 0 {
                    return Err(Error::decode("Negative FLAC LPC shift"));
                }
                let mut coeff = Vec::new();
                for _ in 0..order {
                    coeff.push(br.signed(precision)? as i64);
                }
                (coeff, shift as u32)
            };
            residual(br, &mut out, order)?;
            for i in order..n {
                let mut prediction = 0i64;
                for (j, &coef) in coeff.iter().enumerate() {
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
    for s in &mut out {
        *s = s
            .checked_mul(1i64 << wasted)
            .ok_or_else(|| Error::decode("FLAC wasted bits overflow"))?;
    }
    Ok(out)
}

pub(crate) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Audio> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != b"fLaC" {
        return Err(Error::decode("Invalid FLAC magic"));
    }
    let mut stream = None;
    let mut metadata = crate::AudioMetadata::new();
    let mut expected_md5 = [0u8; 16];
    loop {
        let header = r.take(4)?;
        let last = header[0] & 128 != 0;
        let kind = header[0] & 127;
        let n = ((header[1] as usize) << 16) | ((header[2] as usize) << 8) | header[3] as usize;
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
            options.limits.check(frames, channels, rate)?;
            if !(4..=32).contains(&bits) || max == 0 {
                return Err(Error::decode("Invalid FLAC stream shape"));
            }
            expected_md5.copy_from_slice(&b[18..34]);
            stream = Some((rate, channels, bits, frames, max));
        } else if kind == 4 {
            let mut tags = Reader::new(b);
            let vendor = tags.u32(true)? as usize;
            tags.take(vendor)?;
            let count = tags.u32(true)?;
            for _ in 0..count {
                let n = tags.u32(true)? as usize;
                let tag = String::from_utf8_lossy(tags.take(n)?);
                if let Some((key, value)) = tag.split_once('=') {
                    metadata.insert(key.to_ascii_lowercase(), value.to_string());
                }
            }
        }
        if last {
            break;
        }
    }
    let (rate, channels, bits, total, max) =
        stream.ok_or_else(|| Error::decode("Missing FLAC STREAMINFO"))?;
    let mut out = vec![Vec::<f32>::new(); channels];
    let mut pcm_hash = Md5::new();
    let mut written = 0usize;
    while r.pos < bytes.len() {
        let start = r.pos;
        let mut br = Bits::new(&bytes[start..]);
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
        let next = written
            .checked_add(n)
            .ok_or_else(|| Error::limit("FLAC frame count overflow"))?;
        options.limits.check(next, channels, rate)?;
        if total != 0 && next > total {
            return Err(Error::decode("FLAC exceeds declared sample count"));
        }
        for c in &mut out {
            c.try_reserve_exact(n)
                .map_err(|_| Error::limit("Cannot allocate FLAC output"))?;
        }
        let mut subs = Vec::new();
        for c in 0..channels {
            let extra =
                (assign == 8 && c == 1) || (assign == 9 && c == 0) || (assign == 10 && c == 1);
            subs.push(subframe(&mut br, n, bits + usize::from(extra))?);
        }
        if assign >= 8 {
            for i in 0..n {
                let a = subs[0][i];
                let b = subs[1][i];
                match assign {
                    8 => subs[1][i] = a - b,
                    9 => subs[0][i] = a + b,
                    10 => {
                        let mid = (a << 1) | (b & 1);
                        subs[0][i] = (mid + b) >> 1;
                        subs[1][i] = (mid - b) >> 1;
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
        r.pos = start + br.pos / 8;
        let scale = (1u64 << (bits - 1)) as f64;
        let mut raw = Vec::with_capacity(n * channels * bits.div_ceil(8));
        for i in 0..n {
            for c in 0..channels {
                let s = subs[c][i];
                if s < -(1i64 << (bits - 1)) || s >= (1i64 << (bits - 1)) {
                    return Err(Error::decode("FLAC sample exceeds bit depth"));
                }
                out[c].push((s as f64 / scale) as f32);
                for j in 0..bits.div_ceil(8) {
                    raw.push((s >> (8 * j)) as u8);
                }
            }
        }
        pcm_hash.update(&raw);
        written = next;
    }
    if total != 0 && written != total {
        return Err(Error::decode("FLAC sample count mismatch"));
    }
    if expected_md5 != [0; 16] && pcm_hash.digest() != expected_md5 {
        return Err(Error::decode("FLAC PCM MD5 mismatch"));
    }
    Audio::decoded(
        out,
        rate,
        DecodeInfo {
            format: Some(AudioFormat::Flac),
            sample_format: Some(if bits <= 16 {
                SampleFormat::S16
            } else if bits <= 24 {
                SampleFormat::S24
            } else {
                SampleFormat::S32
            }),
            metadata,
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
    out.push(if options.metadata.is_empty() { 128 } else { 0 });
    out.extend_from_slice(&[0, 0, 34]);
    out.extend_from_slice(&stream.bytes);
    if !options.metadata.is_empty() {
        let mut tags = Vec::new();
        put32(&mut tags, 8, true);
        tags.extend_from_slice(b"audiobox");
        put32(&mut tags, options.metadata.len() as u32, true);
        for (key, value) in &options.metadata {
            if key.is_empty() || key.contains('=') || !key.is_ascii() {
                return Err(Error::encode("Invalid FLAC metadata key"));
            }
            let tag = format!("{}={value}", key.to_ascii_uppercase());
            put32(&mut tags, tag.len() as u32, true);
            tags.extend_from_slice(tag.as_bytes());
        }
        if tags.len() > 0xffffff {
            return Err(Error::encode("FLAC metadata exceeds 16 MiB"));
        }
        out.push(132);
        out.extend_from_slice(&[
            (tags.len() >> 16) as u8,
            (tags.len() >> 8) as u8,
            tags.len() as u8,
        ]);
        out.extend_from_slice(&tags);
    }
    out.extend_from_slice(&frames);
    Ok(out)
}

struct Md5 {
    state: [u32; 4],
    pending: Vec<u8>,
    length: u64,
}
impl Md5 {
    fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            pending: Vec::new(),
            length: 0,
        }
    }
    fn update(&mut self, bytes: &[u8]) {
        self.length += bytes.len() as u64;
        self.pending.extend_from_slice(bytes);
        let full = self.pending.len() / 64 * 64;
        for chunk in self.pending[..full].chunks_exact(64) {
            let mut m = [0u32; 16];
            for (i, b) in chunk.chunks_exact(4).enumerate() {
                m[i] = u32::from_le_bytes(b.try_into().unwrap());
            }
            let [mut a, mut b, mut c, mut d] = self.state;
            let shifts = [
                [7, 12, 17, 22],
                [5, 9, 14, 20],
                [4, 11, 16, 23],
                [6, 10, 15, 21],
            ];
            for i in 0..64 {
                let (f, g) = if i < 16 {
                    ((b & c) | (!b & d), i)
                } else if i < 32 {
                    ((d & b) | (!d & c), (5 * i + 1) % 16)
                } else if i < 48 {
                    (b ^ c ^ d, (3 * i + 5) % 16)
                } else {
                    (c ^ (b | !d), (7 * i) % 16)
                };
                let k = (((i + 1) as f64).sin().abs() * 4294967296.0).floor() as u32;
                let next = b.wrapping_add(
                    a.wrapping_add(f)
                        .wrapping_add(k)
                        .wrapping_add(m[g])
                        .rotate_left(shifts[i / 16][i % 4]),
                );
                a = d;
                d = c;
                c = b;
                b = next;
            }
            for (s, v) in self.state.iter_mut().zip([a, b, c, d]) {
                *s = s.wrapping_add(v);
            }
        }
        self.pending.drain(..full);
    }
    fn digest(mut self) -> [u8; 16] {
        let bits = self.length * 8;
        let n = if self.pending.len() < 56 {
            56 - self.pending.len()
        } else {
            120 - self.pending.len()
        };
        let mut pad = vec![0; n];
        pad[0] = 128;
        pad.extend_from_slice(&bits.to_le_bytes());
        self.update(&pad);
        let mut out = [0; 16];
        for (i, s) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
        }
        out
    }
}
