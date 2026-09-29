//! Raw PCM conversion, including G.711 A-law and µ-law.
use crate::{Audio, DecodeInfo, Endian, Error, Limits, Result, SampleFormat};

pub fn decode_pcm(
    bytes: &[u8],
    format: SampleFormat,
    endian: Endian,
    channels: usize,
    rate: u32,
    limits: &Limits,
) -> Result<Audio> {
    let stride = channels
        .checked_mul(format.bytes())
        .filter(|&n| n > 0)
        .ok_or_else(|| Error::invalid("Invalid PCM channel count"))?;
    if bytes.len() % stride != 0 {
        return Err(Error::decode("Incomplete PCM frame"));
    }
    let frames = bytes.len() / stride;
    limits.check(frames, channels, rate)?;
    let mut data = vec![vec![0.0; frames]; channels];
    for (i, frame) in bytes.chunks_exact(stride).enumerate() {
        for (c, sample) in frame.chunks_exact(format.bytes()).enumerate() {
            data[c][i] = read_sample(sample, format, endian)?;
        }
    }
    Audio::decoded(
        data,
        rate,
        DecodeInfo {
            sample_format: Some(format),
            ..Default::default()
        },
    )
}

pub(crate) fn read_sample(b: &[u8], f: SampleFormat, e: Endian) -> Result<f32> {
    let le = e == Endian::Little;
    let u = |n: usize| {
        let mut v = 0u64;
        for i in 0..n {
            v |= (b[if le { i } else { n - 1 - i }] as u64) << (8 * i);
        }
        v
    };
    let s = match f {
        SampleFormat::U8 => (b[0] as f32 - 128.0) / 128.0,
        SampleFormat::S8 => b[0] as i8 as f32 / 128.0,
        SampleFormat::S16 => u(2) as i16 as f32 / 32768.0,
        SampleFormat::S24 => ((u(3) as i32) << 8 >> 8) as f32 / 8388608.0,
        SampleFormat::S32 => u(4) as i32 as f32 / 2147483648.0,
        SampleFormat::F32 => f32::from_bits(u(4) as u32),
        SampleFormat::F64 => f64::from_bits(u(8)) as f32,
        SampleFormat::Alaw => decode_alaw(b[0]) as f32 / 32768.0,
        SampleFormat::Ulaw => decode_ulaw(b[0]) as f32 / 32768.0,
    };
    if !s.is_finite() {
        return Err(Error::decode("Non-finite PCM sample"));
    }
    Ok(s)
}
pub fn encode_pcm(audio: &Audio, format: SampleFormat, endian: Endian) -> Result<Vec<u8>> {
    let size = audio
        .frames()
        .checked_mul(audio.channels())
        .and_then(|n| n.checked_mul(format.bytes()))
        .ok_or_else(|| Error::encode("PCM size overflow"))?;
    let mut out = Vec::with_capacity(size);
    for i in 0..audio.frames() {
        for c in audio.all_channels() {
            write_sample(&mut out, c[i], format, endian);
        }
    }
    Ok(out)
}
pub(crate) fn write_sample(out: &mut Vec<u8>, sample: f32, f: SampleFormat, e: Endian) {
    let n = f.bytes();
    let quant = |bits: u32| -> i64 {
        let scale = (1u64 << (bits - 1)) as f64;
        (sample as f64 * scale).round().clamp(-scale, scale - 1.0) as i64
    };
    let value = match f {
        SampleFormat::U8 => (quant(8) + 128) as u64,
        SampleFormat::S8 => quant(8) as u64,
        SampleFormat::S16 => quant(16) as u64,
        SampleFormat::S24 => quant(24) as u64,
        SampleFormat::S32 => quant(32) as u64,
        SampleFormat::F32 => sample.to_bits() as u64,
        SampleFormat::F64 => (sample as f64).to_bits(),
        SampleFormat::Alaw => encode_alaw(quant(16) as i16) as u64,
        SampleFormat::Ulaw => encode_ulaw(quant(16) as i16) as u64,
    };
    for i in 0..n {
        let shift = if e == Endian::Little { i } else { n - 1 - i };
        out.push((value >> (shift * 8)) as u8);
    }
}
pub fn decode_ulaw(value: u8) -> i16 {
    let u = !value;
    let t = (((u & 15) as i32) * 8 + 132) << ((u >> 4) & 7);
    if u & 128 != 0 {
        (132 - t) as i16
    } else {
        (t - 132) as i16
    }
}
pub fn encode_ulaw(sample: i16) -> u8 {
    let n = sample as i32;
    let mask = if n < 0 { 0x7f } else { 0xff };
    let n = n.abs().min(32635) + 132;
    let mut exp = 7u8;
    let mut bit = 0x4000;
    while exp > 0 && n & bit == 0 {
        exp -= 1;
        bit >>= 1;
    }
    ((exp << 4) | ((n >> (exp + 3)) & 15) as u8) ^ mask
}
pub fn decode_alaw(value: u8) -> i16 {
    let a = value ^ 0x55;
    let seg = (a >> 4) & 7;
    let mut n = ((a & 15) as i32) << 4;
    n += if seg == 0 { 8 } else { 264 };
    if seg > 1 {
        n <<= seg - 1;
    }
    if a & 128 != 0 { n as i16 } else { -n as i16 }
}
pub fn encode_alaw(sample: i16) -> u8 {
    let n = sample as i32;
    let mask = if n >= 0 { 0xd5 } else { 0x55 };
    let n = if n >= 0 { n } else { -n - 1 };
    let mut seg = 0u8;
    let mut bound = 255;
    while n > bound && seg < 7 {
        seg += 1;
        bound = (bound << 1) | 1;
    }
    let shift = if seg == 0 { 4 } else { seg + 3 };
    ((seg << 4) | ((n >> shift) & 15) as u8) ^ mask
}

