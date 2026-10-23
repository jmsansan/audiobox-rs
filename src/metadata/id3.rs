//! ID3v2.4 framing/text layouts: <https://id3.org/id3v2.4.0-structure>
//! Native frame layouts: <https://id3.org/id3v2.4.0-frames>
use super::{Budget, CoverArt, Tags, canonical, merged_text, utf8};
use crate::{AudioMetadata, EncodeOptions, Error, Result};

fn syncsafe(bytes: &[u8]) -> Result<usize> {
    if bytes.len() != 4 || bytes.iter().any(|b| b & 0x80 != 0) {
        return Err(Error::decode("Invalid ID3 synchsafe size"));
    }
    Ok(bytes.iter().fold(0, |n, &b| (n << 7) | b as usize))
}
fn put_size(out: &mut Vec<u8>, size: usize) {
    for shift in [21, 14, 7, 0] {
        out.push(((size >> shift) & 127) as u8);
    }
}
fn frame_id(key: &str) -> &'static [u8; 4] {
    match canonical(key) {
        Some("title") => b"TIT2",
        Some("artist") => b"TPE1",
        Some("album") => b"TALB",
        Some("album_artist") => b"TPE2",
        Some("track") => b"TRCK",
        Some("disc") => b"TPOS",
        Some("year") => b"TDRC",
        Some("genre") => b"TCON",
        Some("isrc") => b"TSRC",
        Some("comment") => b"COMM",
        _ => b"TXXX",
    }
}
fn frame_key(id: &[u8]) -> Option<&'static str> {
    match id {
        b"TIT2" => Some("title"),
        b"TPE1" => Some("artist"),
        b"TALB" => Some("album"),
        b"TPE2" => Some("album_artist"),
        b"TRCK" => Some("track"),
        b"TPOS" => Some("disc"),
        b"TDRC" | b"TYER" => Some("year"),
        b"TCON" => Some("genre"),
        b"TSRC" => Some("isrc"),
        _ => None,
    }
}

pub(crate) fn encode(options: &EncodeOptions) -> Result<Vec<u8>> {
    let metadata = merged_text(options);
    if metadata.is_empty() && options.tags.cover.is_none() {
        return Ok(Vec::new());
    }
    let mut budget = Budget::default();
    let mut seen = std::collections::BTreeSet::new();
    for (key, value) in &metadata {
        budget.field()?;
        let id = frame_id(key);
        if id != b"TXXX" && !seen.insert(*id) {
            return Err(Error::encode("Duplicate ID3 metadata alias"));
        }
        let overhead = if id == b"COMM" {
            5
        } else if id == b"TXXX" {
            2 + key.len()
        } else {
            1
        };
        budget.add(10 + overhead)?;
        budget.add(value.len())?;
    }
    if let Some(cover) = &options.tags.cover {
        budget.field()?;
        budget.add(14 + cover.mime_type.len())?;
        budget.add(cover.data.len())?;
    }
    let mut frames = Vec::new();
    for (key, value) in metadata {
        let id = frame_id(&key);
        let mut body = vec![3]; // UTF-8
        if id == b"COMM" {
            body.extend_from_slice(b"eng\0");
        }
        if id == b"TXXX" {
            body.extend_from_slice(key.as_bytes());
            body.push(0);
        }
        body.extend_from_slice(value.as_bytes());
        frames.extend_from_slice(id);
        put_size(&mut frames, body.len());
        frames.extend_from_slice(&[0, 0]);
        frames.extend_from_slice(&body);
    }
    if let Some(cover) = &options.tags.cover {
        let size = 4 + cover.mime_type.len() + cover.data.len();
        frames.extend_from_slice(b"APIC");
        put_size(&mut frames, size);
        frames.extend_from_slice(&[0, 0, 3]);
        frames.extend_from_slice(cover.mime_type.as_bytes());
        frames.extend_from_slice(&[0, 3, 0]);
        frames.extend_from_slice(&cover.data);
    }
    let mut out = b"ID3\x04\x00\x00".to_vec();
    put_size(&mut out, frames.len());
    out.extend_from_slice(&frames);
    Ok(out)
}

