use crate::io::{Reader, put16, put32, put64};
use crate::pcm::{decode_pcm, encode_pcm};
use crate::{
    Audio, AudioFormat, DecodeOptions, EncodeOptions, Endian, Error, Result, SampleFormat,
};

fn finish(mut audio: Audio, format: AudioFormat) -> Audio {
    audio.info.format = Some(format);
    audio
}
fn fmt(bits: u16, float: bool, unsigned: bool) -> Result<SampleFormat> {
    match (bits, float, unsigned) {
        (8, false, true) => Ok(SampleFormat::U8),
        (8, false, false) => Ok(SampleFormat::S8),
        (16, false, _) => Ok(SampleFormat::S16),
        (24, false, _) => Ok(SampleFormat::S24),
        (32, false, _) => Ok(SampleFormat::S32),
        (32, true, _) => Ok(SampleFormat::F32),
        (64, true, _) => Ok(SampleFormat::F64),
        _ => Err(Error::decode("Unsupported PCM bit depth")),
    }
}
fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], bytes: &[u8], le: bool) -> Result<()> {
    out.extend_from_slice(tag);
    put32(
        out,
        u32::try_from(bytes.len()).map_err(|_| Error::encode("Chunk exceeds 4 GiB"))?,
        le,
    );
    out.extend_from_slice(bytes);
    if bytes.len() % 2 != 0 {
        out.push(0);
    }
    Ok(())
}

