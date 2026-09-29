use super::{BITRATES, tables::*, transform::Analysis};
use crate::io::BitWriter;
use crate::{Audio, EncodeOptions, Error, Result};

struct Quantized {
    values: [i32; 576],
    gain: usize,
    pairs: usize,
    tables: [usize; 3],
    bits: usize,
}
fn pair_bits(x: i32, y: i32, t: usize) -> Option<usize> {
    let x = x.unsigned_abs() as usize;
    let y = y.unsigned_abs() as usize;
    if t == 0 {
        return if x == 0 && y == 0 { Some(0) } else { None };
    }
    let lin = HUFFMAN_LINBITS_TABLE[t] as usize;
    let max = if lin > 0 {
        15 + ((1usize << lin) - 1)
    } else {
        HUFFMAN_ENCODE_XLEN[t] as usize - 1
    };
    if x > max || y > max {
        return None;
    }
    let ix = if lin > 0 { x.min(15) } else { x };
    let iy = if lin > 0 { y.min(15) } else { y };
    let packed = HUFFMAN_ENCODE
        [HUFFMAN_ENCODE_OFFSET[t] as usize + ix * HUFFMAN_ENCODE_YLEN[t] as usize + iy];
    if packed == 0 {
        return None;
    }
    Some(
        (packed >> 20) as usize
            + usize::from(x > 0)
            + usize::from(y > 0)
            + if lin > 0 {
                usize::from(x >= 15) * lin + usize::from(y >= 15) * lin
            } else {
                0
            },
    )
}
fn region(values: &[i32], tables: &[usize]) -> (usize, usize) {
    let mut best = (0, usize::MAX / 2);
    for &t in tables {
        let mut total = 0;
        let mut valid = true;
        for pair in values.chunks_exact(2) {
            if let Some(n) = pair_bits(pair[0], pair[1], t) {
                total += n;
            } else {
                valid = false;
                break;
            }
        }
        if valid && total < best.1 {
            best = (t, total);
        }
    }
    best
}
fn quantize(spectrum: &[f32; 576], rate: u32, budget: usize) -> Result<Quantized> {
    let bands = sfb_long(rate);
    let boundary = [0, bands[8], bands[14], 576];
    let tables = [
        0, 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
        27, 28, 29, 30, 31,
    ];
    let attempt = |gain: usize| {
        let step = 2f64.powf((gain as f64 - 210.0) / 4.0);
        let mut values = [0; 576];
        for i in 0..576 {
            let x = spectrum[i] as f64;
            let q = ((x.abs() / step).powf(0.75) + 0.4054).floor();
            if q > 8206.0 {
                return None;
            }
            values[i] = q as i32 * if x < 0.0 { -1 } else { 1 };
        }
        let pairs = values
            .iter()
            .rposition(|&n| n != 0)
            .map_or(0, |i| (i + 1).div_ceil(2));
        let mut selected = [0; 3];
        let mut bits = 0;
        for r in 0..3 {
            let start = boundary[r].min(pairs * 2);
            let end = boundary[r + 1].min(pairs * 2);
            let (t, cost) = region(&values[start..end], &tables);
            selected[r] = t;
            bits += cost;
        }
        Some(Quantized {
            values,
            gain,
            pairs,
            tables: selected,
            bits,
        })
    };
    let (mut low, mut high) = (0, 255);
    while low < high {
        let mid = (low + high) / 2;
        let fits = attempt(mid).is_some_and(|q| q.bits <= budget && q.bits <= 4095);
        if fits {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    let out = attempt(low).ok_or_else(|| Error::encode("MP3 spectrum exceeds quantizer range"))?;
    if out.bits > budget || out.bits > 4095 {
        return Err(Error::encode("MP3 granule exceeds bitrate budget"));
    }
    Ok(out)
}
fn write_pair(w: &mut BitWriter, x: i32, y: i32, t: usize) {
    if t == 0 {
        return;
    }
    let a = x.unsigned_abs() as usize;
    let b = y.unsigned_abs() as usize;
    let lin = HUFFMAN_LINBITS_TABLE[t] as usize;
    let ix = if lin > 0 { a.min(15) } else { a };
    let iy = if lin > 0 { b.min(15) } else { b };
    let packed = HUFFMAN_ENCODE
        [HUFFMAN_ENCODE_OFFSET[t] as usize + ix * HUFFMAN_ENCODE_YLEN[t] as usize + iy];
    w.write((packed & 0xfffff) as u64, (packed >> 20) as usize);
    if a >= 15 && lin > 0 {
        w.write((a - 15) as u64, lin);
    }
    if a != 0 {
        w.write(u64::from(x < 0), 1);
    }
    if b >= 15 && lin > 0 {
        w.write((b - 15) as u64, lin);
    }
    if b != 0 {
        w.write(u64::from(y < 0), 1);
    }
}
fn frame_header(bitrate: u32, rate: u32, channels: usize, padding: bool) -> Vec<u8> {
    let index = BITRATES.iter().position(|&b| b == bitrate).unwrap();
    let rate_index = [44100, 48000, 32000]
        .iter()
        .position(|&r| r == rate)
        .unwrap();
    vec![
        255,
        251,
        ((index as u8) << 4) | ((rate_index as u8) << 2) | (u8::from(padding) << 1),
        if channels == 1 { 192 } else { 0 },
    ]
}
fn write_side(w: &mut BitWriter, granules: &[Vec<Quantized>], channels: usize) {
    w.write(0, 9);
    w.write(0, if channels == 1 { 5 } else { 3 });
    for _ in 0..channels {
        w.write(0, 4);
    }
    for gs in granules {
        for q in gs {
            w.write(q.bits as u64, 12);
            w.write(q.pairs as u64, 9);
            w.write(q.gain as u64, 8);
            w.write(0, 4);
            w.write(0, 1);
            for &t in &q.tables {
                w.write(t as u64, 5);
            }
            w.write(7, 4);
            w.write(5, 3);
            w.write(0, 3);
        }
    }
}
pub(crate) fn encode(audio: &Audio, options: &EncodeOptions) -> Result<Vec<u8>> {
    if audio.channels() > 2
        || ![32000, 44100, 48000].contains(&audio.rate)
        || !BITRATES[1..15].contains(&options.bitrate)
    {
        return Err(Error::encode(
            "MP3 requires mono/stereo, 32/44.1/48 kHz and a standard 32–320 kbit/s bitrate",
        ));
    }
    if !options.metadata.is_empty() {
        return Err(Error::encode("MP3 metadata writing is not supported"));
    }
    let channels = audio.channels();
    let rate = audio.rate;
    let bitrate = options.bitrate;
    // The analysis/MDCT chain delays the signal by 1057 samples including the decoder's 529.
    let delay = 528usize;
    let flush = delay + 576;
    let count = audio
        .frames()
        .checked_add(flush)
        .ok_or_else(|| Error::encode("MP3 length overflow"))?
        .div_ceil(1152)
        .max(1);
    let padding = count * 1152 - audio.frames() - delay;
    let mut states: Vec<_> = (0..channels).map(|_| Analysis::new()).collect();
    let mut frames = Vec::new();
    let mut fraction = 0usize;
    let base = 144 * bitrate as usize * 1000 / rate as usize;
    let remainder = 144 * bitrate as usize * 1000 % rate as usize;
    let side = if channels == 1 { 17 } else { 32 };
    for frame in 0..count {
        fraction += remainder;
        let padded = fraction >= rate as usize;
        if padded {
            fraction -= rate as usize;
        }
        let length = base + usize::from(padded);
        let budget = (length - 4 - side) * 8;
        let mut granules = Vec::new();
        let mut remaining = budget;
        for gr in 0..2 {
            let mut gs = Vec::new();
            for (ch, state) in states.iter_mut().enumerate() {
                let spectrum = state.granule(&audio.data[ch], frame * 1152 + gr * 576);
                let remaining_granules = (2 - gr) * channels - ch;
                let target = (remaining / remaining_granules).min(4095);
                let q = quantize(&spectrum, rate, target)?;
                remaining -= q.bits;
                gs.push(q);
            }
            granules.push(gs);
        }
        let mut w = BitWriter::new();
        write_side(&mut w, &granules, channels);
        for gs in &granules {
            for q in gs {
                let bands = sfb_long(rate);
                for i in (0..q.pairs * 2).step_by(2) {
                    let r = if i < bands[8] {
                        0
                    } else if i < bands[14] {
                        1
                    } else {
                        2
                    };
                    write_pair(&mut w, q.values[i], q.values[i + 1], q.tables[r]);
                }
            }
        }
        w.align();
        let mut out = frame_header(bitrate, rate, channels, padded);
        out.extend_from_slice(&w.bytes);
        if out.len() > length {
            return Err(Error::encode("MP3 frame size overflow"));
        }
        out.resize(length, 0);
        frames.extend_from_slice(&out);
    }
    // A self-contained Info frame with a printable encoder tag records exact priming/padding.
    let tag_bitrate = bitrate.max(64);
    let tag_length = 144 * tag_bitrate as usize * 1000 / rate as usize;
    let mut tag = frame_header(tag_bitrate, rate, channels, false);
    tag.resize(tag_length, 0);
    let at = 4 + side;
    tag[at..at + 4].copy_from_slice(b"Info");
    tag[at + 4..at + 8].copy_from_slice(&3u32.to_be_bytes());
    tag[at + 8..at + 12].copy_from_slice(&(count as u32).to_be_bytes());
    let total_bytes = (tag.len() + frames.len()) as u32;
    tag[at + 12..at + 16].copy_from_slice(&total_bytes.to_be_bytes());
    let p = at + 16;
    tag[p..p + 9].copy_from_slice(b"audiobox ");
    tag[p + 21] = (delay >> 4) as u8;
    tag[p + 22] = ((delay & 15) << 4) as u8 | ((padding >> 8) & 15) as u8;
    tag[p + 23] = padding as u8;
    tag.extend_from_slice(&frames);
    Ok(tag)
}
