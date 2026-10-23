//! Write/read publishing tags. Creates synthetic audio, never opens an audio device.
//! cargo run --example tag_audio -- target/tag-demo tests/fixtures/cover.png
use audiobox::{
    Audio, AudioFormat, CoverArt, EncodeOptions, Tags, decode, encode, encode_with_options,
};
use std::path::PathBuf;

fn main() -> audiobox::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let directory = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| "target/tag-demo".into());
    let cover = if let Some(path) = args.next() {
        let data = std::fs::read(path)?;
        let mime = if data.starts_with(b"\x89PNG") {
            "image/png"
        } else {
            "image/jpeg"
        };
        Some(CoverArt::new(mime, data)?)
    } else {
        None
    };
    std::fs::create_dir_all(&directory)?;
    let audio = Audio::new(
        vec![(0..4096).map(|n| (n as f32 * 0.0576).sin() * 0.2).collect()],
        48_000,
    )?;
    let full = Tags {
        title: Some("Título 日本語 🎵".into()),
        artist: Some("José".into()),
        album: Some("Álbum".into()),
        album_artist: Some("Equipo".into()),
        track: Some("3/12".into()),
        disc: Some("1/2".into()),
        year: Some("2026".into()),
        genre: Some("Chiptune".into()),
        comment: Some("Primera línea\nSegunda".into()),
        isrc: Some("ESABC2600001".into()),
        cover,
    };
    for (format, extension) in [
        (AudioFormat::Wav, "wav"),
        (AudioFormat::Flac, "flac"),
        (AudioFormat::Mp3, "mp3"),
    ] {
        let tags = if format == AudioFormat::Wav {
            Tags {
                album_artist: None,
                disc: None,
                isrc: None,
                cover: None,
                ..full.clone()
            }
        } else {
            full.clone()
        };
        let bytes = encode_with_options(
            &audio,
            format,
            &EncodeOptions {
                tags: tags.clone(),
                ..Default::default()
            },
        )?;
        let decoded = decode(&bytes)?;
        assert_eq!(decoded.info().tags, tags);
        std::fs::write(directory.join(format!("tagged.{extension}")), bytes)?;
        std::fs::write(
            directory.join(format!("plain.{extension}")),
            encode(&audio, format)?,
        )?;
        println!(
            "{extension}: tags round-trip, {} audio frames",
            decoded.frames()
        );
    }
    Ok(())
}
