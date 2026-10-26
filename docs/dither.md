# Seeded integer PCM dither

This implements the integer-export primitive for 0.5 tracked in #66. It is opt-in:
`EncodeOptions::dither = Dither::Tpdf { seed }`. `Dither::None` is the default and
keeps the previous quantization and encoded bytes. A full `EncodeOptions` literal
needs the new field; literals using `..Default::default()` need no change.

## Quantization

For destination width `b`, scale `S = 2^(b-1)`, finite source `x` and noise `n`:

```text
q = clamp(round(f64(x) * S + n), -S, S-1)
```

Rounding uses Rust's `round` (nearest, ties away from zero). With dither disabled,
`n = 0`. TPDF uses the difference of two uniform draws in `[0, 1)`, so
`-1 < n < 1` destination LSB: two LSBs peak-to-peak, variance 1/6 LSB² before
quantization. The addition happens in f64 at the destination scale, preserving
noise even at 32 bits. Samples outside the integer range saturate; there is no
wraparound. U8 shifts the signed result by 128 only after this calculation.

Rounded exact silence produces -1, 0 and 1 LSB with probabilities 1/8, 3/4 and
1/8, respectively (0.5 LSB RMS). Dither adds noise to silence deliberately. Empty
input emits no samples and consumes no random draws. Clipping at the range limits
can bias the noise; zero mean and decorrelation apply away from clipping.
The input Audio remains unchanged; there is no intermediate dithered f32 buffer.

## Supported destinations

| Destination | Dither support |
| --- | --- |
| Raw PCM | U8, S8, S16, S24, S32; either byte order |
| WAV | U8, S16, S24, S32; little endian, including speaker masks |
| AIFF, AU | S8, S16, S24, S32; U8 selects the existing signed 8-bit conversion |
| CAF | S8, S16, S24, S32; either byte order; U8 with dither is rejected |
| FLAC | 8, 16, 24, 32 bits; U8 and S8 both select signed 8-bit samples |
| Float PCM, G.711, MP3 | Enabled dither returns InvalidArgument |

FLAC predicts, compresses and hashes the quantized integers without adding noise
again. Its STREAMINFO MD5 covers those integer samples, including the low bits of
32-bit output that a decode to the crate's f32 Audio cannot retain. WAV, raw PCM
and FLAC have identical intended integer samples for equal bit depth/seed/input;
byte order and unsigned 8-bit storage are container differences. Tags, covers and
FLAC compression level do not enter the noise stream.

## Reproducible streams

The seed and zero-based channel index initialize a local SplitMix64 state:

```text
G = 0x9e3779b97f4a7c15
mix(v):
    v = (v xor (v >> 30)) * 0xbf58476d1ce4e5b9
    v = (v xor (v >> 27)) * 0x94d049bb133111eb
    return v xor (v >> 31)
state[channel] = mix(seed xor ((channel + 1) * G))
draw():
    state += G
    return (mix(state) >> 11) / 2^53
n = draw() - draw()
```

All integer arithmetic wraps modulo 2^64. The top 53 bits convert to f64 exactly;
the division is an exact power-of-two scaling. Each channel advances twice per
sample, including clipped samples. Channel zero therefore has the same noise in
mono and multichannel exports. No global state, time or render order enters
the calculation. These choices and the output checksums define this mode's stream;
changing its algorithm requires an explicit compatibility decision.

Whole exports and `pcm::encode_pcm_with_dither` restart the seed. For incremental
input, `stream::PcmEncoder::with_dither(format, endian, dither)` retains each
channel's state and validates channel count/sample rate before advancing it. Empty
chunks consume no draws; shape errors preserve state. A new encoder restarts it.
As before, `push` allocates its returned bytes; this API is not a mixer callback
encoder. Dither uses bounded additional state per input channel, with no
sample-sized noise buffer. Disabled dither adds no state allocation to PCM output.

Apply dither at the final integer export, after all DSP. Re-encoding an already
dithered integer recording with dither enabled adds another noise realization;
the engine does not infer a caller's previous processing history. Float and lossy
exports must explicitly leave it disabled. There is no noise shaping.

## Verification

`tests/dither.rs` checks all supported PCM widths, both byte orders, saturation,
zero/short input and FLAC block boundaries, export/seed isolation, metadata,
stream chunks of 1/7/256/4096 frames and rejected shape changes. It pins raw PCM
and WAV/FLAC encoded bytes with a rational-input fixture; the same tests run on
Linux, macOS, Windows and Node WASI in CI. FLAC also checks its exact integer MD5
against raw quantization, covering 32-bit information lost by f32 decoding.
The disabled-dither regression compares 98 format/width/byte-order combinations
with hashes recorded from an independently compiled 0.4.1 (`7477cb1`), and checks
that aggregate on native targets and WASI. The same WASI fixtures also cover a
pre-existing FLAC Rice-cost overflow: 32-bit residual costs now use u64 before
choosing the bounded verbatim fallback.

The optional `AUDIOBOX_FFMPEG_TESTS=1 cargo test --test interop` job independently
decodes dithered 16/24/32-bit WAV and FLAC with FFmpeg and compares the exact integer
bytes with raw PCM. It runs on releases/manual dispatch, following the existing
interoperability policy; FFmpeg is not a crate dependency.

Statistical controls use 131072 frames: silence mean within 0.01 LSB of zero,
power within 0.01 LSB² of 0.25, each outer-bin probability within 0.01 of 0.125,
and absolute channel correlation below 0.02. A quarter-LSB 997 Hz tone checks
error within 1.5 LSB, mean within 0.01 LSB, power within 0.01 LSB² of 0.25 and
absolute source/error correlation below 0.02. These are reproducible regression
tolerances, not a universal listening-quality assertion.

## Local validation and cost (2026-10-26)

Windows x86_64, AMD Ryzen 7 1700X, Rust 1.98.1 release, all features. The existing
engine benchmark generated a 10-second stereo 48 kHz fixture before timing, then
ran one warm-up, five timed exports and a separate allocation-counted export.
Baseline 0.4.1 was extracted from `7477cb1` (its CSV has no Git identity because
it was an archive); the development branch ran immediately after it, with no
concurrent test suites. Raw results: [before](https://github.com/jmsansan/audiobox-rs/blob/v0.5.1/docs/performance/dither-before-windows.csv)
and [after](https://github.com/jmsansan/audiobox-rs/blob/v0.5.1/docs/performance/dither-after-windows.csv).

| 16-bit export | 0.4.1, no dither | Development, no dither | Development, TPDF seed 7 |
| --- | ---: | ---: | ---: |
| WAV | 25.65 ms | 24.72 ms | 33.97 ms |
| FLAC | 95.51 ms | 90.42 ms | 92.76 ms |

The default encoders retained their checksums and allocation counts. Dither added
one 48-byte channel-state allocation for stereo WAV and no extra allocation to
FLAC's existing quantization pass. This is one run on one host; the differences
between undithered builds are within its observed timing spread, not evidence of
a general speed improvement. The added benchmark cases run on demand, with the
same timing and checksum conventions as the release baseline:

```text
cargo bench --bench engine --all-features -- codec/wav-encode/10s codec/wav-tpdf-encode/10s codec/flac-encode/10s codec/flac-tpdf-encode/10s --quick --runs 5
```

The complete native suite passed in debug/release, and the dither tests passed
on native Rust 1.85 and Node 24 WASI with identical golden hashes. Clippy,
rustdoc warnings and package verification were checked locally. FFmpeg was not
installed on this host, so the optional independent interoperability test remains
for the release/manual-dispatch job; no local FFmpeg result is claimed.