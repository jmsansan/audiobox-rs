//! Publishing tags for MP3 (ID3v2.4), FLAC and WAV INFO.
//!
//! [`Tags`] gives known fields one spelling across containers; the native
//! [`AudioMetadata`] map remains available for custom fields and existing callers.
//! [`EncodeOptions::tags`] and [`DecodeInfo::tags`](crate::DecodeInfo::tags) carry
//! structured tags; whole decoding exposes them through [`Audio::info`](crate::Audio::info).
//! Incremental `stream::Decoder` reads samples and does not expose tag tables.
//!
//! MP3 and FLAC support every field and a JPEG/PNG front cover. WAV INFO supports
//! title, artist, album, track, year, genre and comment. Unsupported structured
//! fields/covers return `EncodeError`; they are never silently omitted. Native WAV
//! keys remain four-character codes, and FLAC keys remain lowercase Vorbis names.
//! MP3 native keys use the structured field names, with custom keys in TXXX frames.
//!
//! Text is UTF-8 on writing; embedded NUL is rejected. Readers validate UTF-8 in
//! new tag data (MP3 also reads Latin-1/UTF-16). Metadata is bounded before copying
//! by [`MAX_METADATA_BYTES`] and [`MAX_METADATA_FIELDS`], in addition to container
//! limits. These bound encoded payloads, not allocator overhead or duplicate native
//! and structured strings. Cover bytes are preserved without decoding the image;
//! MIME/signature validation is not a full image integrity check. FLAC's informational
//! image dimensions are written as zero (unknown), as specified in RFC 9639 §8.8.
use crate::{AudioFormat, AudioMetadata, EncodeOptions, Error, Result};

pub(crate) mod id3;

/// Maximum combined supported metadata payload, 16 MiB minus one byte.
pub const MAX_METADATA_BYTES: usize = 0xff_ffff;
/// Maximum number of text fields/frames accepted in a tag table.
pub const MAX_METADATA_FIELDS: usize = 4096;

/// Caller-supplied JPEG/PNG bytes for a front cover. No image transcoding occurs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoverArt {
    pub mime_type: String,
    pub data: Vec<u8>,
}

impl CoverArt {
    pub fn new(mime_type: impl Into<String>, data: Vec<u8>) -> Result<Self> {
        let cover = Self {
            mime_type: mime_type.into(),
            data,
        };
        cover.validate()?;
        Ok(cover)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.data.len() > MAX_METADATA_BYTES {
            return Err(Error::limit("Cover exceeds metadata limit"));
        }
        let valid = match self.mime_type.as_str() {
            "image/png" => self.data.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => self.data.starts_with(b"\xff\xd8\xff"),
            _ => false,
        };
        if !valid {
            return Err(Error::encode(
                "A cover needs matching image/jpeg or image/png bytes",
            ));
        }
        Ok(())
    }
}

/// Structured publishing fields. Track/disc text can be `1` or `1/12`; year is
/// recording-date text, normally `2026` (MP3 TDRC / FLAC DATE / WAV ICRD).
/// Optional values are distinct from absent fields, including empty strings.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track: Option<String>,
    pub disc: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub comment: Option<String>,
    pub isrc: Option<String>,
    pub cover: Option<CoverArt>,
}

impl Tags {
    pub fn is_empty(&self) -> bool {
        self.cover.is_none() && self.fields().all(|(_, value)| value.is_none())
    }

    pub(crate) fn fields(&self) -> impl Iterator<Item = (&'static str, Option<&String>)> {
        [
            ("title", self.title.as_ref()),
            ("artist", self.artist.as_ref()),
            ("album", self.album.as_ref()),
            ("album_artist", self.album_artist.as_ref()),
            ("track", self.track.as_ref()),
            ("disc", self.disc.as_ref()),
            ("year", self.year.as_ref()),
            ("genre", self.genre.as_ref()),
            ("comment", self.comment.as_ref()),
            ("isrc", self.isrc.as_ref()),
        ]
        .into_iter()
    }