pub(crate) fn read(bytes: &[u8]) -> Result<(AudioMetadata, Tags)> {
    let mut metadata = AudioMetadata::new();
    let mut cover = None;
    if !bytes.starts_with(b"ID3") {
        return Ok((metadata, Tags::default()));
    }
    let mut r = crate::io::Reader::new(bytes);
    let header = r.take(10)?;
    let size = syncsafe(&header[6..10])?;
    let mut budget = Budget::default();
    budget.add(size)?;
    let body = r.take(size)?;
    if header[3] == 4 && header[5] & 16 != 0 {
        let footer = r.take(10)?;
        if !footer.starts_with(b"3DI") {
            return Err(Error::decode("Invalid ID3 footer"));
        }
    }
    let version = header[3];
    // Preserve audio decoding of older/unknown versions and whole-tag v2.3 unsync.
    if !matches!(version, 3 | 4) || (version == 3 && header[5] & 0x80 != 0) {
        return Ok((metadata, Tags::default()));
    }
    let mut r = crate::io::Reader::new(body);
    if header[5] & 0x40 != 0 {
        let extended = if version == 4 {
            syncsafe(r.take(4)?)?
        } else {
            (r.u32(false)? as usize)
                .checked_add(4)
                .ok_or_else(|| Error::decode("ID3 extended header size overflow"))?
        };
        if extended < 4 {
            return Err(Error::decode("Invalid ID3 extended header"));
        }
        r.take(extended - 4)?;
    }
    while r.pos < body.len() {
        if body[r.pos] == 0 {
            break;
        }
        budget.field()?;
        let frame = r.take(10)?;
        if !frame[..4]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            return Err(Error::decode("Invalid ID3 frame identifier"));
        }
        let n = if version == 4 {
            syncsafe(&frame[4..8])?
        } else {
            u32::from_be_bytes(frame[4..8].try_into().unwrap()) as usize
        };
        let payload = r.take(n)?;
        if (version == 3 && frame[9] != 0) || (version == 4 && frame[9] & 0x0c != 0) {
            continue;
        }
        let unescaped;
        let mut payload = if version == 4 && (frame[9] & 2 != 0 || header[5] & 0x80 != 0) {
            unescaped = unsync(payload);
            unescaped.as_slice()
        } else {
            payload
        };
        if version == 4 {
            if frame[9] & 0x40 != 0 {
                payload = payload
                    .get(1..)
                    .ok_or_else(|| Error::decode("Truncated ID3 group"))?;
            }
            if frame[9] & 1 != 0 {
                let indicator = payload
                    .get(..4)
                    .ok_or_else(|| Error::decode("Truncated ID3 data length"))?;
                syncsafe(indicator)?;
                payload = &payload[4..];
            }
        }
        if let Some(key) = frame_key(&frame[..4]) {
            let (&encoding, data) = payload
                .split_first()
                .ok_or_else(|| Error::decode("Missing ID3 text encoding"))?;
            metadata.insert(key.into(), decode_text(encoding, data)?);
        } else if &frame[..4] == b"COMM" {
            let encoding = *payload
                .first()
                .ok_or_else(|| Error::decode("Missing ID3 comment encoding"))?;
            let data = payload
                .get(4..)
                .ok_or_else(|| Error::decode("Truncated ID3 comment language"))?;
            let (description, text) = terminated(encoding, data)?;
            metadata.insert(
                "comment".into(),
                following_text(encoding, description, text)?,
            );
        } else if &frame[..4] == b"TXXX" {
            let (&encoding, data) = payload
                .split_first()
                .ok_or_else(|| Error::decode("Missing ID3 text encoding"))?;
            let (key, text) = terminated(encoding, data)?;
            let value = following_text(encoding, key, text)?;
            let key = decode_text(encoding, key)?;
            if !key.is_empty() {
                metadata.insert(key, value);
            }
        } else if &frame[..4] == b"APIC" {
            let (&encoding, data) = payload
                .split_first()
                .ok_or_else(|| Error::decode("Missing ID3 picture encoding"))?;
            let (mime, rest) = terminated(0, data)?;
            let (&kind, rest) = rest
                .split_first()
                .ok_or_else(|| Error::decode("Missing ID3 picture type"))?;
            let (_, data) = terminated(encoding, rest)?;
            let mime = utf8(mime)?;
            if kind == 3 && cover.is_none() && matches!(mime, "image/jpeg" | "image/png") {
                let image = CoverArt {
                    mime_type: mime.into(),
                    data: data.to_vec(),
                };
                image
                    .validate()
                    .map_err(|_| Error::decode("Invalid ID3 front cover"))?;
                cover = Some(image);
            }
        }
    }
    let mut tags = Tags::from_native(&metadata);
    tags.cover = cover;
    Ok((metadata, tags))
}