pub(crate) fn decode_wav(bytes: &[u8], options: &DecodeOptions) -> Result<Audio> {
    let mut r = Reader::new(bytes);
    let magic = r.take(4)?;
    let size = r.u32(true)? as usize;
    if ![b"RIFF".as_slice(), b"RF64", b"BW64"].contains(&magic) || r.take(4)? != b"WAVE" {
        return Err(Error::decode("Invalid WAVE header"));
    }
    let rf64 = magic != b"RIFF";
    if !rf64 && size.checked_add(8).is_none_or(|n| n > bytes.len()) {
        return Err(Error::decode("Truncated RIFF container"));
    }
    let end = if rf64 { bytes.len() } else { size + 8 };
    let mut desc = None;
    let mut payloads = Vec::new();
    let mut data64 = None;
    let mut fact = None;
    let mut metadata = crate::AudioMetadata::new();
    while r.pos < end {
        if end - r.pos < 8 {
            return Err(Error::decode("Truncated WAVE chunk header"));
        }
        let id = r.take(4)?;
        let n = r.u32(true)?;
        let n = if rf64 && id == b"data" && n == u32::MAX {
            data64.ok_or_else(|| Error::decode("Missing RF64 ds64"))?
        } else {
            n as usize
        };
        if n > end - r.pos {
            return Err(Error::decode("WAVE chunk exceeds container"));
        }
        let b = r.take(n)?;
        match id {
            b"ds64" => {
                let mut s = Reader::new(b);
                s.u64(true)?;
                data64 = Some(
                    usize::try_from(s.u64(true)?)
                        .map_err(|_| Error::decode("RF64 size overflow"))?,
                );
            }
            b"fmt " => {
                let mut f = Reader::new(b);
                let mut code = f.u16(true)?;
                let channels = f.u16(true)? as usize;
                let rate = f.u32(true)?;
                f.u32(true)?;
                let align = f.u16(true)? as usize;
                let bits = f.u16(true)?;
                let mut spb = 0;
                if code == 0xfffe {
                    if b.len() < 40 {
                        return Err(Error::decode("Truncated extensible WAVE format"));
                    }
                    let mut ext = Reader::new(&b[16..]);
                    if ext.u16(true)? < 22 {
                        return Err(Error::decode("Invalid extensible format"));
                    }
                    let valid = ext.u16(true)?;
                    if valid != 0 && valid != bits {
                        return Err(Error::decode("Packed valid bits are unsupported"));
                    }
                    ext.u32(true)?;
                    let guid = ext.take(16)?;
                    if guid[2..] != [0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71] {
                        return Err(Error::decode("Unsupported WAVE subformat GUID"));
                    }
                    code = u16::from_le_bytes([guid[0], guid[1]]);
                } else if code == 17 {
                    let mut ext = Reader::new(
                        b.get(16..)
                            .ok_or_else(|| Error::decode("Missing ADPCM extension"))?,
                    );
                    ext.u16(true)?;
                    spb = ext.u16(true)? as usize;
                }
                desc = Some((code, channels, rate, align, bits, spb));
            }
            b"data" => payloads.push(b),
            b"fact" if b.len() >= 4 => {
                fact = Some(u32::from_le_bytes(b[..4].try_into().unwrap()) as usize)
            }
            b"LIST" if b.starts_with(b"INFO") => {
                let mut info = Reader::new(&b[4..]);
                while info.pos + 8 <= info.bytes.len() {
                    let tag = String::from_utf8_lossy(info.take(4)?).to_string();
                    let n = info.u32(true)? as usize;
                    let value = String::from_utf8_lossy(info.take(n)?)
                        .trim_end_matches('\0')
                        .to_string();
                    metadata.insert(tag, value);
                    if n % 2 != 0 {
                        info.take(1)?;
                    }
                }
            }
            _ => {}
        }
        if n % 2 != 0 && r.pos < end {
            r.take(1)?;
        }
    }
    let (code, channels, rate, align, bits, spb) =
        desc.ok_or_else(|| Error::decode("Missing WAVE fmt chunk"))?;
    if payloads.is_empty() {
        return Err(Error::decode("Missing WAVE data chunk"));
    }
    options.limits.check(0, channels, rate)?;
    let size = payloads
        .iter()
        .try_fold(0usize, |n, b| n.checked_add(b.len()))
        .ok_or_else(|| Error::decode("Payload size overflow"))?;
    let format = match code {
        1 => fmt(bits, false, true)?,
        3 => fmt(bits, true, false)?,
        6 if bits == 8 => SampleFormat::Alaw,
        7 if bits == 8 => SampleFormat::Ulaw,
        17 => SampleFormat::S16,
        _ => return Err(Error::decode("Unsupported WAVE encoding")),
    };
    let frames = if code == 17 {
        if align == 0 {
            return Err(Error::decode("Zero ADPCM block alignment"));
        }
        (size / align)
            .checked_mul(spb)
            .ok_or_else(|| Error::limit("ADPCM size overflow"))?
    } else {
        if align != channels * format.bytes() {
            return Err(Error::decode("Invalid WAVE block alignment"));
        }
        size / align
    };
    options.limits.check(frames, channels, rate)?;
    let payload: Vec<u8> = payloads.into_iter().flatten().copied().collect();
    let mut audio = if code == 17 {
        crate::pcm::decode_ima(&payload, channels, rate, align, spb, &options.limits)?
    } else {
        decode_pcm(
            &payload,
            format,
            Endian::Little,
            channels,
            rate,
            &options.limits,
        )?
    };
    if code == 17 {
        if let Some(n) = fact {
            if n > audio.frames() {
                return Err(Error::decode("ADPCM fact exceeds decoded frames"));
            }
            for c in &mut audio.data {
                c.truncate(n);
            }
        }
    }
    audio.info.metadata = metadata;
    Ok(finish(audio, AudioFormat::Wav))
}

