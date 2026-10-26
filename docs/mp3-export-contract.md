# MP3 export reproducibility

For 0.5, the MP3 contract is **exact regression checks for the named fixtures on
the tested matrix**, rather than arbitrary-input MP3 byte identity across hosts,
compiler versions or audiobox versions. Studio should select WAV/FLAC when its
export policy requires a general exact-byte promise. MP3 is a lossy delivery
format; its decoded PCM is not the original PCM, even when repeated exports match.

No MP3 codec math changes are part of this work (#100). Transform preparation
still uses platform `sin`/`cos`, and encoder quantization and decoder
requantization use platform `powf`. Agreement on a finite corpus cannot prove
that all rounding/quantizer decisions will agree for every possible input. A
compiler, target, optimization or codec change needs fresh comparison evidence.

The guarantees for deterministic normalization PCM/reports and integer PCM,
WAV and FLAC export remain separate and unchanged; see
[normalization and export stages](normalization.md#export-audit). Identical options
include sample format, dither mode/seed and tags. Host-dependent analysis or input
generation must not be mistaken for a codec difference.

## Fixture matrix

`tests/mp3_contract.rs` compares 246 original synthetic cases against
[`tests/fixtures/mp3-contract.csv`](../tests/fixtures/mp3-contract.csv). Sources use
integer LCG state, rational/dyadic samples and explicitly recorded f32 scale bits;
they do not call platform transcendental functions or normalize/resample input.
All fixtures belong to this MIT-licensed repository. Tagged cases reuse the
synthetic PNG described in the fixture README.

| Dimension | Coverage |
| --- | --- |
| Sample rate | All encoder rates: 32,000 / 44,100 / 48,000 Hz |
| Channels | Mono and stereo |
| Bitrate | Representative minimum / middle / maximum: 32 / 128 / 320 kbit/s |
| Short/boundary lengths | 0 / 1 / 575 / 576 / 1,151 / 1,152 / 1,153 / 2,305 frames, with impulses at the start, end and available granule boundaries |
| General signal | 3,457 frames of seeded noise plus a rational triangle |
| Quiet/silent | 2,305 frames at scales 2^-24 / 2^-100, and exact silence |
| Structured tags | Title, artist, album and PNG cover at all rate/channel/bitrate combinations |
| Quantizer decision neighbors | One adjacent positive f32 scale pair per rate/channel shape at 128 kbit/s, 2,305 seeded frames |

The first six categories contribute 234 cases. The six neighbor pairs add 12.
For each pair, a manual binary search finds adjacent scale bit patterns whose
encoded audio differs, while all headers/options/tags remain fixed. The test
checks adjacency and different MP3 hashes. These are actual output-decision
boundaries, not a claim that every internal quantizer threshold is covered. The
search is reproducible and uses no private codec API.

The CSV records the case ID, rate, channels, bitrate, frame count, signal, exact
scale bits, source PCM hash, MP3 byte length/hash and decoded frame count/PCM hash.
Hashes are FNV-1a 64-bit regression fingerprints, not cryptographic proofs. PCM is
hashed as channel-major f32 sample bits in little-endian order, without integer
quantization. Options/PCM comparisons run before MP3 comparisons, which run before
decoded PCM comparisons. Probe delay/padding and decoded shape must also recover
the source frame count. A report is written before baseline assertions, retaining
actual results if a host differs.

## Supported verification matrix and artifacts

The Rust CI workflow runs the same baseline with:

| Job | Compiler | Profiles/features | Runtime |
| --- | --- | --- | --- |
| `ubuntu-latest` | stable | debug/default, debug/all, release/all | Native Linux |
| `macos-latest` | stable | debug/default, debug/all, release/all | Native macOS |
| `windows-latest` | stable | debug/default, debug/all, release/all | Native Windows |
| `msrv` on Ubuntu | 1.85.0 | debug/all | Native Linux |
| `webassembly` on Ubuntu | stable | release/all, `wasm32-wasip1` | Node 22 WASI preview 1 |

Each passing run asserts identical fixture PCM, MP3 bytes and audiobox-decoded
PCM hashes against that one checked-in baseline. CI uploads `mp3-contract-*`
artifacts containing per-profile CSVs, compiled architecture/OS/debug metadata
and `rustc -Vv` output. The workflow run identifies the commit and runner image;
use its reported architecture/compiler rather than assuming that a rolling
`*-latest` label is fixed. These artifacts remain subject to GitHub retention.
The checked-in CSV and generator allow later reconstruction.

This covers the reported runner architectures, default Rust floating-point
settings and the listed compiler/runtime versions. It does not extend a promise
to every CPU, future compiler, browser runtime, custom target feature, fast-math
configuration or MP3 stream from another encoder. `wasm32-unknown-unknown` browser
execution is not the WASI test job.

Local initial comparison on 2026-10-26 used Windows x86_64, Rust 1.98.1 release,
Rust 1.85.0 debug and Node 24.19.0 WASI release. All 246 cases matched the baseline
at each stage. Native Linux/macOS/Windows and the CI Node 22 matrix provide the
additional checks before integration. No encoded-byte or decoded-PCM divergence
has been reproduced by these cases; that observation does not expand the contract.

## Reproduction and investigation

On PowerShell, write a native report and compare its rows with the baseline:

```powershell
$env:AUDIOBOX_MP3_CONTRACT_REPORT = 'target/mp3-contract/native-release.csv'
cargo test --locked --offline --release --test mp3_contract
Compare-Object (Get-Content tests/fixtures/mp3-contract.csv) (Get-Content $env:AUDIOBOX_MP3_CONTRACT_REPORT)
```

On a POSIX shell, including a native or WASI CI reproduction:

```sh
AUDIOBOX_MP3_CONTRACT_REPORT=target/mp3-contract/native-release.csv \
  cargo test --locked --offline --release --test mp3_contract
AUDIOBOX_MP3_CONTRACT_REPORT="$PWD/target/mp3-contract/wasi-release.csv" \
  CARGO_TARGET_WASM32_WASIP1_RUNNER='node --no-warnings scripts/wasi-run.mjs' \
  cargo test --locked --offline --release --target wasm32-wasip1 --test mp3_contract
diff -u tests/fixtures/mp3-contract.csv target/mp3-contract/wasi-release.csv
```

For a single CSV case, the native manual helper saves `source.wav` (f32),
`encoded.mp3`, `decoded.wav` (f32) and a case/hash manifest under
`target/mp3-contract-case`. Choose another output directory with
`AUDIOBOX_MP3_CONTRACT_DIR`:

```powershell
$env:AUDIOBOX_MP3_CONTRACT_CASE = 'boundary-48000-2-128-2305-3e8010fa'
cargo test --locked --offline --release --test mp3_contract dump_export_case -- --ignored
```

Re-run the manual neighbor search with:

```sh
cargo test --locked --offline --release --test mp3_contract find_quantizer_boundaries -- --ignored --nocapture
```

Investigate a mismatch in stage order: first identical source sample bits and
options; then encoded length/bytes; then decoded samples. Retain both reports,
compiler/runtime metadata and the individual case files before considering a
codec change. The manual `emit_export_matrix` helper can write a candidate report
to `AUDIOBOX_MP3_CONTRACT_REPORT` without comparing the baseline; it is not a
reason to overwrite expected hashes after a failure. Any actual math correction
needs its failing input, independent decoder/quality checks, gapless/tag coverage,
compatibility notes and release benchmarks.

This corpus tests reproducibility, not perceptual quality or independent decoder
timing. The separate [gapless correction (#95)](mp3-gapless.md) changes the Info
extension/minimum framing and encoded hashes. All 246 source/decoded PCM hashes
remain unchanged. Its FFmpeg controls independently check exact length and skip
metadata; this corpus alone cannot establish independent trim behavior. Studio
consumes that engine correction rather than patching MP3 bytes itself.