    pub(crate) fn from_native(metadata: &AudioMetadata) -> Self {
        let mut out = Self::default();
        for (key, value) in metadata {
            let field = match canonical(key) {
                Some("title") => &mut out.title,
                Some("artist") => &mut out.artist,
                Some("album") => &mut out.album,
                Some("album_artist") => &mut out.album_artist,
                Some("track") => &mut out.track,
                Some("disc") => &mut out.disc,
                Some("year") => &mut out.year,
                Some("genre") => &mut out.genre,
                Some("comment") => &mut out.comment,
                Some("isrc") => &mut out.isrc,
                _ => continue,
            };
            *field = Some(value.clone());
        }
        out
    }
}

pub(crate) fn canonical(key: &str) -> Option<&'static str> {
    match key.to_ascii_lowercase().as_str() {
        "title" | "inam" => Some("title"),
        "artist" | "iart" => Some("artist"),
        "album" | "iprd" => Some("album"),
        "album_artist" | "albumartist" => Some("album_artist"),
        "track" | "tracknumber" | "iprt" | "itrk" => Some("track"),
        "disc" | "discnumber" => Some("disc"),
        "year" | "date" | "icrd" => Some("year"),
        "genre" | "ignr" => Some("genre"),
        "comment" | "description" | "icmt" => Some("comment"),
        "isrc" => Some("isrc"),
        _ => None,
    }
}

pub(crate) fn validate_options(options: &EncodeOptions, format: AudioFormat) -> Result<()> {
    if options.wav_channel_mask.is_some() && format != AudioFormat::Wav {
        return Err(Error::invalid("A WAVE channel mask requires WAV output"));
    }
    if format == AudioFormat::Wav
        && (options.tags.album_artist.is_some()
            || options.tags.disc.is_some()
            || options.tags.isrc.is_some())
    {
        return Err(Error::encode(
            "WAV INFO has no supported album artist, disc or ISRC field",
        ));
    }
    if !options.tags.is_empty()
        && !matches!(
            format,
            AudioFormat::Wav | AudioFormat::Flac | AudioFormat::Mp3
        )
    {
        return Err(Error::encode(
            "Structured tags are supported by WAV, FLAC and MP3",
        ));
    }
    if options.tags.cover.is_some() && !matches!(format, AudioFormat::Flac | AudioFormat::Mp3) {
        return Err(Error::encode("Front covers are supported by FLAC and MP3"));
    }
    let mut budget = Budget::default();
    for (key, value) in &options.metadata {
        if key.is_empty() {
            return Err(Error::encode("Metadata keys cannot be empty"));
        }
        text(key)?;
        text(value)?;
        budget.field()?;
        budget.add(key.len())?;
        budget.add(value.len())?;
    }
    for (key, value) in options.tags.fields() {
        if let Some(value) = value {
            text(value)?;
            budget.field()?;
            budget.add(key.len())?;
            budget.add(value.len())?;
            if options
                .metadata
                .keys()
                .any(|native| canonical(native) == Some(key))
            {
                return Err(Error::encode(format!(
                    "Tag {key} is supplied in both tags and metadata"
                )));
            }
        }
    }
    if let Some(cover) = &options.tags.cover {
        cover.validate()?;
        budget.add(cover.data.len())?;
        budget.add(cover.mime_type.len())?;
    }
    Ok(())
}

pub(crate) fn merged_text(options: &EncodeOptions) -> AudioMetadata {
    let mut out = options.metadata.clone();
    for (key, value) in options.tags.fields() {
        if let Some(value) = value {
            out.insert(key.into(), value.clone());
        }
    }
    out
}

pub(crate) fn wav_text(options: &EncodeOptions) -> AudioMetadata {
    let mut out = options.metadata.clone();
    for (key, value) in options.tags.fields() {
        let native = match key {
            "title" => "INAM",
            "artist" => "IART",
            "album" => "IPRD",
            "track" => "IPRT",
            "year" => "ICRD",
            "genre" => "IGNR",
            "comment" => "ICMT",
            _ => continue,
        };
        if let Some(value) = value {
            out.insert(native.into(), value.clone());
        }
    }
    out
}

fn text(value: &str) -> Result<()> {
    if value.contains('\0') {
        Err(Error::encode("Metadata text cannot contain NUL"))
    } else {
        Ok(())
    }
}