pub(crate) fn encode_wav(audio: &Audio, options: &EncodeOptions) -> Result<Vec<u8>> {
    let f = options.sample_format;
    if f == SampleFormat::S8 {
        return Err(Error::encode("WAVE uses unsigned 8-bit PCM; select U8"));
    }
    let code = match f {
        SampleFormat::F32 | SampleFormat::F64 => 3,
        SampleFormat::Alaw => 6,
        SampleFormat::Ulaw => 7,
        _ => 1,
    };
    let align = u16::try_from(audio.channels() * f.bytes())
        .map_err(|_| Error::encode("Block alignment overflow"))?;
    let mut desc = Vec::new();
    put16(&mut desc, code, true);
    put16(&mut desc, audio.channels() as u16, true);
    put32(&mut desc, audio.rate, true);
    put32(
        &mut desc,
        audio
            .rate
            .checked_mul(align as u32)
            .ok_or_else(|| Error::encode("Byte rate overflow"))?,
        true,
    );
    put16(&mut desc, align, true);
    put16(&mut desc, f.bits(), true);
    let pcm = encode_pcm(audio, f, Endian::Little)?;
    let mut out = b"RIFF\0\0\0\0WAVE".to_vec();
    chunk(&mut out, b"fmt ", &desc, true)?;
    if code != 1 {
        let mut fact = Vec::new();
        put32(&mut fact, audio.frames() as u32, true);
        chunk(&mut out, b"fact", &fact, true)?;
    }
    if !options.metadata.is_empty() {
        let mut info = b"INFO".to_vec();
        for (key, value) in &options.metadata {
            let tag = match key.as_str() {
                "title" => *b"INAM",
                "artist" => *b"IART",
                "album" => *b"IPRD",
                "comment" => *b"ICMT",
                _ => {
                    if key.len() != 4 || !key.is_ascii() {
                        return Err(Error::encode(
                            "WAVE metadata keys must be four ASCII characters or title/artist/album/comment",
                        ));
                    }
                    key.as_bytes().try_into().unwrap()
                }
            };
            let mut value = value.as_bytes().to_vec();
            value.push(0);
            chunk(&mut info, &tag, &value, true)?;
        }
        chunk(&mut out, b"LIST", &info, true)?;
    }
    chunk(&mut out, b"data", &pcm, true)?;
    let size = u32::try_from(out.len() - 8).map_err(|_| Error::encode("RIFF exceeds 4 GiB"))?;
    out[4..8].copy_from_slice(&size.to_le_bytes());
    Ok(out)
}

