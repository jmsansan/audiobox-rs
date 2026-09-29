use super::{Header, find, tables::*, tag_size, transform, vbr};
use crate::io::Bits;
use crate::{Audio, AudioFormat, DecodeInfo, DecodeOptions, Error, Result, SampleFormat};
#[derive(Clone, Debug, Default)]
pub(super) struct Granule {
    pub length: usize,
    pub pairs: usize,
    pub gain: u32,
    pub compress: usize,
    pub block: usize,
    pub mixed: bool,
    pub table: [usize; 3],
    pub subgain: [u32; 3],
    pub r0: usize,
    pub r1: usize,
    pub pre: bool,
    pub scale: f64,
    pub count: usize,
}
#[derive(Clone, Default)]
struct Scalefactors {
    long: [u32; 23],
    short: [[u32; 14]; 3],
    long_illegal: [bool; 23],
    short_illegal: [[bool; 14]; 3],
}
type SideInfo = (usize, Vec<[u32; 4]>, Vec<Vec<Granule>>);
fn side(bytes: &[u8], h: Header) -> Result<SideInfo> {
    let mut br = Bits::new(bytes);
    let mpeg1 = h.version == 1;
    let main = br.read(if mpeg1 { 9 } else { 8 })? as usize;
    br.read(if mpeg1 {
        if h.channels == 1 { 5 } else { 3 }
    } else if h.channels == 1 {
        1
    } else {
        2
    })?;
    let mut scfsi = vec![[0; 4]; h.channels];
    if mpeg1 {
        for c in &mut scfsi {
            for v in c {
                *v = br.read(1)?;
            }
        }
    }
    let mut granules = Vec::new();
    for _ in 0..if mpeg1 { 2 } else { 1 } {
        let mut gs = Vec::new();
        for _ in 0..h.channels {
            let mut g = Granule {
                length: br.read(12)? as usize,
                pairs: br.read(9)? as usize,
                gain: br.read(8)?,
                compress: br.read(if mpeg1 { 4 } else { 9 })? as usize,
                ..Default::default()
            };
            let switching = br.read(1)? != 0;
            if switching {
                g.block = br.read(2)? as usize;
                g.mixed = br.read(1)? != 0;
                g.table[0] = br.read(5)? as usize;
                g.table[1] = br.read(5)? as usize;
                for gain in &mut g.subgain {
                    *gain = br.read(3)?;
                }
                g.r0 = if g.block == 2 && !g.mixed { 8 } else { 7 };
                g.r1 = 20 - g.r0;
            } else {
                for t in &mut g.table {
                    *t = br.read(5)? as usize;
                }
                g.r0 = br.read(4)? as usize;
                g.r1 = br.read(3)? as usize;
            }
            if g.pairs > 288
                || switching && g.block == 0
                || g.table.iter().any(|&t| t == 4 || t == 14)
            {
                return Err(Error::decode("Invalid MP3 side information"));
            }
            g.pre = mpeg1 && br.read(1)? != 0;
            g.scale = if br.read(1)? != 0 { 1.0 } else { 0.5 };
            g.count = br.read(1)? as usize;
            gs.push(g);
        }
        granules.push(gs);
    }
    Ok((main, scfsi, granules))
}
const NR: [[[usize; 4]; 3]; 6] = [
    [[6, 5, 5, 5], [9, 9, 9, 9], [6, 9, 9, 9]],
    [[6, 5, 7, 3], [9, 9, 12, 6], [6, 9, 12, 6]],
    [[11, 10, 0, 0], [18, 18, 0, 0], [15, 18, 0, 0]],
    [[7, 7, 7, 0], [12, 12, 12, 0], [6, 15, 12, 0]],
    [[6, 6, 6, 3], [12, 9, 9, 6], [6, 12, 9, 6]],
    [[8, 8, 5, 0], [15, 12, 9, 0], [6, 18, 9, 0]],
];
fn scalefactors(
    br: &mut Bits<'_>,
    g: &mut Granule,
    h: Header,
    scfsi: [u32; 4],
    gr: usize,
    prev: &Scalefactors,
    ch: usize,
) -> Result<Scalefactors> {
    let mut sf = Scalefactors::default();
    let short = g.block == 2;
    if h.version == 1 {
        let n1 = SLEN1[g.compress] as usize;
        let n2 = SLEN2[g.compress] as usize;
        if short {
            if g.mixed {
                for b in 0..8 {
                    sf.long[b] = br.read(n1)?;
                }
            }
            for b in if g.mixed { 3 } else { 0 }..12 {
                for w in 0..3 {
                    sf.short[w][b] = br.read(if b < 6 { n1 } else { n2 })?;
                }
            }
        } else {
            for (group, (start, end, n)) in [(0, 6, n1), (6, 11, n1), (11, 16, n2), (16, 21, n2)]
                .iter()
                .copied()
                .enumerate()
            {
                for b in start..end {
                    sf.long[b] = if gr == 1 && scfsi[group] != 0 {
                        prev.long[b]
                    } else {
                        br.read(n)?
                    };
                }
            }
        }
    } else {
        let compress = g.compress;
        let intensity = ch == 1 && h.mode == 1 && h.extension & 1 != 0;
        let (slots, index) = if !intensity {
            if compress < 400 {
                let c = compress >> 4;
                ([c / 5, c % 5, (compress % 16) >> 2, compress % 4], 0)
            } else if compress < 500 {
                let c = (compress - 400) >> 2;
                ([c / 5, c % 5, (compress - 400) % 4, 0], 1)
            } else {
                let c = compress - 500;
                g.pre = true;
                ([c / 3, c % 3, 0, 0], 2)
            }
        } else {
            let c = compress >> 1;
            if c < 180 {
                ([c / 36, (c % 36) / 6, c % 6, 0], 3)
            } else if c < 244 {
                let d = c - 180;
                ([d >> 4, (d % 16) >> 2, d % 4, 0], 4)
            } else {
                let d = c - 244;
                ([d / 3, d % 3, 0, 0], 5)
            }
        };
        let shape = if short {
            if g.mixed { 2 } else { 1 }
        } else {
            0
        };
        let counts = NR[index][shape];
        let mut band = 0;
        // Mixed MPEG-2 has six long factors followed by short band 3 onwards.
        for slot in 0..4 {
            for _ in 0..counts[slot] {
                let value = br.read(slots[slot])?;
                let illegal = intensity && slots[slot] > 0 && value == (1u32 << slots[slot]) - 1;
                if !short {
                    if band < 22 {
                        sf.long[band] = value;
                        sf.long_illegal[band] = illegal;
                    }
                } else if g.mixed && band < 6 {
                    sf.long[band] = value;
                    sf.long_illegal[band] = illegal;
                } else {
                    let at = if g.mixed { band - 6 + 9 } else { band };
                    if at / 3 < 13 {
                        sf.short[at % 3][at / 3] = value;
                        sf.short_illegal[at % 3][at / 3] = illegal;
                    }
                }
                band += 1;
            }
        }
    }
    Ok(sf)
}
fn leaf(br: &mut Bits<'_>, offset: usize) -> Result<i32> {
    let mut width = 5;
    let mut leaf = *HUFFMAN_TABS
        .get(offset + br.peek(width) as usize)
        .ok_or_else(|| Error::decode("Invalid Huffman table"))?;
    for _ in 0..9 {
        if leaf >= 0 {
            br.skip((leaf >> 8) as usize)?;
            return Ok(leaf);
        }
        br.skip(width)?;
        width = (leaf & 7) as usize;
        let at = offset + br.peek(width) as usize + (-(leaf >> 3)) as usize;
        leaf = *HUFFMAN_TABS
            .get(at)
            .ok_or_else(|| Error::decode("Invalid Huffman trie"))?;
    }
    Err(Error::decode("Runaway MP3 Huffman trie"))
}
fn huffman(br: &mut Bits<'_>, g: &Granule, rate: u32, end: usize) -> Result<([i32; 576], usize)> {
    let bands = sfb_long(rate);
    let (r1, r2) = if g.block != 0 {
        (
            if g.block == 2 {
                sfb_short(rate)[3] * 3
            } else {
                bands[8]
            },
            576,
        )
    } else {
        (bands[(g.r0 + 1).min(22)], bands[(g.r0 + g.r1 + 2).min(22)])
    };
    let mut out = [0; 576];
    let mut pos = 0;
    while pos < g.pairs * 2 {
        let region = if pos < r1 {
            0
        } else if pos < r2 {
            1
        } else {
            2
        };
        let table = g.table[region];
        if table == 0 {
            pos += 2;
            continue;
        }
        if br.pos >= end {
            return Err(Error::decode("MP3 big values exceed granule"));
        }
        let l = leaf(br, HUFFMAN_TABLE_INDEX[table] as usize)?;
        let lin = HUFFMAN_LINBITS_TABLE[table] as usize;
        let mut x = l & 15;
        let mut y = (l >> 4) & 15;
        if x == 15 && lin > 0 {
            x += br.read(lin)? as i32;
        }
        if x != 0 && br.read(1)? != 0 {
            x = -x;
        }
        if y == 15 && lin > 0 {
            y += br.read(lin)? as i32;
        }
        if y != 0 && br.read(1)? != 0 {
            y = -y;
        }
        if br.pos > end {
            return Err(Error::decode("MP3 Huffman pair exceeds granule"));
        }
        out[pos] = x;
        out[pos + 1] = y;
        pos += 2;
    }
    let table: &[u32] = if g.count == 1 {
        &COUNT1_TABLE_B
    } else {
        &COUNT1_TABLE_A
    };
    while br.pos < end && pos + 4 <= 576 {
        let mut l = table[br.peek(4) as usize];
        if l & 8 == 0 {
            let extra = (l & 3) as usize;
            let base = (l >> 3) as usize;
            l = table[base + (br.peek(4 + extra) & ((1 << extra) - 1)) as usize];
        }
        let length = (l & 7) as usize;
        let signs = (0..4).filter(|s| l & (128 >> s) != 0).count();
        if br.pos + length + signs > end {
            break;
        }
        br.skip(length)?;
        for s in 0..4 {
            if l & (128 >> s) != 0 {
                out[pos + s] = if br.read(1)? != 0 { -1 } else { 1 };
            }
        }
        pos += 4;
    }
    let nonzero = out.iter().rposition(|&n| n != 0).map_or(0, |n| n + 1);
    Ok((out, nonzero))
}
fn requantize(values: &[i32; 576], g: &Granule, sf: &Scalefactors, rate: u32) -> [f32; 576] {
    let mut out = [0.0; 576];
    let base = 2f64.powf((g.gain as f64 - 210.0) / 4.0);
    let long = sfb_long(rate);
    let long_end = if g.block == 2 {
        if g.mixed { 36 } else { 0 }
    } else {
        576
    };
    for b in 0..22 {
        let scale =
            base * 2f64.powf(-g.scale * (sf.long[b] + if g.pre { PRETAB[b] } else { 0 }) as f64);
        for i in long[b]..long[b + 1].min(long_end) {
            out[i] = (values[i].signum() as f64
                * (values[i].unsigned_abs() as f64).powf(4.0 / 3.0)
                * scale) as f32;
        }
    }
    if g.block == 2 {
        let bands = sfb_short(rate);
        let mut pos = if g.mixed { 36 } else { 0 };
        for b in if g.mixed { 3 } else { 0 }..13 {
            let width = bands[b + 1] - bands[b];
            for w in 0..3 {
                let scale =
                    base * 2f64.powf(-2.0 * g.subgain[w] as f64 - g.scale * sf.short[w][b] as f64);
                for _ in 0..width {
                    if pos >= 576 {
                        break;
                    }
                    out[pos] = (values[pos].signum() as f64
                        * (values[pos].unsigned_abs() as f64).powf(4.0 / 3.0)
                        * scale) as f32;
                    pos += 1;
                }
            }
        }
    }
    out
}
fn reorder(x: &mut [f32; 576], g: &Granule, rate: u32) {
    if g.block != 2 {
        return;
    }
    let mut scratch = *x;
    let bands = sfb_short(rate);
    let mut pos = if g.mixed { 36 } else { 0 };
    for b in if g.mixed { 3 } else { 0 }..13 {
        for w in 0..3 {
            for freq in bands[b]..bands[b + 1] {
                let target = freq / 6 * 18 + w * 6 + freq % 6;
                if target < 576 && pos < 576 {
                    scratch[target] = x[pos];
                }
                pos += 1;
            }
        }
    }
    *x = scratch;
}
fn stereo(
    left: &mut [f32; 576],
    right: &mut [f32; 576],
    h: Header,
    g: &Granule,
    sf: &Scalefactors,
) {
    let ms = h.extension & 2 != 0;
    let intensity = h.extension & 1 != 0;
    let mut regions = Vec::new();
    if g.block != 2 {
        let bands = sfb_long(h.rate);
        for b in 0..22 {
            regions.push((bands[b], bands[b + 1], 0, b, sf.long[b], sf.long_illegal[b]));
        }
    } else {
        let long = sfb_long(h.rate);
        let short = sfb_short(h.rate);
        let mixed_lines = if g.mixed { short[3] * 3 } else { 0 };
        if g.mixed {
            for b in 0..22 {
                if long[b] >= mixed_lines {
                    break;
                }
                regions.push((
                    long[b],
                    long[b + 1].min(mixed_lines),
                    0,
                    b,
                    sf.long[b],
                    sf.long_illegal[b],
                ));
            }
        }
        let mut pos = mixed_lines;
        for b in if g.mixed { 3 } else { 0 }..13 {
            for w in 0..3 {
                let end = (pos + short[b + 1] - short[b]).min(576);
                regions.push((pos, end, w, b, sf.short[w][b], sf.short_illegal[w][b]));
                pos = end;
            }
        }
    }
    let mut last = [None; 3];
    for &(from, to, w, b, _, _) in &regions {
        if right[from..to].iter().any(|&s| s != 0.0) {
            last[w] = Some(b);
        }
    }
    if g.mixed {
        let highest = last.iter().flatten().max().copied();
        if let Some(b) = highest {
            last.fill(Some(b));
        }
    }
    for &(from, to, w, b, mut position, mut illegal) in &regions {
        let final_band = if g.block == 2 { 12 } else { 21 };
        if b == final_band {
            if last[w].is_some_and(|n| n >= b - 1) {
                position = if h.version == 1 { 3 } else { 0 };
                illegal = false;
            } else if g.block == 2 {
                position = sf.short[w][b - 1];
                illegal = sf.short_illegal[w][b - 1];
            } else {
                position = sf.long[b - 1];
                illegal = sf.long_illegal[b - 1];
            }
        }
        let use_intensity = intensity
            && last[w].is_none_or(|n| b > n)
            && !illegal
            && (h.version != 1 || position < 7);
        if use_intensity {
            let (l, r) = if h.version == 1 {
                if position == 6 {
                    (1.0, 0.0)
                } else {
                    let t = (position as f64 * std::f64::consts::PI / 12.0).tan();
                    (t / (1.0 + t), 1.0 / (1.0 + t))
                }
            } else {
                let step = if g.compress & 1 == 0 { -0.25 } else { -0.5 };
                if position == 0 {
                    (1.0, 1.0)
                } else if position & 1 != 0 {
                    (2f64.powf(step * position.div_ceil(2) as f64), 1.0)
                } else {
                    (1.0, 2f64.powf(step * (position / 2) as f64))
                }
            };
            for i in from..to {
                let value = left[i] as f64;
                left[i] = (value * l) as f32;
                right[i] = (value * r) as f32;
            }
        } else if ms {
            for i in from..to {
                let m = left[i];
                let s = right[i];
                left[i] = (m + s) * std::f32::consts::FRAC_1_SQRT_2;
                right[i] = (m - s) * std::f32::consts::FRAC_1_SQRT_2;
            }
        }
    }
}
pub(crate) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Audio> {
    let (at, first) = find(bytes, tag_size(bytes))
        .ok_or_else(|| Error::decode("No readable MPEG Layer III frame"))?;
    let tag = vbr(bytes, at, first)?;
    let channels = first.channels;
    let rate = first.rate;
    let mut output = vec![Vec::new(); channels];
    options.limits.check(0, channels, rate)?;
    if let Some(tag) = tag {
        let frames = tag
            .frames
            .checked_mul(if first.version == 1 { 1152 } else { 576 })
            .ok_or_else(|| Error::limit("MP3 sample count overflow"))?;
        options.limits.check(frames, channels, rate)?;
    }
    let mut history = Vec::new();
    let mut overlap = vec![[0.0; 576]; channels];
    let mut synth: Vec<_> = (0..channels).map(|_| transform::Synthesis::new()).collect();
    let mut previous = vec![Scalefactors::default(); channels];
    let mut pos = at;
    let mut index = 0;
    let mut frames = 0usize;
    while let Some(h) = super::header(bytes, pos) {
        if pos + h.length > bytes.len() {
            return Err(Error::decode("Truncated MP3 frame"));
        }
        if h.rate != rate || h.channels != channels {
            return Err(Error::decode("MP3 stream shape changed"));
        }
        let side_start = pos + 4 + h.crc;
        let data_start = side_start + h.side;
        let payload = &bytes[data_start..pos + h.length];
        let (back, scfsi, mut granules) = side(&bytes[side_start..data_start], h)?;
        let enough = back <= history.len();
        let mut main = if enough {
            history[history.len() - back..].to_vec()
        } else {
            Vec::new()
        };
        main.extend_from_slice(payload);
        history.extend_from_slice(payload);
        if history.len() > 4096 {
            history.drain(..history.len() - 4096);
        }
        pos += h.length;
        index += 1;
        if index == 1 && tag.is_some() {
            continue;
        }
        if !enough {
            continue;
        }
        let next = frames
            .checked_add(granules.len() * 576)
            .ok_or_else(|| Error::limit("MP3 sample count overflow"))?;
        options.limits.check(next, channels, rate)?;
        for c in &mut output {
            c.try_reserve_exact(next - frames)
                .map_err(|_| Error::limit("Cannot allocate MP3 output"))?;
        }
        let mut br = Bits::new(&main);
        for (gr, gs) in granules.iter_mut().enumerate() {
            let mut spectra = [[0.0; 576]; 2];
            let mut factors = Vec::new();

            for ch in 0..channels {
                let g = &mut gs[ch];
                let end = br
                    .pos
                    .checked_add(g.length)
                    .filter(|&n| n <= main.len() * 8)
                    .ok_or_else(|| Error::decode("MP3 granule exceeds main data"))?;
                let sf = scalefactors(&mut br, g, h, scfsi[ch], gr, &previous[ch], ch)?;
                if br.pos > end {
                    return Err(Error::decode("MP3 scalefactors exceed granule"));
                }
                let (values, _) = huffman(&mut br, g, rate, end)?;

                spectra[ch] = requantize(&values, g, &sf, rate);
                br.pos = end;
                previous[ch] = sf.clone();
                factors.push(sf);
            }
            if channels == 2 && h.mode == 1 {
                let (l, r) = spectra.split_at_mut(1);
                stereo(&mut l[0], &mut r[0], h, &gs[1], &factors[1]);
            }
            for ch in 0..channels {
                reorder(&mut spectra[ch], &gs[ch], rate);
                transform::alias(&mut spectra[ch], gs[ch].block == 2, gs[ch].mixed, false);
                let time = transform::imdct(&spectra[ch], &gs[ch], &mut overlap[ch]);
                output[ch].extend_from_slice(&synth[ch].granule(&time));
            }
        }
        frames = next;
    }
    if frames == 0 {
        return Err(Error::decode("No decodable MP3 frames"));
    }
    options.limits.check(
        frames
            .checked_add(64)
            .ok_or_else(|| Error::limit("MP3 flush overflow"))?,
        channels,
        rate,
    )?;
    for ch in 0..channels {
        output[ch]
            .try_reserve_exact(64)
            .map_err(|_| Error::limit("Cannot allocate MP3 filter tail"))?;
        for _ in 0..2 {
            output[ch].extend_from_slice(&synth[ch].process(&[0.0; 32]));
        }
    }
    let tag = tag.unwrap_or_default();
    let start = (tag.delay + 529 + 64).min(output[0].len());
    let end = output[0]
        .len()
        .saturating_sub(tag.padding.saturating_sub(529))
        .max(start);
    let data = output.into_iter().map(|c| c[start..end].to_vec()).collect();
    Audio::decoded(
        data,
        rate,
        DecodeInfo {
            format: Some(AudioFormat::Mp3),
            sample_format: Some(SampleFormat::S16),
            ..Default::default()
        },
    )
}