pub(crate) fn utf8(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|_| Error::decode("Invalid UTF-8 metadata"))
}

#[derive(Default)]
pub(crate) struct Budget {
    bytes: usize,
    fields: usize,
}
impl Budget {
    pub fn add(&mut self, bytes: usize) -> Result<()> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|&n| n <= MAX_METADATA_BYTES)
            .ok_or_else(|| Error::limit("Metadata exceeds 16 MiB minus one byte"))?;
        Ok(())
    }
    pub fn field(&mut self) -> Result<()> {
        self.fields += 1;
        if self.fields > MAX_METADATA_FIELDS {
            Err(Error::limit("Too many metadata fields"))
        } else {
            Ok(())
        }
    }
}

pub(crate) fn flac_comments(options: &EncodeOptions) -> Result<Option<Vec<u8>>> {
    let mut metadata = options.metadata.clone();
    for (key, value) in options.tags.fields() {
        let name = match key {
            "album_artist" => "albumartist",
            "track" => "tracknumber",
            "disc" => "discnumber",
            "year" => "date",
            _ => key,
        };
        if let Some(value) = value {
            metadata.insert(name.into(), value.clone());
        }
    }
    if metadata.is_empty() {
        return Ok(None);
    }
    let mut budget = Budget::default();
    budget.add(16)?; // vendor length, "audiobox", field count
    let mut normalized = AudioMetadata::new();
    for (key, value) in metadata {
        let name = key.to_ascii_uppercase();
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| (0x20..=0x7d).contains(&b) && b != b'=')
        {
            return Err(Error::encode("Invalid FLAC metadata key"));
        }
        if normalized.contains_key(&name) {
            return Err(Error::encode("Duplicate FLAC metadata alias"));
        }
        budget.field()?;
        budget.add(5 + name.len())?;
        budget.add(value.len())?;
        normalized.insert(name, value);
    }
    let mut out = Vec::new();
    crate::io::put32(&mut out, 8, true);
    out.extend_from_slice(b"audiobox");
    crate::io::put32(&mut out, normalized.len() as u32, true);
    for (key, value) in normalized {
        crate::io::put32(&mut out, (key.len() + 1 + value.len()) as u32, true);
        out.extend_from_slice(key.as_bytes());
        out.push(b'=');
        out.extend_from_slice(value.as_bytes());
    }
    Ok(Some(out))
}

pub(crate) fn flac_picture(cover: &CoverArt) -> Result<Vec<u8>> {
    cover.validate()?;
    let size = 32 + cover.mime_type.len() + cover.data.len();
    Budget::default().add(size)?;
    let mut out = Vec::with_capacity(size);
    crate::io::put32(&mut out, 3, false);
    crate::io::put32(&mut out, cover.mime_type.len() as u32, false);
    out.extend_from_slice(cover.mime_type.as_bytes());
    for _ in 0..5 {
        crate::io::put32(&mut out, 0, false);
    } // description, dimensions/depth/colours unknown
    crate::io::put32(&mut out, cover.data.len() as u32, false);
    out.extend_from_slice(&cover.data);
    Ok(out)
}

pub(crate) fn read_flac_picture(bytes: &[u8]) -> Result<Option<CoverArt>> {
    let mut r = crate::io::Reader::new(bytes);
    let kind = r.u32(false)?;
    let n = r.u32(false)? as usize;
    let mime = utf8(r.take(n)?)?;
    let n = r.u32(false)? as usize;
    utf8(r.take(n)?)?;
    r.take(16)?;
    let n = r.u32(false)? as usize;
    let data = r.take(n)?;
    if r.pos != bytes.len() {
        return Err(Error::decode("Trailing FLAC picture data"));
    }
    if kind != 3 || !matches!(mime, "image/jpeg" | "image/png") {
        return Ok(None);
    }
    let cover = CoverArt {
        mime_type: mime.into(),
        data: data.to_vec(),
    };
    cover
        .validate()
        .map_err(|_| Error::decode("Invalid FLAC front cover"))?;
    Ok(Some(cover))
}
