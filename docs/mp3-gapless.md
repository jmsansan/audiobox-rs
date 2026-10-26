# MP3 priming, padding and independent decoding (0.5)

Engine #95 fixes newly encoded MP3 files so that FFmpeg applies priming/padding
without a Studio byte patch or consumer-side trim. The codec remains audiobox's
own safe-Rust MPEG-1 CBR encoder. This changes gapless metadata and the minimum
short-file framing, not transform/quantizer math or the source PCM.

## Layout and identification

An initial non-audio `Info` frame records the coded audio-frame count, MPEG byte
count and a 36-byte LAME-layout extension. `LAMEabox1` is audiobox's explicit
compatibility identifier for that extension; it is not an official LAME version
or a statement that LAME encoded the audio. The `abox1` suffix identifies this
writer/layout. Revision zero/CBR, music length, music CRC and Info CRC are written.
Caller ID3 fields and JPEG/PNG covers are unchanged.

The prefix is needed because FFmpeg 9.0.1 only accepts gapless delay from selected
Info identifiers. Its Info checksum also controls publication of the encoder tag.
See the [independent demuxer source](https://github.com/FFmpeg/FFmpeg/blob/n9.0.1/libavformat/mp3dec.c#L239-L280).
Both CRCs use reflected CRC-16/ANSI (polynomial 0xa001, initial zero). Music CRC
covers coded audio bytes after Info; Info CRC covers the MPEG header through the
music CRC, excluding ID3 and the Info CRC itself. The MPEG byte counts include the
Info frame and exclude ID3. There is no added decoder dependency.

At least two coded audio frames follow Info, including for zero/one-frame PCM.
This satisfies independent demuxers that look for two consecutive audio frames.
The extra coded silence is counted as padding and never returned as body PCM.

## Frame-zero and length contract

At every supported encoder rate (32 / 44.1 / 48 kHz), one MPEG-1 audio frame
represents 1,152 source-sample frames. For an input of N PCM frames:

```text
coded_frames = max(2, ceil((N + 528 + 576) / 1152))
encoder_delay = 528
encoder_padding = coded_frames * 1152 - N - encoder_delay
```

The 576-frame flush allowance retains the existing analysis/MDCT tail policy.
FFmpeg discards 528 + 529 = 1,057 frames of combined encoder/decoder priming,
then `encoder_padding - 529` trailing frames. Therefore its unmodified decode
returns exactly N PCM frames. For one second of stereo 48 kHz silence: 43 coded
frames, delay 528, padding 1,008; FFmpeg discards 1,057 leading and 479 trailing
frames from 49,536 decoded frames and returns 48,000. Short padding can span
multiple packets; summing packet discard values is necessary when inspecting it.

Returned frame zero is the lossy codec's rendition aligned to source frame zero,
not a promise of equal sample amplitude or lossless startup/tail reconstruction.
Audiobox's internal synthesis filter also accounts for its 64-frame flush state;
that existing convention is retained. `Mp3Info.frames` counts
MPEG frames; its delay/padding fields use the convention above. Whole decoding,
incremental reads and seeks retain the same aligned samples and exact length.

Old files bearing the printable `audiobox ` Info identifier continue to decode
inside audiobox. Their bytes are not automatically rewritten, and their old
FFmpeg behavior remains. New exports from original PCM get the correction.

## Compatibility and evidence

Encoded MP3 bytes intentionally change in 0.5: the identifier, revision/CBR byte,
music length and CRC fields differ. Inputs below 49 frames gain a coded audio
frame. WAV/FLAC, normalization PCM/reports and MP3 DSP are unchanged. All 246
source/decoded-PCM hashes in the [export matrix](mp3-export-contract.md) were
compared before updating **only** encoded lengths/hashes. The six mastering MP3
hashes likewise change only at the encoded stage; their PCM/report/lossless and
decoded hashes remain pinned.

For inputs of 32..=48 PCM frames, the extra coded frame can correct a few final
samples that previously came from the internal decoder's uncoded filter flush.
The short compatibility fixture retains the first 31 samples exactly and measures
a maximum tail difference below 0.00001 amplitude (about 0.0000069 observed).
This is an intentional short-file PCM compatibility effect, while length/frame
zero and all 246 earlier matrix hashes for decoded PCM remain unchanged.

`tests/mp3_gapless.rs` verifies empty/short/nonaligned shapes at every rate,
mono/stereo and 32/320 kbit/s, independent MSB-first CRC reference checks,
structured tags/cover, legacy identity, incremental reads and seeks. Existing
decoder-allocation controls continue to apply. The suite runs in native/MSRV/WASI
CI; it needs no external software.

The opt-in `tests/interop.rs` suite requires `ffmpeg` and `ffprobe` on PATH:

```sh
AUDIOBOX_FFMPEG_TESTS=1 cargo test --locked --offline --release --test interop -- --nocapture
```

On Windows PowerShell, set `$env:AUDIOBOX_FFMPEG_TESTS='1'` first. The 96 gapless
controls cover empty/one-frame PCM, 48/49-frame framing, short/nonaligned impulses,
one-second silence, all encoder rates, mono/stereo, 32/320 kbit/s and tagged PNG
files. They require exact external PCM counts, 1,057 skip frames, the declared
sum of padding, and FFprobe's CRC-validated `LAMEabox1` encoder field. No manual
sample alignment is accepted. Matched audiobox/FFmpeg decoder samples use an
absolute amplitude tolerance of 0.0015 on these transient controls; this is
decoder agreement, not a perceptual-quality guarantee. Existing sine controls
still require source-to-FFmpeg SNR above 30 dB and now also require exact length.

Local independent checks used the verified Gyan FFmpeg/FFprobe 9.0.1 Windows
build (archive SHA-256
`fec81ae03971d9dd4be3ebe02e263bd2ec1d789483f931bdba5f5715e65da2e9`).
The pre-fix one-frame reproduction failed to open; the corrected file returns
one stereo frame and publishes Skip Samples. FFmpeg 7.1 also passed that short
smoke control. These are tested implementations, not a promise for every player.

## Measured export cost

Windows x86_64 / Ryzen 7 1700X / Rust 1.98.1 release / stereo 48 kHz, 10-second
original benchmark fixture, one warm-up + five timed + one allocation-counted run:

| Encoder | Median ms | Allocations / reallocations | Phase peak bytes |
| --- | ---: | ---: | ---: |
| Before | 756.2951 | 2,094 / 4,576 | 786,304 |
| Gapless correction | 757.5050 | 2,094 / 4,576 | 786,304 |

The median difference is about 0.16%, within one-host timing variation. Fixtures
are prepared outside timing and no other engine tests ran during measurement.
The baseline uses the pre-fix encoder from parent commit `adba148`; raw metadata,
min/max timing, byte hashes and memory counters are in the
[before](https://github.com/jmsansan/audiobox-rs/blob/v0.5.1/docs/performance/mp3-gapless-before-windows.csv) and
[after](https://github.com/jmsansan/audiobox-rs/blob/v0.5.1/docs/performance/mp3-gapless-after-windows.csv) CSVs. This does not set a
portable throughput/deadline bound. Reproduce with:

```sh
cargo bench --locked --offline --all-features --bench engine -- codec/mp3-encode/10s --quick --runs 5 --csv target/mp3-gapless-bench.csv
```