fn extended_rate(b: &[u8]) -> Result<u32> {
    let exp = u16::from_be_bytes(b[..2].try_into().unwrap());
    let mantissa = u64::from_be_bytes(b[2..].try_into().unwrap());
    if exp & 0x8000 != 0 || exp & 0x7fff == 0x7fff {
        return Err(Error::decode("Invalid AIFF sample rate"));
    }
    let rate = (mantissa as f64 * 2f64.powi((exp & 0x7fff) as i32 - 16383 - 63)).round();
    if !rate.is_finite() || rate < 1.0 || rate > u32::MAX as f64 {
        return Err(Error::decode("Invalid AIFF sample rate"));
    }
    Ok(rate as u32)
}
fn write_extended(out: &mut Vec<u8>, rate: u32) {
    let exponent = 31 - rate.leading_zeros();
    put16(out, (exponent + 16383) as u16, false);
    put64(out, (rate as u64) << (63 - exponent), false);
}
pub(crate) fn decode_aiff(bytes: &[u8], options: &DecodeOptions) -> Result<Audio> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != b"FORM" {
        return Err(Error::decode("Invalid AIFF header"));
    }
    let size = r.u32(false)? as usize;
    let form = r.take(4)?;
    if form != b"AIFF" && form != b"AIFC" {
        return Err(Error::decode("Invalid AIFF form"));
    }
    let end = size
        .checked_add(8)
        .filter(|&n| n <= bytes.len() && n >= 12)
        .ok_or_else(|| Error::decode("Truncated AIFF"))?;
    let mut desc = None;
    let mut payload = None;
    let mut metadata = crate::AudioMetadata::new();
    while r.pos < end {
        if end - r.pos < 8 {
            return Err(Error::decode("Truncated AIFF chunk"));
        }
        let id = r.take(4)?;
        let n = r.u32(false)? as usize;
        if n > end - r.pos {
            return Err(Error::decode("AIFF chunk exceeds form"));
        }
        let b = r.take(n)?;
        match id {
            b"COMM" => {
                let mut c = Reader::new(b);
                let channels = c.u16(false)? as usize;
                let frames = c.u32(false)? as usize;
                let bits = c.u16(false)?;
                let rate = extended_rate(c.take(10)?)?;
                let code = if form == b"AIFC" { c.take(4)? } else { b"NONE" };
                let (f, e) = match code {
                    b"NONE" | b"twos" => (fmt(bits, false, false)?, Endian::Big),
                    b"sowt" => (fmt(bits, false, false)?, Endian::Little),
                    b"fl32" | b"FL32" => (SampleFormat::F32, Endian::Big),
                    b"fl64" | b"FL64" => (SampleFormat::F64, Endian::Big),
                    b"alaw" | b"ALAW" => (SampleFormat::Alaw, Endian::Big),
                    b"ulaw" | b"ULAW" => (SampleFormat::Ulaw, Endian::Big),
                    _ => return Err(Error::decode("Unsupported AIFF-C compression")),
                };
                options.limits.check(frames, channels, rate)?;
                desc = Some((channels, frames, rate, f, e));
            }
            b"SSND" => {
                let mut s = Reader::new(b);
                let offset = s.u32(false)? as usize;
                s.u32(false)?;
                s.take(offset)?;
                payload = Some(&b[s.pos..]);
            }
            b"NAME" | b"AUTH" | b"ANNO" | b"(c) " => {
                let key = match id {
                    b"NAME" => "title",
                    b"AUTH" => "artist",
                    b"ANNO" => "comment",
                    _ => "copyright",
                };
                metadata.insert(key.into(), String::from_utf8_lossy(b).into_owned());
            }
            _ => {}
        }
        if n % 2 != 0 && r.pos < end {
            r.take(1)?;
        }
    }
    let (channels, frames, rate, f, e) = desc.ok_or_else(|| Error::decode("Missing AIFF COMM"))?;
    let n = frames
        .checked_mul(channels)
        .and_then(|n| n.checked_mul(f.bytes()))
        .ok_or_else(|| Error::decode("AIFF size overflow"))?;
    let b = payload
        .ok_or_else(|| Error::decode("Missing AIFF SSND"))?
        .get(..n)
        .ok_or_else(|| Error::decode("Truncated AIFF PCM"))?;
    let mut audio = decode_pcm(b, f, e, channels, rate, &options.limits)?;
    audio.info.metadata = metadata;
    Ok(finish(audio, AudioFormat::Aiff))
}
pub(crate) fn encode_aiff(audio: &Audio, options: &EncodeOptions) -> Result<Vec<u8>> {
    let f = if options.sample_format == SampleFormat::U8 {
        SampleFormat::S8
    } else {
        options.sample_format
    };
    let aifc = matches!(
        f,
        SampleFormat::F32 | SampleFormat::F64 | SampleFormat::Alaw | SampleFormat::Ulaw
    );
    let mut out = if aifc {
        b"FORM\0\0\0\0AIFC".to_vec()
    } else {
        b"FORM\0\0\0\0AIFF".to_vec()
    };
    if aifc {
        chunk(&mut out, b"FVER", &0xa2805140u32.to_be_bytes(), false)?;
    }
    let mut comm = Vec::new();
    put16(&mut comm, audio.channels() as u16, false);
    put32(&mut comm, audio.frames() as u32, false);
    put16(&mut comm, f.bits(), false);
    write_extended(&mut comm, audio.rate);
    if aifc {
        comm.extend_from_slice(match f {
            SampleFormat::F32 => b"fl32",
            SampleFormat::F64 => b"fl64",
            SampleFormat::Alaw => b"alaw",
            _ => b"ulaw",
        });
        comm.extend_from_slice(&[0, 0]);
    }
    chunk(&mut out, b"COMM", &comm, false)?;
    for (key, value) in &options.metadata {
        let tag = match key.as_str() {
            "title" => b"NAME",
            "artist" => b"AUTH",
            "comment" => b"ANNO",
            "copyright" => b"(c) ",
            _ => return Err(Error::encode("Unsupported AIFF metadata key")),
        };
        chunk(&mut out, tag, value.as_bytes(), false)?;
    }
    let mut ssnd = vec![0; 8];
    ssnd.extend_from_slice(&encode_pcm(audio, f, Endian::Big)?);
    chunk(&mut out, b"SSND", &ssnd, false)?;
    let n = u32::try_from(out.len() - 8).map_err(|_| Error::encode("AIFF exceeds 4 GiB"))?;
    out[4..8].copy_from_slice(&n.to_be_bytes());
    Ok(out)
}

