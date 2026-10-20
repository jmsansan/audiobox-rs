# audiobox for Rust

Audio decoding, editing, analysis and encoding in **safe Rust with zero dependencies**.
The crate uses the Rust standard library: no C/C++ libraries, FFmpeg subprocesses,
WebAssembly blobs, build scripts, or runtime downloads.

The Rust implementation lives in `src/` and is built by the root `Cargo.toml`.
The original TypeScript package is maintained separately in
[jmsansan/audiobox](https://github.com/jmsansan/audiobox).
The current Rust release is version **0.2.0**, with an API independent of the npm version.
See [CHANGELOG.md](CHANGELOG.md) for what changed.

## Install

Website, live sandbox and API guide: [jmsansan.github.io/audiobox-rs](https://jmsansan.github.io/audiobox-rs/).

## Use from this checkout

```toml
[dependencies]
audiobox = { path = "/path/to/audiobox-rs" }
```

Install the published crate from crates.io:

```toml
[dependencies]
audiobox = "0.2"
```

Rust 1.85 or newer is required. The crate has no dependencies. Optional features add
modules without adding dependencies, so you compile only what you use:

| Feature | Module | Adds |
| --- | --- | --- |
| `synth` | `audiobox::synth` | 8-bit style synthesizer and text score format |
| `sfx` | `audiobox::sfx` | Parametric sound-effect generator with seeded presets (enables `synth`) |
| `mixer` | `audiobox::mixer` | Real-time mixer for games: voices, buses, fades, loops and resampling |

```toml
[dependencies]
audiobox = { version = "0.2", features = ["mixer", "synth", "sfx"] }
```

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

All transforms create a new `Audio`. Channel data is exposed as immutable slices;
construction validates channel lengths and finite samples. Errors are `Result<T, Error>`
with a stable `ErrorCode`.

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

WAV metadata supports `title`, `artist`, `album`, `comment`, or four-character RIFF INFO
keys. AIFF supports `title`, `artist`, `comment`, `copyright`; FLAC supports Vorbis
comments; AU supports `comment`. CAF and MP3 metadata writing is currently unsupported.
The returned WAV metadata uses its RIFF keys, such as `INAM` for a title.

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
- **Buses:** a master bus and named buses, each with a gain that can fade.
- **Rendering:** `render(&mut [f32])` fills interleaved mono or stereo at the output
  rate, resampling sources at other rates with a 16-tap windowed-sinc filter. It writes
  whole frames and zeroes any remainder; 0 frames only applies due changes.
- **Real time:** `render` allocates nothing, takes no lock and does not block. Changes
  travel through a bounded queue drained at the start of each render, and finished
  voices travel back to be freed on the game's thread.
- **Determinism:** `handle.at(frame)` schedules changes at exact frames, independent of
  buffer sizes, and the same changes at the same frames give the same samples on every
  platform.

Render a short scene with music and effects to a WAV file:

```sh
cargo run --example game_audio --features mixer,sfx -- game.wav
```

## Development and publishing

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo test --release --all-features
cargo doc --no-deps --all-features
cargo package --list
cargo publish --dry-run
```

Independent interoperability tests use an installed FFmpeg **only in tests**:

```sh
AUDIOBOX_FFMPEG_TESTS=1 cargo test --test interop
```

CI runs Rust formatting, linting, tests on Linux/macOS/Windows, MSRV compilation,
FFmpeg interoperability, and package verification. The `.crate` contains the Rust
implementation, tests, Rust example, this README and MIT license; it excludes the
TypeScript sources, node_modules, website and build output.

Try the file-conversion example:

```sh
cargo run --example convert -- input.mp3 output.flac
```

Publishing has not been performed by preparing this checkout. Before the first release,
verify that the `audiobox` name is available to your crates.io account. Follow the
[Cargo publishing guide](https://doc.rust-lang.org/cargo/reference/publishing.html),
authenticate with `cargo login`, commit the release, then run `cargo publish`.

## License

MIT, same as the original TypeScript implementation. Specification tables and transform
windows were ported from [the original audiobox repository](https://github.com/jmsansan/audiobox); no external codec source is vendored.
