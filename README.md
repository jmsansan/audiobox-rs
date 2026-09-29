# audiobox for Rust

Audio decoding, editing, analysis and encoding in **safe Rust with zero dependencies**.
The crate uses the Rust standard library: no C/C++ libraries, FFmpeg subprocesses,
WebAssembly blobs, build scripts, or runtime downloads.

The Rust implementation lives in `src/` and is built by the root `Cargo.toml`.
The original TypeScript package is maintained separately in
[jmsansan/audiobox](https://github.com/jmsansan/audiobox).
This is the first Rust release, version **0.1.0**, with an API independent of the npm version.

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
audiobox = "0.1"
```

Rust 1.85 or newer is required. No optional dependencies or feature flags are needed.

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

## Streaming PCM

`stream::PcmDecoder` retains partial frames between input chunks. `stream::PcmEncoder`
validates that successive chunks have the same sample rate and channel count.
`stream::Resampler` retains the sinc filter history and preserves phase across chunks;
call `finish` to drain its tail. Its output matches whole-buffer resampling regardless
of input chunk boundaries. Stream limits apply to the complete stream, not each chunk.

Container/MP3/FLAC decoding currently operates on complete input slices. Browser
Workers, Web Audio and WebCodecs adapters belong to the TypeScript package and are
not part of the Rust API. Dithering, Sony Wave64, RF64 encoding and file-system streaming
are also not implemented in this first Rust release.

## Development and publishing

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --release
cargo doc --no-deps
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