pub(crate) fn decode_au(bytes: &[u8], options: &DecodeOptions) -> Result<Audio> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != b".snd" {
        return Err(Error::decode("Invalid AU magic"));
    }
    let offset = r.u32(false)? as usize;
    let size = r.u32(false)?;
    let code = r.u32(false)?;
    let rate = r.u32(false)?;
    let channels = r.u32(false)? as usize;
    let f = match code {
        1 => SampleFormat::Ulaw,
        2 => SampleFormat::S8,
        3 => SampleFormat::S16,
        4 => SampleFormat::S24,
        5 => SampleFormat::S32,
        6 => SampleFormat::F32,
        7 => SampleFormat::F64,
        27 => SampleFormat::Alaw,
        _ => return Err(Error::decode("Unsupported AU encoding")),
    };
    if offset < 24 || offset > bytes.len() {
        return Err(Error::decode("Invalid AU data offset"));
    }
    let end = if size == u32::MAX {
        bytes.len()
    } else {
        offset
            .checked_add(size as usize)
            .filter(|&n| n <= bytes.len())
            .ok_or_else(|| Error::decode("Truncated AU data"))?
    };
    let mut audio = decode_pcm(
        &bytes[offset..end],
        f,
        Endian::Big,
        channels,
        rate,
        &options.limits,
    )?;
    let comment = String::from_utf8_lossy(&bytes[24..offset])
        .trim_end_matches('\0')
        .to_string();
    if !comment.is_empty() {
        audio.info.metadata.insert("comment".into(), comment);
    }
    Ok(finish(audio, AudioFormat::Au))
}
pub(crate) fn encode_au(audio: &Audio, options: &EncodeOptions) -> Result<Vec<u8>> {
    let f = if options.sample_format == SampleFormat::U8 {
        SampleFormat::S8
    } else {
        options.sample_format
    };
    let code = match f {
        SampleFormat::Ulaw => 1,
        SampleFormat::S8 => 2,
        SampleFormat::S16 => 3,
        SampleFormat::S24 => 4,
        SampleFormat::S32 => 5,
        SampleFormat::F32 => 6,
        SampleFormat::F64 => 7,
        SampleFormat::Alaw => 27,
        _ => unreachable!(),
    };
    if options.metadata.keys().any(|k| k != "comment") {
        return Err(Error::encode("AU supports only comment metadata"));
    }
    let mut comment = options
        .metadata
        .get("comment")
        .map_or_else(Vec::new, |s| s.as_bytes().to_vec());
    if !comment.is_empty() {
        comment.push(0);
        while (24 + comment.len()) % 8 != 0 {
            comment.push(0);
        }
    }
    let pcm = encode_pcm(audio, f, Endian::Big)?;
    let mut out = b".snd".to_vec();
    put32(&mut out, (24 + comment.len()) as u32, false);
    put32(
        &mut out,
        u32::try_from(pcm.len()).map_err(|_| Error::encode("AU exceeds 4 GiB"))?,
        false,
    );
    put32(&mut out, code, false);
    put32(&mut out, audio.rate, false);
    put32(&mut out, audio.channels() as u32, false);
    out.extend_from_slice(&comment);
    out.extend_from_slice(&pcm);
    Ok(out)
}

