# Ogg decoding

`decode`, `read_audio_file` and `stream::Decoder` recognize Ogg Vorbis I from
its identification packet. `AudioFormat::Vorbis` explicitly selects it; `ogg`,
`oga` and `vorbis` parse to that format. Opus is detected from `OpusHead`;
`AudioFormat::Opus` or `opus` selects it explicitly. Neither codec encodes.

```rust
use audiobox::{decode, stream::Decoder};
fn import(bytes: &[u8]) -> audiobox::Result<()> {
    let audio = decode(bytes)?;
    let mut decoder = Decoder::new(bytes)?;
    let mut buffer = vec![0.0; 256 * decoder.channels()];
    decoder.seek(100)?;
    let frames = decoder.read(&mut buffer)?;
    let at = 100.min(audio.frames());
    assert_eq!(&buffer[..frames * audio.channels()],
        &audio.to_interleaved()[at * audio.channels()..(at + frames) * audio.channels()]);
    Ok(())
}
```

Vorbis supports 1–8 channels, floor 0/1, residue 0/1/2, coupling and short/long
block switching. Rates pass through `Limits`; there is no implicit resampling.
Multichannel PCM follows WAV speaker order, including 6.1. Headroom is retained:
decoded floats may exceed ±1. The mixer's mono/stereo source requirement is unchanged.

## Container and timing

Opening checks every page's capture/version/flags, CRC, serial, sequence,
continuation/lacing and EOS, without storing a page/packet index. Audio block
durations must agree with granules. Empty streams with three valid headers and
zero program length work. Chained/multiplexed links are explicitly unsupported.

The first synthesis packet primes overlap state and emits no PCM. The first
non-EOS audio granule can establish an initial offset: positive origins are
removed from the public frame count; negative origins trim initial overlap.
EOS can shorten only the final packet. A forged small granule cannot hide an
arbitrary number of decoded packets. `frames()` reports the presented count.

`seek` resets existing state and replays from the first audio packet. It is
sample-exact and allocates/frees nothing after preparation, but costs O(distance
from the start). `rewind` resets immediately without reparsing/replanning.
`next_chunk` creates owned `Audio`, as for other formats; use `read` for streaming.
Native `FileStream` retains its WAV/FLAC/MP3 disk-reader scope. For Ogg use
resident encoded bytes with `Decoder`, or whole-file import and shared `Audio`.

## Bounds and metadata

Presented PCM shape/duration/frames/bytes use `Limits`. Incremental opening also
checks its block output capacity, as other incremental codecs do. Working memory
is independent of duration: one reusable packet, setup tables, transform plans,
channel scratch/overlap and one output block. No decoded-audio cache grows with
the file. Preparation conservatively accounts for workspace up to
`min(Limits::max_decoded_bytes, 64 MiB)`, separately from whole-file PCM output.
Data-dependent codebook allocations are charged before allocation.

A packet is capped at 1 MiB, a codebook at 65,536 entries/8192 dimensions and a
Vorbis block at 8192 source samples. Oversized configurations return
`LimitExceeded`; unsupported channels/links return `UnsupportedFormat`.
Corrupt input returns errors. Successful reads/seeks/rewinds reuse storage;
error paths may construct messages, as existing decoders do.

Comments validate UTF-8, field syntax and framing; trailing comment padding is
ignored within the packet cap. At most 4096 fields are accepted. Native keys
are lowercase; duplicate keys keep the last value, matching the map API.
Known fields populate `DecodeInfo::tags`. Embedded image comment strings remain
native metadata; this reader does not extract their image payloads.

## Verification and source provenance

Packet DSP adapts a pinned MIT-licensed TPT Cadence revision and its BSD-licensed
Opus reference port. Their licenses,
[provenance and corrections](../src/ogg/NOTICE.md) are retained in the package.
No Cargo dependency, C, FFI or build-time download is introduced. Ogg framing,
API/limit integration and seek policy are audiobox code.