const IMA_STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
pub(crate) fn decode_ima(
    bytes: &[u8],
    channels: usize,
    rate: u32,
    block: usize,
    samples_per_block: usize,
    limits: &Limits,
) -> Result<Audio> {
    if !(1..=2).contains(&channels)
        || block < channels * 4
        || samples_per_block == 0
        || bytes.len() % block != 0
    {
        return Err(Error::decode("Invalid IMA ADPCM blocks"));
    }
    let frames = (bytes.len() / block)
        .checked_mul(samples_per_block)
        .ok_or_else(|| Error::limit("ADPCM frame count overflow"))?;
    limits.check(frames, channels, rate)?;
    let mut data: Vec<Vec<f32>> = (0..channels).map(|_| Vec::with_capacity(frames)).collect();
    for b in bytes.chunks_exact(block) {
        let mut predictor = vec![0i32; channels];
        let mut index = vec![0i32; channels];
        for c in 0..channels {
            predictor[c] = i16::from_le_bytes([b[c * 4], b[c * 4 + 1]]) as i32;
            index[c] = b[c * 4 + 2] as i32;
            if index[c] > 88 {
                return Err(Error::decode("Invalid ADPCM step index"));
            }
            data[c].push(predictor[c] as f32 / 32768.0);
        }
        let mut pos = channels * 4;
        let mut emitted = vec![1; channels];
        while pos < b.len() {
            for c in 0..channels {
                let count = if channels == 1 { b.len() - pos } else { 4 };
                let chunk = b
                    .get(pos..pos + count)
                    .ok_or_else(|| Error::decode("Incomplete ADPCM channel group"))?;
                for &v in chunk {
                    for shift in [0, 4] {
                        if emitted[c] >= samples_per_block {
                            break;
                        }
                        let code = (v >> shift) & 15;
                        let step = IMA_STEPS[index[c] as usize];
                        let diff = (step >> 3)
                            + if code & 1 != 0 { step >> 2 } else { 0 }
                            + if code & 2 != 0 { step >> 1 } else { 0 }
                            + if code & 4 != 0 { step } else { 0 };
                        predictor[c] = (predictor[c] + if code & 8 != 0 { -diff } else { diff })
                            .clamp(-32768, 32767);
                        index[c] = (index[c] + [-1, -1, -1, -1, 2, 4, 6, 8][(code & 7) as usize])
                            .clamp(0, 88);
                        data[c].push(predictor[c] as f32 / 32768.0);
                        emitted[c] += 1;
                    }
                }
                pos += count;
            }
        }
        if emitted.iter().any(|&n| n != samples_per_block) {
            return Err(Error::decode("ADPCM block has too few samples"));
        }
    }
    Audio::decoded(data, rate, DecodeInfo::default())
}
