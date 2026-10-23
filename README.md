# audiobox

[![crates.io](https://img.shields.io/crates/v/audiobox.svg)](https://crates.io/crates/audiobox)
[![docs.rs](https://docs.rs/audiobox/badge.svg)](https://docs.rs/audiobox)
[![CI](https://github.com/jmsansan/audiobox-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/jmsansan/audiobox-rs/actions/workflows/ci.yml)
[![MSRV 1.85](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](#install)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

**Audio in safe Rust with zero dependencies: decode, edit, analyze, encode and stream
WAV, FLAC and MP3, and give a game its sound with a real-time mixer, an 8-bit chiptune
synthesizer and sfxr-style sound effects.**

No C libraries, FFmpeg, build scripts or `unsafe`: only the standard library. It
compiles to WebAssembly, and the synthesizer, effects and mixer render the same samples
on every platform.

**[Try it in your browser →](https://jmsansan.github.io/audiobox-rs/playground.html)**
Generate sound effects and play chiptune scores, rendered by this crate through
WebAssembly. The [sandbox](https://jmsansan.github.io/audiobox-rs/sandbox.html) edits
your own audio files the same way.

- **Codecs:** decode WAV, AIFF, CAF, AU, FLAC and MP3; encode all of them, including
  FLAC and MP3. Formats are detected from the bytes.
- **Editing and analysis:** cut, mix, fade, filter, resample, limit, change tempo and
  pitch; measure peaks, RMS, LUFS loudness and spectra.
- **Streaming:** decode WAV, FLAC and MP3 a chunk at a time, with seeking and bounded
  memory, so games play music without decoding it whole.
- **Game audio:** a mixer for audio callbacks (voices, buses, fades, loops, resampling,
  no allocation while rendering), a synthesizer that plays text scores, and a
  sound-effect generator with seeded presets.

## Install

```toml
[dependencies]
audiobox = "0.3"
```

Rust 1.85 or newer. Optional features add modules, never dependencies, so you compile
only what you use:

| Feature | Module | Adds |
| --- | --- | --- |
| `mixer` | [`audiobox::mixer`](https://docs.rs/audiobox/latest/audiobox/mixer/) | Real-time game mixer: voices, buses, fades, loops and resampling |
| `synth` | [`audiobox::synth`](https://docs.rs/audiobox/latest/audiobox/synth/) | 8-bit style synthesizer and text score format |
| `file-stream` | `audiobox::stream::FileStream` (native) | Bounded disk streaming on a reader thread; mixer optional |
| `sfx` | [`audiobox::sfx`](https://docs.rs/audiobox/latest/audiobox/sfx/) | Parametric sound effects with seeded presets (enables `synth`) |
| `assets` | `audiobox::assets` | Background loading on native targets and a budgeted cache of decoded sounds |
| `music` | `audiobox::music` | Beat/bar transitions, crossfades, cues and intensity layers (enables `mixer`) |

```toml
[dependencies]
audiobox = { version = "0.3", features = ["mixer", "synth", "sfx"] }
```

## Examples

Prepare a recording: cut, downmix, resample, normalize and encode.

```rust
use audiobox::{AudioFormat, NormalizeOptions, decode, encode};

fn prepare_recording(bytes: &[u8]) -> audiobox::Result<Vec<u8>> {
    let audio = decode(bytes)?;
    let clip = audio
        .cut(12.0, 90.0)?
        .to_mono()
        .resample(16_000)?
        .normalize(NormalizeOptions::default())?;
    encode(&clip, AudioFormat::Wav)
}
```

Give a game its sound: chiptune music from a text score and a jump effect from a seed,
mixed in the audio callback.

```rust
use audiobox::mixer::{Mixer, MixerConfig, Play};
use audiobox::sfx::{Effect, Preset};
use audiobox::synth::{RenderOptions, Score, Synth};
use std::sync::Arc;

fn game_audio() -> audiobox::Result<()> {
    let (mut mixer, mut handle) = Mixer::new(MixerConfig::default())?;
    let score: Score = "
        tempo 140
        instrument lead wave=pulse duty=25 adsr=4,80,60,60 vibrato=12,170,200
        channel melody = @lead o5 l8 L [c e g >c< g e]2
    ".parse()?;
    let looping = RenderOptions { loops: None, ..RenderOptions::default() };
    handle.play(Synth::new(&score, 48_000, &looping)?, &Play { priority: 255, ..Play::default() })?;

    let jump = Arc::new(Effect::preset(Preset::Jump, 7).render(48_000)?);
    handle.play(jump, &Play { pan: -0.3, ..Play::default() })?;

    let mut buffer = vec![0.0f32; 2 * 512]; // In the audio callback:
    mixer.render(&mut buffer);
    Ok(())
}
```

More in [`examples/`](examples): `convert`, `chiptune` (a song to WAV), `sfx` (effects
to WAV) and `game_audio` (music and effects mixed on an audio thread).

### Music that follows the game

The `music` feature schedules a complete track transition, cue or intensity change
as one mixer command. A track holds up to 16 stems, each a `Sound`: shared decoded
audio, a streaming decoder or a synthesizer. Stems start together and keep advancing
at zero gain, so raising the game intensity reveals the correct musical position.

```rust
use audiobox::music::{Music, TimeSignature, Timing, Track, Transport};
use audiobox::mixer::{BusId, Fade, Mixer, MixerConfig};
use audiobox::Audio;
use std::sync::Arc;

fn start_music(sound: Arc<Audio>) -> audiobox::Result<()> {
    let (mut mixer, mut handle) = Mixer::new(MixerConfig::default())?;
    let grid = Transport::new(48_000, 120.0, TimeSignature::default(), 0)?;
    let music = Music::new(&mut handle, grid, BusId::MASTER)?;
    music.transition(&mut handle, Track::new(sound), 0.0, Timing::BAR, Fade::linear(0.5))?;
    // The host calls mixer.render in its audio callback and polls handle events.
    Ok(())
}
```

`Timing::NOW` applies at receipt, `BEAT` at the next beat and `BAR` at the next bar.
The audio thread resolves timing, using the greater of its current output frame and
`Timing::not_before`. Beat/bar means **strictly after** that frame, including when it
already lies on a boundary. BPM counts quarter notes, rounded to 0.001 BPM; the time
signature defines the beat unit (an eighth note in 6/8). Every grid boundary is
rounded independently to the nearest output frame, avoiding accumulated drift.

Use `Track::layers` for intensity stems. Each `Layer::intensity = [low, high]` maps
intensity to zero gain at/below `low` and full `Layer::gain` at/above `high`, linearly
between. Equal endpoints are a step; the default `[0, 0]` is always on. Schedule
`Music::set_intensity` with a `Fade` to smooth changes. `Music::cue` adds a finite
one-shot source, independently of track transitions and `Music::stop`.

Transitions reuse the mixer's linear, equal-power and exponential fade curves. A
full submission queue changes nothing; insufficient voices reject the complete
group without fading the previous track. `MixerEvent::MusicApplied(id, frame)`
reports when an operation starts, and `MusicRejected` reports audio-side rejection.
Reserve enough mixer voices for both tracks during a crossfade and for cues/effects;
music uses priority 255, and equal-priority sounds can still steal it. Enabling
`music` reserves return storage for up to 16 voices per queued command; poll events
regularly to free containers and finished voices off the audio thread.
As with normal mixer voices, exhausting both return buffers without polling can
free retired data on audio; the reservation covers one render's worst case.

Multilayer sources must share sample rate and known frame length. The caller supplies
matching musical timing, compatible loops and finite cues, including when a source's
length is unknown. A bus pause keeps its stems together but the output-frame grid
continues; the controller does not follow rate/pitch changes. A limiter's lookahead
delays every audible boundary by `handle.latency()` frames.

Run `cargo run --example adaptive_music --features music,synth -- music.wav` for
synthetic stems, an intensity fade, a cue and a bar-aligned transition to a synthetic
FLAC recording. Device output is provided by the host.

### Background loading and cached sounds

With `assets`, a `Loader` reads files and decodes on one worker thread. Poll the
result from the game's control thread, then pass its `Arc<Audio>` to
`MixerHandle::play`. Loading also works without the `mixer` feature.

```rust,no_run
use audiobox::assets::{Loader, LoaderConfig};

fn load_sound() -> audiobox::Result<()> {
    let loader = Loader::new(LoaderConfig {
        cache_bytes: 32 * 1024 * 1024,
        ..LoaderConfig::default()
    })?;
    let mut loading = loader.load_file("sounds/jump.wav")?;
    // Once per game frame, on the control thread:
    if let Some(result) = loading.try_take() {
        let sound = result?; // Shared Arc<Audio>, ready to play.
    }
    // Keep the loader across game frames: dropping it waits for accepted work.
    Ok(())
}
```

`load_bytes(name, bytes)` accepts owned encoded data instead. Names and paths are
separate cache keys; paths become absolute when submitted. Keys identify immutable
assets: repeated loads share a cached sound even if supplied bytes or file contents
change. Use a new name or loader for changed content. The worker queue defaults to
16 waiting requests; a full queue returns `LimitExceeded` without waiting. Each
encoded input is limited to 64 MiB by default, and `LoaderConfig::decode` supplies
the usual decoded-audio limits. I/O and decode failures arrive through the handle.

`AudioCache` also works on its own. Its LRU policy skips sounds held by callers,
pending results or mixer voices, and weak references. The byte budget accounts for
retained PCM/channel capacities, entry structs and key/metadata strings; allocator
overhead, spare collection capacity, metadata tree nodes, temporary decoding memory
and externally retained sounds are excluded. Failed admission preserves existing
entries. If a sound is too large or all sufficient victims are in use, the loader
still returns it without caching. Zero cache bytes disables retention.

All asset operations, polling and destruction belong off the audio callback. The
cache supports WebAssembly; the thread-based loader is available on native targets.
Run `cargo run --example load_assets --features assets -- path/to/sound.wav` for a
complete file-loading example.

## When to use audiobox

audiobox fits when you want one dependency-free crate that both decodes **and encodes**,
edits and analyzes audio; when you need deterministic synthesized audio for a game; when
you target WebAssembly; or when you want to bring your own audio output. It does not open
audio devices: pass the mixer's `render` to the callback of whatever library does.

Other crates may suit you better:

- [symphonia](https://crates.io/crates/symphonia) decodes many more formats and codecs,
  such as AAC, ALAC and Vorbis, and is tuned for decoding speed. It does not encode.
- [hound](https://crates.io/crates/hound) reads and writes WAV, and nothing else.
- [rodio](https://crates.io/crates/rodio) plays audio on output devices through cpal.
- [kira](https://crates.io/crates/kira) is a complete game audio engine with device
  output, mixer tracks, tweens and effects such as reverb.

All transforms create a new `Audio`. Channel data is exposed as immutable slices;
construction validates channel lengths and finite samples. Errors are `Result<T, Error>`
with a stable `ErrorCode`. The original TypeScript package lives separately in
[jmsansan/audiobox](https://github.com/jmsansan/audiobox); the Rust API is independent
of it. See [CHANGELOG.md](CHANGELOG.md) for releases.

## Formats

| Format | Decode | Encode | Details |
| --- | --- | --- | --- |
| WAV / RF64 / BW64 | Yes | WAV | PCM 8/16/24/32, float 32/64, G.711 A-law/µ-law; IMA ADPCM decoding |
| AIFF / AIFF-C | Yes | Yes | Integer PCM, `sowt`, float 32/64, G.711 |
| CAF | Yes | Yes | PCM and G.711; 64-bit chunk sizes |
| AU / SND | Yes | Yes | Integer PCM, float and G.711 |
| FLAC | Yes | Yes | Fixed/LPC prediction and stereo decorrelation decoding; fixed prediction + Rice encoding, CRC and MD5 verification |
| MP3 | Yes | Yes | MPEG-1/2/2.5 Layer III decoding, CBR/VBR and gapless tags; MPEG-1 CBR encoding |
| Raw PCM | Explicit shape | Yes | `pcm::decode_pcm` and `pcm::encode_pcm` |

Formats are detected from bytes. To override detection, use `decode_with_options`.
An extension is only used to choose the **output** format in `write_audio_file`.
Unsupported input and malformed or truncated containers return an error.

```rust
use audiobox::{AudioFormat, EncodeOptions, SampleFormat, decode, encode_with_options};

fn to_flac(bytes: &[u8]) -> audiobox::Result<Vec<u8>> {
    let audio = decode(bytes)?;
    encode_with_options(&audio, AudioFormat::Flac, &EncodeOptions {
        sample_format: SampleFormat::S24,
        compression_level: 5,
        ..Default::default()
    })
}
```

FLAC compression level 0 searches the constant/verbatim/order-0 modes; levels 1–8
add fixed prediction orders 1–4. They currently share the same search and do **not**
represent eight different speed/quality presets. FLAC only accepts integer sample formats.

MP3 output requires mono or stereo at 32, 44.1 or 48 kHz, and a standard MPEG-1
bitrate from 32 to 320 kbit/s (`EncodeOptions::bitrate`, default 192). It uses long
blocks and global quantization with Huffman table selection. The TypeScript encoder's
VBR, psychoacoustic noise shaping, transient block switching and encoder bit reservoir
have **not** been ported. Prefer higher bitrates for complex material.

MP3 files contain an Info tag with the original sample count's priming and padding.
Audiobox honors it. Some external decoders, including FFmpeg, only apply gapless
trimming for a whitelist of encoder names; they may expose priming and trailing padding
for audiobox's own encoder name. The independent MP3 encoding tests explicitly align
that reference by the tag before checking samples and gain.

### Publishing tags and cover images

Use `EncodeOptions::tags` for structured publishing fields. Whole decoding reads them
back through `Audio::info().tags`, with the same field names across containers.

```rust
use audiobox::{Audio, AudioFormat, CoverArt, EncodeOptions, Tags, encode_with_options};

fn publish(audio: &Audio, png_bytes: Vec<u8>) -> audiobox::Result<Vec<u8>> {
    encode_with_options(audio, AudioFormat::Mp3, &EncodeOptions {
        tags: Tags {
            title: Some("Título".into()),
            artist: Some("José".into()),
            album: Some("Game soundtrack".into()),
            album_artist: Some("The team".into()),
            track: Some("3/12".into()),
            disc: Some("1/2".into()),
            year: Some("2026".into()),
            genre: Some("Chiptune".into()),
            comment: Some("Original soundtrack".into()),
            isrc: Some("ESABC2600001".into()),
            cover: Some(CoverArt::new("image/png", png_bytes)?),
        },
        ..Default::default()
    })
}
```

| Fields | MP3 | FLAC | WAV INFO |
| --- | --- | --- | --- |
| Title, artist, album, track, year, genre, comment | ID3v2.4 | Vorbis comments | Yes |
| Album artist, disc, ISRC | ID3v2.4 | Vorbis comments | Unsupported |
| JPEG/PNG front cover | APIC frame | PICTURE block | Unsupported |

WAV rejects unsupported structured fields instead of discarding them. Images are
supplied by the caller; MIME/signature validation does not decode or fully validate
the image. Original bytes are preserved. Only a front cover is exposed; unsupported
picture types/MIME types are ignored on reading. FLAC dimensions/depth are zero
(unknown), as allowed by [RFC 9639 §8.8](https://www.rfc-editor.org/rfc/rfc9639.html#section-8.8).

The existing `metadata` map remains available. WAV returns RIFF keys such as `INAM`,
`IART`, `IPRD`, `IPRT`, `ICRD`, `IGNR` and `ICMT`. FLAC returns lowercase native names
such as `albumartist`, `tracknumber`, `discnumber` and `date`; MP3 uses the structured
field names and stores custom map keys in UTF-8 TXXX frames. Supplying a field in
both `tags` and a known map alias is an error, as are duplicate native aliases on
writing. FLAC continues to accept custom Vorbis fields; WAV accepts four printable
ASCII character keys. AIFF's map supports `title`, `artist`, `comment`, `copyright`;
AU supports `comment`. CAF map writing and structured tags for these other formats
are unsupported.

Text is UTF-8 on writing and NUL is rejected. New readers check UTF-8; MP3 also reads
common ID3v2.3/2.4 Latin-1 and UTF-16 text, and frame unsynchronisation. Unsupported
ID3 versions, compressed/encrypted frames and whole-tag v2.3 unsynchronisation are
skipped for metadata while audio decoding remains available. Supported metadata
payloads are bounded to 16 MiB minus one byte and 4096 fields/frames, with container
sizes checked before copying. These bounds exclude allocator overhead and duplicate
native/structured strings. Tags are exposed by whole decoding; the incremental
decoder's sample reads/seek remain independent of tag access.

`cargo run --example tag_audio -- target/tag-demo tests/fixtures/cover.png` writes
synthetic WAV, FLAC and MP3 examples and verifies their tags. For optional independent
verification, install Mutagen in a separate Python environment and run
`python scripts/check-metadata.py target/tag-demo tests/fixtures/cover.png --ffmpeg ffmpeg`.
The script checks fields in Mutagen/FFmpeg, exact cover extraction and unchanged PCM.
These tools are test-only; the crate remains dependency-free and normal CI needs no
FFmpeg installation.

## Audio editing and DSP

- Shape: `sample_rate`, `channels`, `frames`, `duration`, `channel_data`, `all_channels`, `to_interleaved`.
- Construction: `Audio::new`, `Audio::silence`, `Audio::from_interleaved`, `Audio::merge`.
- Editing: `cut`, `remove`, `concat`, `pad`, `reverse`, `mix`, `crossfade_to`.
- Channels: `to_mono`, `to_stereo`, `to_channels`, `map_channels`, `pan`, `split`.
- DSP: `resample`, `gain`, `gain_db`, `fade`, `filter`, `limit`, `remove_dc_offset`.
- Silence: `detect_silence`, `trim_silence`.
- Time/pitch: `speed`, WSOLA `tempo`, `pitch`.
- Measurement: `peak`, `peak_db`, `true_peak_db`, `rms_db`, `loudness`, `waveform`.
- Spectral analysis: `analyze::spectrum`, `analyze::spectrogram`.

Transforms that render audio (pan, gain, fades, crossfades, filters, the limiter,
resampling, tempo, pitch, silence trimming and peak normalization) compute with the
crate's own math, so they give the same samples on every platform, WebAssembly
included; CI pins them with checksums. Analysis that only reports numbers, such as
loudness, uses the platform's math.

The resampler uses a Kaiser-windowed sinc filter with a downsampling anti-alias cutoff.
The limiter links channels with lookahead and release. Loudness uses K-weighting and
BS.1770 absolute/relative gating; `range` retains the original library's 400-ms block
percentile estimate and is **not** a certified EBU Tech 3342 loudness-range measurement.

```rust
use audiobox::{NormalizeOptions, NormalizeUnit, decode};

fn normalize_podcast(bytes: &[u8]) -> audiobox::Result<audiobox::Audio> {
    decode(bytes)?.normalize(NormalizeOptions {
        to: -16.0,
        unit: NormalizeUnit::Lufs,
        peak_ceiling_db: -1.0,
    })
}
```

Positions accept seconds (`f64`), `TimePosition::Frames(i64)`, or parsed timestamps
such as `"1:30".parse::<TimePosition>()?` and `"500ms".parse::<TimePosition>()?`.
Negative edit positions count from the end; negative durations are rejected.

## Safety limits

Default decoding limits are 512 MiB of decoded float samples, six hours, 64 channels,
sample rates 1–768 kHz, and at most `u32::MAX` frames. Declared shapes and incremental
decoding are checked before PCM allocations. Limits can be customized for trusted input:

```rust
use audiobox::{DecodeOptions, Limits, decode_with_options};

fn decode_upload(bytes: &[u8]) -> audiobox::Result<audiobox::Audio> {
    decode_with_options(bytes, &DecodeOptions {
        limits: Limits { max_duration_seconds: 600.0, ..Default::default() },
        ..Default::default()
    })
}
```

Limits bound decoded PCM, not the size of the input already held by the caller or all
intermediate DSP buffers. Enforce upload size limits in the calling application.

## Incremental decoding

`stream::Decoder` decodes WAV, FLAC or MP3 held in memory a chunk at a time, so a
game can play music without decoding all of it: three minutes of 44.1 kHz stereo is
about 63 MB as `f32`. It takes the bytes as `&'static [u8]` (from `include_bytes!`),
`Vec<u8>` or `Arc<[u8]>`.

```rust
use audiobox::stream::Decoder;

fn play(bytes: &'static [u8]) -> audiobox::Result<()> {
    let mut music = Decoder::new(bytes)?;
    let mut buffer = vec![0.0f32; 1024 * music.channels()];
    loop {
        let frames = music.read(&mut buffer)?; // Interleaved; 0 at the end.
        if frames == 0 {
            music.rewind()?; // Loop without decoding anything again.
            break;
        }
        // Send buffer[..frames * channels] to the audio device.
    }
    music.seek(44_100)?; // Or jump to any frame.
    Ok(())
}
```

- **Equal to `decode`:** the concatenated output equals what `decode` gives for the
  same bytes, sample for sample, for every chunk size.
- **Seeking:** `seek(frame)` moves to any frame. WAV seeks directly. FLAC uses the
  SEEKTABLE when there is one and otherwise decodes forward. MP3 restarts a few frames
  before the target to refill the bit reservoir and filter state, so the samples
  match a whole decode. `rewind()` returns to the start at once.
- **Bounded memory:** apart from the input bytes, the decoder holds one block of the
  input format (a FLAC block, an MP3 frame, an IMA ADPCM block or `chunk_frames` of
  PCM) and the MP3 bit reservoir: tens of kilobytes, allocated when it opens.
  `read` allocates nothing, so it can run on an audio thread. `DecoderOptions` takes
  the same `Limits` as `decode`, checked against the declared stream on opening.
- **Errors:** malformed or truncated input gives the crate's errors, never a panic.
  After an error, reads return it again until a seek or rewind.

AIFF, CAF and AU, and Ogg Vorbis and Opus, are not decoded incrementally.

### Streaming from disk (`file-stream` feature)

`stream::FileStream` keeps the file and codec on a dedicated reader thread. It
supports WAV (PCM, float, G.711 and IMA), FLAC and MP3 through a fixed encoded
window and a fixed PCM ring. Opening scans headers and prefills the ring on the
loading thread. It never retains the full encoded file or full decoded song.

```rust,no_run
use audiobox::stream::FileStream;
use audiobox::ErrorCode;

let (mut source, reader) = FileStream::open("music.flac")?;
let mut buffer = vec![0.0; 1024 * source.channels()]; // Prepare off the callback.
match source.read(&mut buffer) {
    Ok(0) => { /* End of file. */ }
    Ok(frames) => { /* Consume buffer[..frames * source.channels()]. */ }
    Err(e) if e.code == ErrorCode::Underrun => { /* Emit silence; retry later. */ }
    Err(e) => return Err(e),
}
source.seek(48_000)?; // Publishes a request; no file access or waiting here.
let status = reader.status(); // Buffered frames and underrun count, no allocation.
# Ok::<(), audiobox::Error>(())
```

Reads return any available complete frames before reporting a pending/error state;
they leave the unwritten destination untouched. Error messages after opening are
empty to keep **all read/seek paths allocation-free**; match on the stable code.
Seeks discard old queued audio by generation. WAV seeks directly; FLAC/MP3 replay
from the start on the worker for exact decoder state. Compressed seeks can take
time on long files. Metadata is skipped and files must stay unchanged while open.

With `mixer`, pass the source to `Sound::stream`. The mixer holds source position
on `Underrun`, emits silence and retries on the next render segment. Output-time
fades and stops continue. Normal voice whole/region loops queue their seeks on the
worker; restarts can underrun, so gapless disk loops are not guaranteed. Delivered
PCM matches memory decoding, while silent-gap timing depends on disk availability.

Retain the returned `FileStreamHandle` on a control thread until playback is over.
Dropping the stream requests cancellation; dropping the handle cancels and **joins**
the reader, so it belongs off the audio thread. The mixer's `poll_event` frees
returned streams on the control thread. Final stream/mixer destruction also belongs
there. The host supplies its own audio device.

`FileStreamOptions` separates output admission (`decoder.limits`) from resident
memory: a 1 MiB encoded window, a 16384-frame ring, a 32 MiB PCM ring/scratch budget,
a conservative 16 MiB codec budget and at most 4096 RIFF/FLAC header chunks by
default. Actual retained buffers use their prepared sizes, not the entire budgets.
The codec budget admits a fixed 128 KiB plus 20 bytes per block sample; the bounded
WAV range index needs at most twice `max_header_chunks` ranges (32 bytes per chunk
on 64-bit hosts). Add the encoded window, PCM/codec budgets and that index when
setting an application memory ceiling. Shared MP3 transform tables and thread
stacks are process/runtime costs outside these per-stream data budgets.

FLAC frames must fit the encoded window: an oversized declared maximum fails with
`LimitExceeded`; an oversized undeclared frame fails decoding. The default encoded
file admission limit is 8 GiB. The normal `Limits` still bound duration and total
decoded output, even though that output is not retained; raise them explicitly for
trusted long files while keeping resident budgets small. Native targets only;
enabling `file-stream` on WebAssembly does not expose the file API.

The example creates a FLAC when no input is supplied, streams it through the mixer
with looping, and exports a five-second WAV plus underrun diagnostics:

```sh
cargo run --example stream_from_disk --features file-stream,mixer
cargo run --example stream_from_disk --features file-stream,mixer -- music.mp3 target/disk.wav
```

## Streaming PCM

`stream::PcmDecoder` retains partial frames between input chunks. `stream::PcmEncoder`
validates that successive chunks have the same sample rate and channel count.
`stream::Resampler` retains the sinc filter history and preserves phase across chunks;
call `finish` to drain its tail. Its output matches whole-buffer resampling regardless
of input chunk boundaries. Stream limits apply to the complete stream, not each chunk.

Browser Workers, Web Audio and WebCodecs adapters belong to the TypeScript package and
are not part of the Rust API. Dithering, Sony Wave64, RF64 encoding and reading files
from disk incrementally are also not implemented.

## Synthesizer (`synth` feature)

`audiobox::synth` renders music written as text scores in an 8-bit style. A score is a
few kilobytes of text that the project owns, and it renders to the same samples on
every platform.

```rust
use audiobox::synth::Score;

fn render_song() -> audiobox::Result<audiobox::Audio> {
    let score: Score = "
        tempo 150
        instrument lead wave=pulse duty=25 adsr=5,60,70,40 vibrato=12,160,200
        instrument bass wave=triangle
        instrument hat  wave=noise noise=short adsr=0,30,0,0 volume=6
        channel melody = @lead o5 l8 L [c e g >c< g e]2 c2
        channel bass   = @bass o3 l2 c g c1
        channel drums  = @hat o7 l8 [c r c c]4
    ".parse()?;
    score.render(44_100)
}
```

- **Channels:** up to 8, each playing one note at a time.
- **Waveforms:** pulse with 12.5, 25, 50 and 75% duty, triangle, sawtooth, and noise from a
  15-bit shift register in a long (hiss) or short (metallic, 93-step) mode.
- **Instruments:** waveform, linear ADSR envelope, volume, pitch slide, vibrato,
  arpeggio and duty sweep.
- **Rendering:** offline with `Score::render`, or incrementally with `synth::Synth`,
  whose `fill(&mut [f32])` allocates nothing and can run on an audio thread. Chunks
  are identical to the offline render. Songs loop from their loop point, a set number
  of times or forever.
- **Aliasing:** `Aliasing::BandLimited` (default) applies PolyBLEP to pulse and saw,
  PolyBLAMP to triangle corners, and averages noise over each sample;
  `Aliasing::Naive` keeps the raw, harsher period sound.
- **Determinism:** no `sin`, `exp` or `powf` decides a sample. Pitch and vibrato use
  the crate's own series, and note positions use integer arithmetic.
- **Rust API:** `Score::channel` returns a writer that builds the same score in code;
  a parsed score compares equal to the same score built in code.

### Score format

One directive per line; `;` starts a comment.

| Directive | Meaning |
| --- | --- |
| `tempo 150` | Quarter notes per minute, 1–999 (default 120) |
| `ticks 48` | Time base in ticks per quarter note, 1–960 (default 48) |
| `instrument NAME key=value ...` | Defines an instrument |
| `pattern NAME = ...` | Defines a reusable pattern of commands |
| `channel NAME = ...` | Appends commands to a channel; repeat the line to continue it |

Instrument keys: `wave=pulse|triangle|saw|noise`, `duty=12.5|25|50|75` (a list such as
`25,50,75` sweeps, every `duty-step` ms), `noise=long|short`,
`adsr=attack_ms,decay_ms,sustain_percent,release_ms`, `volume=0..15`,
`slide=cents_per_second`, `vibrato=depth_cents,period_ms[,delay_ms]`,
`arpeggio=0,4,7` (cycled every `arpeggio-step` ms, default 16).

| Command | Meaning |
| --- | --- |
| `c d e f g a b` | Note; `+` or `#` sharp, `-` flat, then an optional length |
| `r` | Rest |
| `4` `8.` `%12` | Lengths: quarter, dotted eighth, 12 ticks; without one, the `l` default |
| `^8`, `c4&c8` | Ties: lengthen the previous note, or join two notes of the same pitch |
| `o4` `>` `<` | Octave (0–9; `o4 a` is 440 Hz), one up, one down |
| `l8` | Default length |
| `v12` | Channel volume, 0–15 |
| `q6` | Gate: notes sound 6/8 of their length, then release |
| `@NAME` | Select instrument |
| `[ ... \| ... ]3` | Repeat 3 times; the part after `\|` is skipped on the last time |
| `$NAME` | Play a pattern |
| `L` | Loop point of the song |

Parse errors are `synth::ParseError` values with a line, a column and a stable
`ErrorCode` (`Syntax`, `InvalidArgument` or `LimitExceeded`). `ScoreLimits` bounds
the text length, the nesting of repeats and patterns, the number of channels, notes
and expanded commands, and the song's duration. The full reference, including how
loops, envelopes and noise pitch work, is in the `audiobox::synth` module documentation.

Render the example song to a WAV file:

```sh
cargo run --example chiptune --features synth -- song.wav
```

### Tracker instruments

Opt in with `Instrument::chip: Some(ChipInstrument { .. })` or these score keys.
Existing scores keep their sample bits. Each new note restarts source phase and
macro age; ties continue them, and notes crossing `L` restart on the next lap.

```text
instrument table wavetable=0123456789abcdefedcba98765432100 macro-arpeggio=0,7,12 macro-arpeggio-step=ticks,6 macro-arpeggio-loop=0
instrument lead wave=pulse quantize=ntsc-pulse macro-duty=0,1,2,3 macro-duty-loop=0 macro-pitch=0,-8,8 macro-pitch-loop=0
instrument hat wave=noise noise=short noise-period=6 macro-volume=15,8,2,0
instrument pcm sample=00040a0f080300 sample-bits=4 sample-rate=4000 sample-root=60 sample-loop=on
```

| Control | Format and bounds |
| --- | --- |
| Wavetable | `wavetable=HEX`, 2..256 unsigned four-bit nibbles, one per entry |
| Sample | `sample=HEX`, 1..65536 unsigned values, **two hex digits per value**; `sample-bits=2..8` (default 4), `sample-rate=100..32000` (default 8000 Hz), `sample-root=0..127` (default MIDI 60), `sample-loop=off` (default) or `on` |
| Noise | `wave=noise noise=long\|short noise-period=0..15` selects the NTSC period table, independent of note pitch |
| Macros | `macro-volume` 0..15, `macro-duty` 0..3 (12.5/25/50/75%), `macro-arpeggio` ±48 semitones, `macro-pitch` ±1200 cents; comma-separated, up to 64 values each |
| Macro clock | Each `macro-LANE-step=frames,COUNT,HZ` (both 1..1000, default `frames,1,60`) or `ticks,COUNT` (1..960) |
| Macro ending | `macro-LANE-loop=INDEX` (zero-based) repeats from that index after the initial prefix; default `hold` keeps the last value |
| Quantization | `quantize=volume` rounds combined ADSR/volume/velocity/macro gain to 16 levels; `ntsc-pulse` or `ntsc-triangle` also selects the matching period pitch model; `none` disables both |

Macro steps use a rational clock relative to the note, independent of callback size;
the first sample at or after each boundary takes the new value. Tick steps can differ
by one sample from globally rounded score note positions. All clocks keep advancing
at zero gain. Custom sources are mutually exclusive; tables/PCM reject wave/duty/noise
settings, fixed noise rejects pitch modulation, and duplicate legacy/macro lanes are
errors. Gate and ADSR apply to all sources; non-looping PCM also stops at its data end.

Wavetables and PCM use sample-and-hold without anti-aliasing. PCM speed follows the
ratio to its root note. NTSC pitch models use 11-bit timers and clamp out-of-range
requests; they do not emulate hardware mute rules or cycle-accurate consoles. See the
[full reference](https://docs.rs/audiobox/latest/audiobox/synth/#tracker-instruments-opt-in)
for exact clocks, ranges and Rust types.

Export the four voices separately and together:

```sh
cargo run --example tracker_instruments --features synth -- target/tracker-instruments
```

### Synthesized drum kits

The `synth` feature also provides kick, snare, closed/open hi-hat, tom, clap and crash
instruments in `chiptune` and `electronic` kits. All sounds come from oscillators and
noise; no recorded samples are included. Select a drum just like another instrument:

```text
tempo 120
instrument kick drum=kick kit=electronic
instrument snare drum=snare kit=electronic drum-tone=100 drum-noise=220
instrument closed drum=closed-hat kit=chiptune
instrument open drum=open-hat kit=chiptune
channel kick = @kick l8 [c!127 r c!80 r]2
channel snare = @snare [r4 c4!100]2
channel hats = l8 [@closed c!90 c!50 @open c!80 r]2
```

`!0`..`!127` is a per-note velocity suffix, separate from channel `v0`..`v15`.
Place it after the length and any dots (`c16.!96`). It defaults to 127 on each note,
so a soft note does not change the next note's velocity. Ties keep the first attack's
velocity and reject a new suffix on the continuation. The builder has
`ChannelWriter::note_with_velocity`; `note` still uses 127. Existing scores retain
their previous sample bits.

Construct settings with `Drum::new(DrumKind::Kick, DrumKit::Electronic)` and
`Instrument::drum(settings)`, or render one hit offline with `settings.render(rate,
velocity)`. Both kits have independent finite tone/noise envelopes, pitch sweeps and
fixed seeds. Chiptune uses triangle tones and clocked long/short 15-bit noise;
electronic uses sine bodies, filtered noise and metallic oscillator banks. These
presets have synthetic characters rather than modelling a specific drum machine.

| Score key | Meaning |
| --- | --- |
| `drum-pitch=55` | Base frequency at C4, 20..=20000 Hz. Other notes transpose pitched components. |
| `drum-tone=400` / `drum-noise=220` | Independent finite envelope lengths, 0..=10000 ms; zero mutes that component. |
| `drum-sweep=2400,35` | Initial cents above the base (0..=4800) and time to reach it (0..=1000 ms). |
| `drum-mix=70` | Noise percentage (0..=100), with the complementary tone level. Clap uses noise only. |
| `drum-click=25` | Separate 3 ms filtered-noise transient level, 0..=100 percent. |
| `drum-seed=123` | Fixed noise seed, 0..=4294967295. Zero maps to one; chip mode uses the low 15 bits. |

Parameters omitted from text use the selected kit/kind preset, independently of key
order. `volume` also works. Waveform, ADSR and melodic modulation keys are rejected
on drum instruments. Their squared decay envelopes include a 1 ms attack; a clap
adds short bursts at 0, 10 and 20 ms before its final tail at 30 ms. Very short
envelopes can be silent at low sample rates. Metallic components can alias.

Each channel remains monophonic: a new note replaces its previous hit and tail.
Putting open/closed hats on one channel gives a natural choke; use separate channels
for overlapping kick, snare and cymbals. Gate-off does not cut a drum's tail, and the
last tail is included in the finite render length. A loop carry alone does not
retrigger percussion. Every trigger resets phase, filters and noise seed; rendering
the prepared `Synth` stays allocation-free and independent of buffer sizes.

Render a pattern in each kit (optionally choose an output directory):

```sh
cargo run --example drum_kits --features synth -- target/drum-kits
```

## Sound effects (`sfx` feature)

`audiobox::sfx` renders game sound effects, such as jumps, coins, hits and menu blips,
from a few parameters that can be kept as text in a game's repository. It uses the
synthesizer's waveforms and gives the same samples on every platform.

```rust
use audiobox::sfx::{Effect, Preset};

fn effects() -> audiobox::Result<(audiobox::Audio, audiobox::Audio)> {
    let coin: Effect = "wave=pulse duty=50 frequency=987.8 sustain=0.05 punch=0.5
                        decay=0.3 arpeggio=5 arpeggio-time=0.07".parse()?;
    let coin_audio = coin.render(44_100)?;

    // Presets draw parameters from a seed: try seeds until one sounds right,
    // then keep the seed or the text that `to_string` writes.
    let jump = Effect::preset(Preset::Jump, 7);
    println!("{jump}");
    let jump_audio = jump.render(44_100)?;
    Ok((coin_audio, jump_audio))
}
```

The text form is whitespace-separated `key=value` pairs, on one line or several, with
`;` comments. Missing keys keep their defaults. `Display` writes the same form, and
parsing it gives back an equal `Effect`.

| Key | Unit | Range | Default |
| --- | --- | --- | --- |
| `wave` | `pulse`, `triangle`, `saw`, `noise` | | `pulse` |
| `duty` | pulse duty, percent | `12.5`, `25`, `50`, `75` | `50` |
| `noise` | noise mode | `long`, `short` | `long` |
| `frequency` | Hz | 10–20000 | 440 |
| `slide` | octaves per second | ±64 | 0 |
| `slide-change` | octaves per second² | ±256 | 0 |
| `vibrato-depth`, `vibrato-speed` | semitones, Hz | 0–24, 0–100 | 0 |
| `attack`, `sustain`, `decay` | seconds | 0–10 each | 0, 0.1, 0.2 |
| `punch` | extra level at the start of the sustain | 0–1 | 0 |
| `duty-sweep` | duty change per second (pulse only) | ±10 | 0 |
| `low-pass`, `high-pass` | cutoff in Hz, 0 for off | 0 or 10–20000 | 0 |
| `low-pass-sweep`, `high-pass-sweep` | octaves per second | ±16 | 0 |
| `arpeggio`, `arpeggio-time` | semitones, seconds | ±48, 0–10 | 0 |
| `repeat` | seconds; restarts pitch, slides, arpeggio and duty; 0 for off | 0 or 0.01–10 | 0 |
| `volume` | linear | 0–1 | 0.5 |

The effect lasts `attack + sustain + decay`, which must be more than 0 and at most
10 seconds. The filters are 12 dB/octave Butterworth state-variable filters that
attenuate their cutoff by 3 dB. Out-of-range values give `ErrorCode::InvalidArgument`;
in text, the `ParseError` also gives the line and column of the value.

`Preset` covers `Pickup`, `Laser`, `Explosion`, `PowerUp`, `Hit`, `Jump` and `Blip`.
The same preset and seed always give the same parameters and samples. The full
reference is in the `audiobox::sfx` module documentation.

Write a few effects to WAV files:

```sh
cargo run --example sfx --features sfx -- out_dir
```

## Mixer (`mixer` feature)

`audiobox::mixer` mixes a game's sounds in its audio callback. The crate does not open
the audio device: pass `Mixer::render` to whatever library does (the callback of
`cpal`, Web Audio, SDL and so on).

```rust
use audiobox::mixer::{Fade, Loop, Mixer, MixerConfig, Play};
use audiobox::stream::Decoder;
use std::sync::Arc;

fn start(music: &'static [u8], jump: Arc<audiobox::Audio>) -> audiobox::Result<()> {
    let (mut mixer, mut handle) = Mixer::new(MixerConfig {
        buses: vec!["music".into(), "effects".into()],
        ..MixerConfig::default() // 48 kHz stereo, 32 voices
    })?;
    let music_bus = handle.bus("music").unwrap();
    let effects = handle.bus("effects").unwrap();

    // Game thread: compressed music streams; effects share decoded audio.
    handle.play(Decoder::new(music)?, &Play {
        bus: music_bus,
        looping: Loop::Whole,
        priority: 255,
        fade_in: Fade::linear(2.0),
        ..Play::default()
    })?;
    let voice = handle.play(jump, &Play { bus: effects, pan: 0.3, ..Play::default() })?;
    handle.set_bus_gain(music_bus, 0.5, Fade::linear(0.3))?;
    handle.stop(voice, Fade::linear(0.05))?;

    // Audio thread: once per device buffer (interleaved stereo).
    let mut buffer = vec![0.0f32; 2 * 512];
    mixer.render(&mut buffer);

    // Game thread, once per frame: events, and freeing finished voices.
    while let Some(_event) = handle.poll_event() {}
    Ok(())
}
```

- **Sounds:** decoded `Audio` shared through an `Arc`, so several voices play one sound
  without copying it, and streams: the incremental `Decoder`, a synthesizer `Synth`, or
  any `mixer::Stream`. Effects from `sfx` play as rendered `Audio`.
- **Voices:** gain, constant-power pan, looping (the whole sound or a region), and
  fades in, out and between gains with a `FadeCurve`. At most `max_voices` play; a new
  voice takes the place of the lowest-priority, oldest voice if that priority is not
  higher than its own, which fades out over 5 ms; otherwise the new voice is rejected.
- **Rate, pitch and variation:** a playback rate and a pitch offset per voice, set at
  the start and changed while it plays with a ramp, and seeded random variation of
  pitch and gain for repeated sounds:

  ```rust
  use audiobox::mixer::{Mixer, MixerConfig, Play, Variation};
  use std::sync::Arc;

  fn footsteps(step: Arc<audiobox::Audio>) -> audiobox::Result<()> {
      let (_mixer, mut handle) = Mixer::new(MixerConfig { seed: 42, ..MixerConfig::default() })?;
      // Each step is up to a semitone higher or lower and 2 dB louder or softer.
      let footstep = Play { variation: Variation { pitch: 1.0, gain_db: 2.0 }, ..Play::default() };
      for i in 0..4 {
          handle.at(i * 18_000).play(step.clone(), &footstep)?;
      }
      Ok(())
  }
  ```
- **Instance limits:** at most so many voices of a sound at once (refuse, stop the
  oldest or stop the quietest), and a minimum gap between starts, so ten coins at
  once do not distort: `Play { instances: Some(Instances::of(&coin).max(3).min_gap(0.05)), .. }`.
- **2D positions:** a position relative to the listener (the camera's centre) pans a
  voice and fades it with distance, linearly or inversely between a minimum and a
  maximum distance; moving sounds update with a ramp (`set_position`).
- **Pause and position:** pause and resume a voice or a whole bus, with optional fades,
  so a pause menu stops the game's sounds and keeps its own; start a voice partway
  through; read a voice's state and position without blocking the audio thread.
- **Buses:** a master bus and named buses, each with a gain that can fade.
- **Bus effects:** ordered low/high-pass filters, compression and sidechain ducking,
  delay and reverb, with sample-scheduled parameter ramps; see below.
- **Master limiter:** an optional look-ahead peak limiter on the master bus
  (`MixerConfig::limiter`) keeps the sum of many voices at or below a ceiling, passes
  quieter signals unchanged, and reports its gain reduction for meters.
- **Rendering:** `render(&mut [f32])` fills interleaved mono or stereo at the output
  rate, resampling sources at other rates with a 16-tap windowed-sinc filter. It writes
  whole frames and zeroes any remainder; 0 frames only applies due changes.
- **Real time:** `render` allocates nothing, takes no lock and does not block. Changes
  travel through a bounded single-producer, single-consumer queue of the crate's own,
  drained at the start of each render, and finished
  voices travel back to be freed on the game's thread.
- **Determinism:** `handle.at(frame)` schedules changes at exact frames, independent of
  buffer sizes, and the same changes at the same frames give the same samples on every
  platform.

Render a short scene with music and effects to a WAV file:

```sh
cargo run --example game_audio --features mixer,sfx -- game.wav
```

### 3D emitters and surround output

`Play::emitter` opts into a point source with world position, velocity and its own
attenuation. `MixerConfig::listener` supplies position, forward/up orientation and
velocity; the default looks down -Z with +X right and +Y up. Positions use one
consistent unit (normally metres), velocities use units/second. The host supplies
new positions explicitly; velocity controls Doppler and does not integrate motion.

```rust
use audiobox::mixer::*;
use audiobox::Audio;
let (mut mixer, mut handle) = Mixer::new(MixerConfig {
    channels: OutputLayout::Surround71.channels(),
    ..Default::default()
})?;
let sound = Audio::new(vec![vec![0.1; 4800]], 48_000)?;
let voice = handle.play(sound, &Play {
    emitter: Some(Emitter {
        position: [3.0, 1.0, -5.0],
        velocity: [-10.0, 0.0, 0.0],
        attenuation: Attenuation {
            min_distance: 1.0, max_distance: 100.0,
            model: DistanceModel::Inverse, rolloff: 1.0,
        },
    }),
    ..Default::default()
})?;
handle.at(240).set_emitter(voice, Emitter {
    position: [2.0, 1.0, -5.0], ..Default::default()
}, Fade::linear(0.016))?;
handle.at(480).set_listener(Listener {
    forward: [1.0, 0.0, 0.0], ..Default::default()
}, Fade::linear(0.016))?;
let mut output = [0.0; 8 * 256];
mixer.render(&mut output);
# Ok::<(), audiobox::Error>(())
```

All vectors must be finite. Listener axes are normalized/orthogonalized; zero or
near-parallel forward/up axes are rejected. Reference distance is 0.001..1e9,
maximum distance is greater than reference and at most 1e12, rolloff is 0..16.
For `d = clamp(distance, min, max)` and `r = rolloff`:

| Model | Gain |
| --- | --- |
| Linear | `clamp(1 - r*(d-min)/(max-min), 0, 1)` |
| Inverse | `min / (min + r*(d-min))` |
| Exponential | `(d/min)^(-r)` |

These use the [OpenAL clamped models](https://www.openal.org/documentation/openal-1.1-specification.pdf).
Maximum distance clamps evaluation; inverse/exponential keep a nonzero floor.
Rolloff zero disables attenuation. Existing `Space2d` keeps its previous behavior.
Doppler uses both radial velocities with `SpatialConfig::speed_of_sound` (1..1e6,
default 343.3) and `doppler_factor` (0..16, default 1; zero disables it). Scaled
radial velocities clamp to ±0.9 times sound speed and the playback ratio to
0.25..4. The final rate × pitch × Doppler speed retains the 1/1024..512 bound.

Placement is recomputed at command application, using the listener then current.
Direction ramps keep unit power; distance and Doppler ramp separately. Their
clocks freeze with voice/bus pauses, and listener updates include fading tails.
Stereo sources collapse immediately to `(L+R)/2` in point mode, so antiphase
material can cancel. `set_emitter` enables that mode permanently; its fade ramps
spatial parameters, not stereo-to-mono conversion. Pan/2D-position commands then
have no audible effect. Choose either `Play::position` or `Play::emitter`.

| Output channels | Order | Horizontal speaker angles from forward | WAV mask |
| --- | --- | --- | --- |
| 1 | Mono | none | `0x4` |
| 2 | FL, FR | constant-power stereo pan | `0x3` |
| 4 | FL, FR, BL, BR | fronts ±45°, backs ±135° | `0x33` |
| 6 | FL, FR, C, LFE, BL, BR | fronts ±30°, centre 0°, surrounds ±110° | `0x3f` |
| 8 | FL, FR, C, LFE, BL, BR, SL, SR | fronts ±30°, centre 0°, backs ±150°, sides ±90° | `0x63f` |

Surround uses normalized positive gains between adjacent speakers. Elevation
affects distance/Doppler; a directly vertical/coincident source uses the front
direction. LFE stays silent without explicit bass routing; there is no HRTF,
occlusion or propagation delay. Legacy voices use the front pair on surround.
Bus effects process every channel; compressor/limiter detection links all channels.
Modulation channel phase offsets repeat by channel index.

`OutputLayout::downmix_stereo(input, output)` folds caller-owned interleaved buffers
without allocation. Each full-range matrix column has unit power: FL/BL/SL go
left, FR/BR/SR right, centre/mono at 1/√2 to each side; LFE is omitted. This
preserves isolated-channel and uncorrelated-input energy. Correlated channels can
sum above unity; apply headroom or a limiter **after** the fold for a stereo ceiling.

For PCM/float WAV export, set `EncodeOptions::wav_channel_mask` to
`Some(layout.channel_mask())`. It writes WAVEFORMATEXTENSIBLE with the
[standard speaker order](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ksmedia/ns-ksmedia-waveformatextensible).
Set-bit count must match channels, using the standard low 18 bits; other output
formats reject the option. The default `None` retains existing WAV bytes.

Export an orbit and a Doppler flyby in all three surround layouts, plus stereo folds:

```sh
cargo run --example spatial_audio --features mixer -- target/spatial-audio
```

### Bus effects and sidechains

`mixer::effects` prepares up to 16 effects per bus on the control thread. Install a
chain with `set_effects`, update a matching slot with `set_effect`, or schedule either
operation with `handle.at(frame)`. Numeric updates ramp per sample and preserve DSP
history. The same chain API is the extension point for additional effect variants.

```rust
use audiobox::mixer::{Fade, Mixer, MixerConfig};
use audiobox::mixer::effects::{Compressor, Delay, Effect, EffectTime, Reverb};

fn main() -> audiobox::Result<()> {
    let (_mixer, mut handle) = Mixer::new(MixerConfig {
        buses: vec!["music".into(), "dialogue".into()],
        ..Default::default()
    })?;
    let music = handle.bus("music").unwrap();
    let dialogue = handle.bus("dialogue").unwrap();
    handle.set_effects(music, &[
        Effect::LowPass { cutoff_hz: 8000.0 },
        Effect::Compressor(Compressor::ducking(dialogue)),
        Effect::Delay(Delay {
            time: EffectTime::Beats { beats: 0.5, bpm: 120.0 },
            ..Default::default()
        }),
        Effect::Reverb(Reverb::default()),
    ])?;
    handle.set_effect(music, 0, Effect::LowPass { cutoff_hz: 600.0 }, Fade::linear(0.2))?;
    Ok(())
}
```

Effects run before the bus gain. A compressor's optional sidechain hears another
named bus **after** its effects, gain and pause fade, in the same frame. Named buses
are ordered by these dependencies, then summed with direct master voices; master
effects run on the sum before master gain and the existing limiter. A master key,
self key or dependency cycle is refused. Application-time cycles, missing slots,
type changes and delay-capacity increases report `MixerEvent::EffectsRejected` and
preserve the previous settings. Successful chain edits report `EffectsApplied`.
Poll events regularly to free retired chains; voice and effect event queues are
independent, so their combined polling order need not follow frame order.

The filters are first-order (6 dB/octave, -3 dB at cutoff). The hard-knee compressor
links stereo peak detection; attack/release are exponential time constants, and
`depth` controls the amount of gain reduction. Delay has a prepared `max_seconds`
capacity, fractional read positions, signed feedback and a linear dry/wet mix.
Delay-time ramps produce pitch changes. Beat times take explicit BPM: send updated
settings to follow tempo changes. Reverb uses four feedback combs and two allpass
diffusers per channel; `decay_seconds` specifies nominal T60 with zero damping.
Damping shortens high-frequency tails. Parameter ranges are in the API docs.

Paused buses freeze their effect buffers and automation clock. Tails continue after
voices end. Replacing/removing a chain is immediate and resets/removes its state;
fade the bus around structural edits if a discontinuity would be audible. With no
chains installed, the existing mixer path is unchanged. An installed chain can
change summation rounding, including at dry/neutral settings. No latency is added
to the dry path; delay and reverb tails belong to their wet paths.

Rendering allocates no effect memory and frees no chains, including when the handle
is disconnected. At most `MixerConfig::queue` owning chain edits may await polling;
further edits return an error until events are reclaimed. Delay/reverb buffers are
allocated only at installation preparation. Render an example with filter ramps,
ducking, a tempo-synced delay and reverb:

```sh
cargo run --example bus_effects --features mixer -- bus-effects.wav
```

### Modulation and colour effects

The same `mixer::effects` chains also accept `Chorus`, `Flanger`, `Phaser`,
`Bitcrusher` and `Saturation`. Use a dedicated bus for one voice (`Play::bus`) or
one whole music track (`Music::new(..., track_bus)`). All stems and transitions of
that music controller share its bus chain; individual `Layer` routing is not exposed.

```rust
use audiobox::mixer::{Fade, Mixer, MixerConfig};
use audiobox::mixer::effects::{Chorus, Effect, ModulationRate, Saturation};

fn main() -> audiobox::Result<()> {
    let (_mixer, mut handle) = Mixer::new(MixerConfig {
        buses: vec!["pad".into()], ..Default::default()
    })?;
    let pad = handle.bus("pad").unwrap();
    let chorus = Chorus {
        rate: ModulationRate::Beats { beats: 2.0, bpm: 120.0 },
        ..Default::default()
    };
    handle.set_effects(pad, &[
        Effect::Chorus(chorus),
        Effect::Saturation(Saturation { drive_db: 3.0, ..Default::default() }),
    ])?;
    handle.at(48_000).set_effect(pad, 0,
        Effect::Chorus(Chorus { mix: 0.6, ..chorus }), Fade::linear(0.2))?;
    Ok(())
}
```

- **Chorus:** two modulated delay taps per channel, a quarter cycle apart, with
  optional feedback and a right-channel phase offset. Delay capacity is 80 ms.
- **Flanger:** one modulated short delay per channel, mixed with dry audio for a
  moving comb response. Delay capacity is 20 ms; feedback can be positive or negative.
- **Phaser:** four first-order allpasses per channel. With fixed settings and zero
  feedback the full-wet magnitude is one; a dry/wet blend produces two notches.
  The sweep is logarithmic, with a centre frequency and a depth in octaves.
- **Bitcrusher:** nearest signed PCM levels (2..=24 bits) and a sample-and-hold
  reducer. Integer depth gives exactly `2^bits` levels from -1 to `1 - 2^(1-bits)`.
  Fractional depth blends adjacent integer quantizers. `rate_ratio = 0.25` captures
  the first frame and every fourth frame thereafter, with one phase for both channels.
- **Saturation:** symmetric cubic soft clipping, `v - v³/3` within +/-1 and +/-2/3
  beyond it, with input drive, wet output gain and dry/wet mix. Its slope joins
  continuously at the boundaries. It adds colour without modelling physical tape.

`ModulationRate` supports Hz (zero freezes the phase) or a cycle period in quarter-note
beats with explicit BPM. Tempo changes use `set_effect`; speed and other parameters
ramp per sample while retaining LFO phase, delay history, allpass state and reducer
phase. Installation starts the LFO at phase zero; beat mode synchronizes period,
without locking phase to a song transport. Pausing a bus freezes that state too.
The default flanger needs at least 2000 Hz output and the default phaser 1778 Hz;
adjust delay/depth or centre frequency when using the mixer's lower supported rates.

Delay +/- depth must fit the effect's capacity and stay at least one sample at
validation. Evaluated delays and phaser frequencies are clamped during automation
as well, because nonlinear fade curves can cross bounds between valid endpoints.
Modulated read heads intentionally bend pitch. Quantization/hold steps and nonlinear
harmonics are intentional; these effects do not dither, oversample or anti-alias.

Render six labelled sections (dry, chorus, phaser, flanger, bitcrusher, saturation)
of the same synthetic pad for comparison:

```sh
cargo run --example colour_effects --features mixer -- colour-effects.wav
```

## Development and releases

CI runs these checks on every pull request and on `main`, on Linux, macOS and Windows.
To run them locally:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features synth -- -D warnings
cargo clippy --all-targets --features mixer -- -D warnings
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo test --release --all-features
cargo +1.85.0 test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
AUDIOBOX_FFMPEG_TESTS=1 cargo test --test interop   # needs FFmpeg
python3 scripts/build-site.py && node --test site/tests/engine.test.mjs
cargo publish --dry-run
```

The interoperability tests use an installed FFmpeg **only in tests**. Their CI job
runs on published releases and manual dispatches of the **Rust** workflow, so
routine push/PR checks do not wait for FFmpeg installation. Normal CI checks the
MSRV and codec regressions; the website is built on pull requests and deployed
from `main`. The `.crate` contains the Rust implementation,
tests, examples, this README, the changelog and the MIT license; it excludes the
website and build output.

To release: update `CHANGELOG.md` and the version in `Cargo.toml`, run the checks,
merge to `main`, run `cargo publish`, then tag `vX.Y.Z` and publish a GitHub release.

## License

MIT, same as the original TypeScript implementation. Specification tables and transform
windows were ported from [the original audiobox repository](https://github.com/jmsansan/audiobox); no external codec source is vendored.