35 synthetic encoded inputs and full f32 references cover mono rates 8–96 kHz,
all channel counts 1–8, qualities −1/0/4/10, short lengths 1–1025 and authored
floor-0 odd/even/vector cases. Seven review regressions add forward chained
coupling, valid floor/residue packet exhaustion (including type-2 interleaving),
floor-1 shared short/long geometry and a 256-book/8192-dimension lookup setup. libsndfile 1.2.2 / libvorbisfile (SoundFile 0.14.0)
independently decoded every reference. The 25 libvorbis-encoded cases were also
compared against aligned FFmpeg 9.0.1 samples where available. FFmpeg can expose
padding or omit a very short packet, so its raw length is not the trim oracle.
All full reference lengths equal the Ogg program lengths. References do not come
from audiobox or the adapted DSP.

Every sample is compared with absolute tolerance 0.00001 and exact frame counts,
without a lag search or consumer trimming. Whole/incremental/seek PCM is exact
within a host across 1/7/127/4096-frame reads. Cross-host lossy PCM bit identity
is not promised: transform preparation uses platform floating-point math.
These controls also run on MSRV and WASI. Tests cover continuation pages, zero
laces, comment padding, initial offsets, empty streams, truncation/CRC/sequence,
setup budgets, repaired-CRC mutations and allocator activity (floor 0 and 7.1).

The [fixture manifest](https://github.com/jmsansan/audiobox-rs/blob/v0.5.1/tests/fixtures/ogg/vorbis.csv) records encoded/reference
hashes; [the generator](https://github.com/jmsansan/audiobox-rs/blob/v0.5.1/tests/fixtures/generate_ogg.py) authors integer signals
and minimal floor-0 packets. FFmpeg and libvorbisfile are development oracles only.

## Opus

Opus supports SILK, CELT and hybrid, mono/stereo mode changes, 2.5–120 ms packets,
CBR/VBR packing, padding, DTX and in-band FEC-bearing packets (normal decoding,
not an exposed loss-recovery API). Mapping families 0/1 cover 1–8 output channels,
including coupled/uncoupled streams, duplicated and silent mappings. Output is
always 48 kHz in WAV speaker order. The original input-rate field is informational.
Other mapping families, chained links and multiplexed links return errors.

EOS may trim within the final page, including more than one packet (RFC 7845
§4.4); it cannot discard audio from previous pages. The public timeline removes
`pre_skip` and final EOS trim, and subtracts a
positive initial granule origin. Header gain in signed Q8 dB is applied once,
without clipping float headroom. All stream durations must agree in a multistream
packet. Each coded frame is at most 1275 bytes, each packet at most 120 ms and each
decoder at most eight coded streams. Reusable PCM capacity is 5760 frames.
Preparation charges 2 MiB per coded stream plus packet, output and metadata
storage within the same Ogg workspace budget.

35 synthetic libopus-encoded fixtures and complete, aligned float libopus
references (FFmpeg 9.0.1, explicitly requested float sample format) cover all
three modes, durations, internal rates, 1–8 channels, short trim, transitions,
gain, DTX and FEC. Each sample uses absolute tolerance 0.006 and each complete
fixture requires SNR >50 dB (or exact equality). Observed native maximum error is
0.005711 and minimum SNR 54.15 dB; most SILK cases are exact. CELT floating-point
synthesis differs from libopus builds. No lag search, resampling or consumer trim
is allowed. Whole/read/replayed-seek samples are bit-identical within a host.
Tests cover malformed packets, limits, silent/duplicate mappings, CRC-repaired
mutations and zero callback allocations/frees. The manifest is
[`opus.csv`](https://github.com/jmsansan/audiobox-rs/blob/v0.5.1/tests/fixtures/ogg/opus.csv); regenerate with
`python tests/fixtures/generate_opus.py /path/to/ffmpeg`.

The optional ignored unit test `official_rfc8251_ranges_and_pcm` reads the
[official RFC 8251 vectors](https://opus-codec.org/docs/) from
`AUDIOBOX_OPUS_TESTVECTORS`. The archive SHA-256 is
`6b26a22f9ba87b2b836906a9bb7afec5f8e54d49553b1200382520ee6fedfa55`.
All 20,075 packet final ranges match; full stereo PCM lengths agree and measured
SNR is 85.17–111.44 dB, with vectors 2/3/4 integer-exact. The optional check
requires >80 dB or exact equality. External vector audio is not bundled or downloaded
by Cargo or normal CI. Synthetic fixture controls run offline on native/MSRV/WASI.