pub(crate) fn decode_caf(bytes: &[u8], options: &DecodeOptions) -> Result<Audio> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != b"caff" || r.u16(false)? != 1 {
        return Err(Error::decode("Invalid CAF header"));
    }
    r.u16(false)?;
    let mut desc = None;
    let mut payload = None;
    while r.pos < bytes.len() {
        let id = r.take(4)?;
        let size = r.u64(false)?;
        let n = if size == u64::MAX && id == b"data" {
            bytes.len() - r.pos
        } else {
            usize::try_from(size).map_err(|_| Error::decode("CAF size overflow"))?
        };
        let b = r.take(n)?;
        match id {
            b"desc" => {
                let mut d = Reader::new(b);
                let rate = f64::from_bits(d.u64(false)?);
                if !rate.is_finite() || rate < 1.0 || rate > u32::MAX as f64 || rate.fract() != 0.0
                {
                    return Err(Error::decode("Invalid CAF rate"));
                }
                let code = d.take(4)?;
                let flags = d.u32(false)?;
                let packet = d.u32(false)?;
                let fpp = d.u32(false)?;
                let channels = d.u32(false)? as usize;
                let bits = d.u32(false)?;
                let f = match code {
                    b"lpcm" => fmt(
                        u16::try_from(bits).map_err(|_| Error::decode("Invalid CAF bit depth"))?,
                        flags & 1 != 0,
                        false,
                    )?,
                    b"alaw" => SampleFormat::Alaw,
                    b"ulaw" => SampleFormat::Ulaw,
                    _ => return Err(Error::decode("Unsupported CAF codec")),
                };
                if fpp != 1 || packet as usize != channels * f.bytes() {
                    return Err(Error::decode("Invalid CAF packet shape"));
                }
                desc = Some((
                    channels,
                    rate as u32,
                    f,
                    if flags & 2 != 0 {
                        Endian::Little
                    } else {
                        Endian::Big
                    },
                ));
            }
            b"data" => {
                if b.len() < 4 {
                    return Err(Error::decode("Truncated CAF data"));
                }
                payload = Some(&b[4..]);
            }
            _ => {}
        }
    }
    let (c, rate, f, e) = desc.ok_or_else(|| Error::decode("Missing CAF desc"))?;
    Ok(finish(
        decode_pcm(
            payload.ok_or_else(|| Error::decode("Missing CAF data"))?,
            f,
            e,
            c,
            rate,
            &options.limits,
        )?,
        AudioFormat::Caf,
    ))
}
pub(crate) fn encode_caf(audio: &Audio, options: &EncodeOptions) -> Result<Vec<u8>> {
    if !options.metadata.is_empty() {
        return Err(Error::encode("CAF metadata writing is not supported"));
    }
    let f = options.sample_format;
    let mut out = b"caff\0\x01\0\0desc".to_vec();
    put64(&mut out, 32, false);
    put64(&mut out, (audio.rate as f64).to_bits(), false);
    out.extend_from_slice(match f {
        SampleFormat::Alaw => b"alaw",
        SampleFormat::Ulaw => b"ulaw",
        _ => b"lpcm",
    });
    let flags = if matches!(f, SampleFormat::F32 | SampleFormat::F64) {
        1
    } else if f != SampleFormat::U8 {
        4
    } else {
        0
    } | if options.endian == Endian::Little {
        2
    } else {
        0
    } | 8;
    put32(&mut out, flags, false);
    put32(&mut out, (audio.channels() * f.bytes()) as u32, false);
    put32(&mut out, 1, false);
    put32(&mut out, audio.channels() as u32, false);
    put32(&mut out, f.bits() as u32, false);
    let pcm = encode_pcm(audio, f, options.endian)?;
    out.extend_from_slice(b"data");
    put64(&mut out, (pcm.len() + 4) as u64, false);
    put32(&mut out, 0, false);
    out.extend_from_slice(&pcm);
    Ok(out)
}
