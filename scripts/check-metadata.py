"""Optional independent metadata/cover/PCM verification; never required by Rust CI.

Install mutagen separately, generate examples/tag_audio output, then run:
python scripts/check-metadata.py target/tag-demo tests/fixtures/cover.png --ffmpeg ffmpeg
"""
import argparse
import json
import subprocess
from pathlib import Path

import mutagen
from mutagen.flac import FLAC
from mutagen.id3 import ID3


def ffmpeg(binary, *arguments):
    return subprocess.run(
        [binary, "-hide_banner", "-loglevel", "error", *map(str, arguments)],
        check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    ).stdout


def text_metadata(payload):
    # FFmetadata escapes punctuation and embeds escaped newlines in values.
    lines, current, escaped = [], [], False
    for character in payload.decode("utf-8"):
        if escaped:
            current.append(character)
            escaped = False
        elif character == "\\":
            escaped = True
        elif character == "\n":
            lines.append("".join(current))
            current = []
        else:
            current.append(character)
    if current:
        lines.append("".join(current))
    return dict(line.split("=", 1) for line in lines if "=" in line and not line.startswith(";"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("cover", type=Path)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    args = parser.parse_args()
    expected = {
        "title": "Título 日本語 🎵", "artist": "José", "album": "Álbum",
        "album_artist": "Equipo", "track": "3/12", "disc": "1/2", "date": "2026",
        "genre": "Chiptune", "comment": "Primera línea\nSegunda", "isrc": "ESABC2600001",
    }
    cover = args.cover.read_bytes()
    mime = "image/png" if cover.startswith(b"\x89PNG") else "image/jpeg"
    id3 = ID3(args.directory / "tagged.mp3")
    assert id3.version == (2, 4, 0), id3.version
    for frame, name in {
        "TIT2": "title", "TPE1": "artist", "TALB": "album", "TPE2": "album_artist",
        "TRCK": "track", "TPOS": "disc", "TDRC": "date", "TCON": "genre", "TSRC": "isrc",
    }.items():
        assert str(id3[frame].text[0]) == expected[name], (frame, id3[frame])
    assert id3.getall("COMM")[0].text == [expected["comment"]]
    picture = id3.getall("APIC")[0]
    assert picture.type == 3 and picture.mime == mime and picture.data == cover
    flac = FLAC(args.directory / "tagged.flac")
    for name, value in expected.items():
        native = {"album_artist": "albumartist", "track": "tracknumber", "disc": "discnumber"}.get(name, name)
        assert flac[native] == [value], (native, flac.get(native))
    assert flac.pictures[0].type == 3
    assert flac.pictures[0].mime == mime and flac.pictures[0].data == cover
    for extension in ("wav", "flac", "mp3"):
        path = args.directory / ("tagged." + extension)
        external = {key.lower(): value for key, value in text_metadata(ffmpeg(args.ffmpeg, "-i", path, "-f", "ffmetadata", "-")).items()}
        # FFmpeg exposes the standard MP3 TSRC frame under its native frame name.
        if "tsrc" in external:
            external["isrc"] = external.pop("tsrc")
        fields = expected if extension != "wav" else {key: value for key, value in expected.items() if key not in {"album_artist", "disc", "isrc"}}
        for key, value in fields.items():
            assert external.get(key) == value, (extension, key, external)
        if extension != "wav":
            image = ffmpeg(args.ffmpeg, "-i", path, "-map", "0:v:0", "-c", "copy", "-f", "image2pipe", "-")
            assert image == cover, extension + " extracted image differs"
        pcm = ffmpeg(args.ffmpeg, "-i", path, "-map", "0:a:0", "-f", "f32le", "-")
        plain = ffmpeg(args.ffmpeg, "-i", args.directory / ("plain." + extension), "-map", "0:a:0", "-f", "f32le", "-")
        assert pcm == plain, extension + " PCM differs"
    version = subprocess.run([args.ffmpeg, "-version"], check=True, stdout=subprocess.PIPE).stdout.decode().splitlines()[0]
    print(json.dumps({"ffmpeg": version, "mutagen": mutagen.version_string,
                      "formats": ["wav", "flac", "mp3"], "tags": "pass",
                      "cover_bytes": len(cover), "cover_extraction": "pass", "pcm_unchanged": "pass"}))


if __name__ == "__main__":
    main()