fn unsync(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        out.push(bytes[i]);
        if bytes[i] == 255 && bytes.get(i + 1) == Some(&0) {
            i += 1;
        }
        i += 1;
    }
    out
}
fn terminated(encoding: u8, bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    let width = if matches!(encoding, 1 | 2) { 2 } else { 1 };
    let end = if width == 2 {
        bytes
            .chunks_exact(2)
            .position(|c| c == [0, 0])
            .map(|n| n * 2)
    } else {
        bytes.iter().position(|&b| b == 0)
    }
    .ok_or_else(|| Error::decode("Missing ID3 string terminator"))?;
    Ok((&bytes[..end], &bytes[end + width..]))
}
fn decode_text(encoding: u8, bytes: &[u8]) -> Result<String> {
    decode_text_order(encoding, bytes, false)
}
fn following_text(encoding: u8, descriptor: &[u8], bytes: &[u8]) -> Result<String> {
    decode_text_order(
        encoding,
        bytes,
        encoding == 1 && descriptor.starts_with(&[255, 254]),
    )
}
fn decode_text_order(encoding: u8, bytes: &[u8], inherited_little: bool) -> Result<String> {
    match encoding {
        0 => Ok(bytes
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| char::from(b))
            .collect()),
        3 => Ok(utf8(bytes)?.trim_end_matches('\0').to_owned()),
        1 | 2 => {
            let (little, bytes) = if bytes.starts_with(&[255, 254]) {
                (true, &bytes[2..])
            } else if bytes.starts_with(&[254, 255]) {
                (false, &bytes[2..])
            } else {
                (inherited_little, bytes)
            };
            if bytes.len() % 2 != 0 {
                return Err(Error::decode("Odd ID3 UTF-16 text length"));
            }
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| {
                    if little {
                        u16::from_le_bytes([c[0], c[1]])
                    } else {
                        u16::from_be_bytes([c[0], c[1]])
                    }
                })
                .collect();
            String::from_utf16(&units)
                .map(|s| s.trim_end_matches('\0').to_owned())
                .map_err(|_| Error::decode("Invalid ID3 UTF-16 text"))
        }
        _ => Err(Error::decode("Unsupported ID3 text encoding")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tag(id: &[u8; 4], flags: u8, body: &[u8], version: u8) -> Vec<u8> {
        let mut frame = id.to_vec();
        if version == 4 {
            put_size(&mut frame, body.len());
        } else {
            frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
        }
        frame.extend_from_slice(&[0, flags]);
        frame.extend_from_slice(body);
        let mut out = vec![b'I', b'D', b'3', version, 0, 0];
        put_size(&mut out, frame.len());
        out.extend_from_slice(&frame);
        out
    }
    #[test]
    fn external_latin1_and_utf16_frames_and_aligned_descriptors() {
        let (metadata, _) = read(&tag(b"TIT2", 0, b"\0Jos\xe9", 3)).unwrap();
        assert_eq!(metadata["title"], "José");
        let mut comment = b"\x01eng\xff\xfe\0\0".to_vec();
        // The descriptor carries the BOM; the next string inherits its byte order.
        for c in "España".encode_utf16() {
            comment.extend_from_slice(&c.to_le_bytes());
        }
        let (metadata, _) = read(&tag(b"COMM", 0, &comment, 3)).unwrap();
        assert_eq!(metadata["comment"], "España");
        // U+0100 contains a zero byte but is not a UTF-16 string terminator.
        let body = b"\x01\xff\xfe\0\x01\0\0\xff\xfev\0";
        let (metadata, _) = read(&tag(b"TXXX", 0, body, 4)).unwrap();
        assert_eq!(metadata["Ā"], "v");
    }
    #[test]
    fn oversized_v23_extended_headers_are_errors_on_all_pointer_widths() {
        let mut out = b"ID3\x03\0\x40\0\0\0\x04".to_vec();
        out.extend_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(read(&out).unwrap_err().code, crate::ErrorCode::DecodeError);
    }

    #[test]
    fn frame_unsynchronisation_preserves_cover_bytes() {
        let original = b"\xff\xd8\xff\xe0\0\xff\0\xff\xd9";
        let mut body = b"\x03image/jpeg\0\x03\0".to_vec();
        for (i, &byte) in original.iter().enumerate() {
            body.push(byte);
            if byte == 255 && original.get(i + 1).is_none_or(|b| *b == 0 || *b >= 224) {
                body.push(0);
            }
        }
        let (_, tags) = read(&tag(b"APIC", 2, &body, 4)).unwrap();
        assert_eq!(tags.cover.unwrap().data, original);
    }
}
