# Whole-file MP3 output growth (#63)

## Scope and contracts

The whole-file decoder formerly called `try_reserve_exact` for every channel on
all decoded frames and again for the filter tail. It then copied each retained
gapless slice into a fresh vector. Incremental decoding is unchanged.

The fix uses fallible geometric growth (1.5 times prior capacity), capped by the
configured frame, decoded-byte and duration limits. Required sample counts retain
the existing checked arithmetic and limit/error checks. VBR counts retain their
existing validation but are deliberately not used as allocation hints: a declared
count can disagree with the actual stream while remaining within configured limits.
Gapless trimming uses `copy_within` and `truncate` on the existing channels.
Samples, metadata, stream shape checks, delay/padding and error categories retain
their existing behavior. The allocation tracker and failure injector are confined
to an integration test; library code remains safe Rust with zero dependencies.

Geometric capacity is retained after trimming. This trades spare capacity (normally
less than 50% of raw PCM, plus small-stream/frame rounding) for bounded total growth
and removes the trimming allocation. This does not promise smaller retained output
than the old exact-sized result, or a universal bound on process RSS. The test
allocator measures requested live bytes, not allocator internals or operating-system
page residency. Failed speculative PCM allocations return `LimitExceeded`.

## Reproduce

`tests/mp3_output_alloc.rs` contains two generated inputs, with and without Xing /
LAME gapless tags. Fixtures are created before timing and allocation tracking:

- `silence48k128`: synthetic MPEG-1 Layer III silence, 48 kHz stereo, 128 kbit/s,
  384 bytes / 1152 samples per frame. This isolates allocation from nonzero spectral
  decoding; it is not a music throughput benchmark.
- `lame_sine44k_vbr`: repeat complete audio frames from the existing MIT synthetic
  `tests/fixtures/lame-stereo.mp3` (44.1 kHz stereo, FFmpeg/libmp3lame VBR q6). Remove
  the original ID3 and repeat no Xing header; set the generated header's frame count.
  Repeated segments include encoder padding and are intended for decoding stress,
  not audio quality assessment. Average bitrate follows from input bytes and frames.

Warm transform tables before measuring. Defaults are 15/60/120/257 seconds, each
rounded up to a complete frame, both inputs and tag variants, three serial runs.
The manual benchmark is ignored in normal tests and has no time assertions:

```powershell
cargo test --locked --offline --release --test mp3_output_alloc profile_whole_mp3 -- --ignored --nocapture --test-threads=1
```

Select cases with `AUDIOBOX_PROFILE_KIND`, `AUDIOBOX_PROFILE_SECONDS`,
`AUDIOBOX_PROFILE_TAGGED` (`true` / `false`) and `AUDIOBOX_PROFILE_RUNS`. Set
`AUDIOBOX_PROFILE_UNTRACKED=1` for the native system allocator without counters.
CSV columns include actual decoded frames, wall milliseconds, allocation/reallocation
calls, requested-byte sum, requested-live peak, retained channel capacity and a
sample-bit checksum. `moved_bytes` sums old allocation sizes when `System.realloc`
returns a different address: it estimates possible copied bytes, not measured bus
traffic. Realloc-internal overlap/temporary storage is excluded from requested-live
peak. Frame counts and checksums are computed identically in untracked runs.

For an exact baseline, check out `b307879` in a separate directory, copy this test
file into its `tests/` directory, and build baseline and changed binaries separately:

```powershell
cargo test --locked --offline --release --test mp3_output_alloc --no-run
```

The build prints the executable path. On Windows, run the same selected cases in
separate processes with `scripts/profile-mp3.ps1` for each binary:

```powershell
./scripts/profile-mp3.ps1 -Executable <baseline-test.exe> -OutputPath baseline.json -Untracked
./scripts/profile-mp3.ps1 -Executable <changed-test.exe> -OutputPath changed.json -Untracked
```

The script selects untagged 60 s (3 runs), untagged 257 s (1 run), tagged 60 s
(3 runs), plus a tagged nonzero 15 s sine control (3 runs). It samples Windows
`PeakWorkingSet64` every 20 ms in a separate process per case. This process high-water
mark includes fixture generation, warmed tables, checksums and the harness, not
only decode. `ProcessCpuSeconds` covers that whole process too; CSV wall time covers
only `decode`. Keep other builds/tests/benchmarks idle for timing runs.

## Measurements (2026-10-24)

Host: AMD Ryzen 7 1700X (8 cores / 16 logical processors), Windows 10 build 19045,
x86_64-pc-windows-msvc, rustc 1.98.1 (48a229cea), LLVM 22.1.8, Cargo release profile,
System allocator. All decoding runs within each process are serial. No commercial
recording was used or added to the repository.

Preliminary baseline results below ran concurrently with other project validation
and therefore do **not** establish reliable wall-time speedups. Allocation counts
and requested-size sums demonstrate the growth mechanism independently of timing.
Values are for untagged 48 kHz / 128 kbit/s stereo silence:

| Nominal seconds | Actual output frames | Realloc calls | Sum of requested allocation bytes | Estimated moved old bytes (one run) |
| ---: | ---: | ---: | ---: | ---: |
| 15 | 719,471 | 1,250 | 1,814,417,832 | 1,566,024,192 |
| 60 | 2,879,471 | 5,000 | 28,857,617,832 | 28,563,522,048 |
| 120 | 5,759,471 | 10,000 | 115,315,217,832 | 114,969,512,448 |
| 257 | 12,336,239 | 21,418 | 528,704,547,240 | 528,200,999,424 |

The requested-size sum scales quadratically with decoded length on this workload;
changed-address sizes show that this Windows allocator frequently relocated PCM.
This demonstrates an allocation/copy bottleneck in this workload, without assigning
all of the commercial-reference observation to it. Codec transform/Huffman costs
and wider music workloads remain separate from this bounded change (#78 / #82).

Final quiet native comparisons and fixed allocation results are recorded below.

Other project builds/tests were paused for the final comparison. A manager sample
before the window found no Cargo/rustc/test workloads and low background CPU
activity (largest non-sampler approximately 0.03 CPU seconds over two seconds).
This was a coordinated quiet window, not an isolated machine. Each executable
used the same harness (SHA-256
`fd3e3c981922d9f97fc32752bddc799ba82cf9a8009c2e3765db6012471db335`).
Counters were disabled for these native wall-time / Windows process-memory runs:

| Workload | Baseline decode ms (all runs) | Changed decode ms (all runs) | Baseline / changed process peak bytes |
| --- | --- | --- | ---: |
| Silence, untagged 60 s | 9775.373 / 9941.958 / 9281.490 | 503.915 / 508.455 / 502.168 | 42,856,448 / 35,201,024 |
| Silence, untagged 257 s (one run) | 168820.076 | 2094.052 | 157,761,536 / 147,492,864 |
| Silence, tagged 60 s | 9610.404 / 9367.298 / 9643.243 | 491.459 / 512.572 / 512.369 | 42,860,544 / 35,254,272 |
| Nonzero LAME sine, tagged 15 s | 527.016 / 558.422 / 489.618 | 114.683 / 118.279 / 115.812 | 16,404,480 / 11,669,504 |

Whole-process CPU seconds (baseline / changed, covering all repeats and setup):
28.90625 / 1.578125, 168.671875 / 2.171875, 28.65625 / 1.578125,
and 1.609375 / 0.390625, respectively. Similar wall/CPU time on the long baseline,
together with the allocation relocation evidence, supports an allocation/copy
bottleneck on this synthetic workload. This is not a sampled call-stack profile
and does not characterize the remaining DSP work for arbitrary music.

The single 257 s synthetic silence observation is about 80.6 times faster on this
host. Its actual process peak improved modestly, from 150.5 to 140.7 MiB. Retained
PCM capacity increased from 98,689,912 to 137,903,256 bytes: bounded spare capacity
is a material tradeoff and must not be described as universally lower memory use.
This is not a general MP3/music speedup promise, and the commercial-reference
recording was not benchmarked. The 60 s controls show repeat variation; only the
257 s case has a single long observation.

Every native comparison retained identical channels, frames and sample-bit hash.
Silence checksums are zero; the nonzero sine checksum is `732fdc338314bca3` on both
versions. Its 661,068 output frames and 66,159 input bytes imply approximately
35.3 kbit/s including the header. The silence cases contain 2,879,471 untagged /
2,878,424 tagged frames at 60 s, and 12,336,239 untagged frames at 257 s.

A separate fixed, instrumented run (one per case, same fixture generation) gave:

| Nominal seconds | Realloc calls | Requested-byte sum | Estimated moved old bytes | Requested-live peak | Retained PCM capacity |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 15 | 32 | 24,207,416 | 13,856,004 | 8,092,712 | 8,071,160 |
| 60 | 38 | 81,714,408 | 52,895,828 | 27,261,704 | 27,240,152 |
| 120 | 42 | 183,864,968 | 121,005,420 | 61,311,888 | 61,290,336 |
| 257 | 46 | 413,703,728 | 274,231,260 | 137,924,808 | 137,903,256 |

These rows are untagged silence; tagged rows had identical capacity, allocation
count (6), reallocation count, requested-byte sum and requested-live peak. The
baseline made 8 allocations versus 6 after removing the trimming allocation for
each channel. Fixed requested-live peaks are retained capacity plus 21,552 bytes
of decoder storage: trimming introduces no second PCM allocation. These tracker
peaks still exclude transient memory inside `System.realloc`. Geometric growth
makes realloc counts logarithmic and requested-size/copy estimates linear up to
capacity-growth steps, in place of the measured quadratic baseline growth.

## Validation

All commands below passed locally on Windows, using an external target directory
because the pre-existing `target/` build lock was inaccessible. Stable is rustc
1.98.1. The manual profile is ignored by normal tests. The new deterministic
growth regression fails against `b307879` (250 reallocations for 3 s), and passes
on the fix. Its tagged/untagged silence and nonzero sine cases compare every
sample bit and both gapless endpoints against the unchanged incremental decoder.
Additional tests exercise exact byte/frame/duration boundaries, inconsistent VBR
counts (0 / 1 / 40,000), oversized tags, malformed side information, truncation,
shape changes and injected PCM growth allocation failure returning `LimitExceeded`.
Existing fixture/gapless/metadata, hostile-input and incremental allocation tests
also passed without changing their expectations.

```text
cargo fmt --check
cargo clippy --locked --offline --all-targets -- -D warnings
cargo clippy --locked --offline --all-targets --all-features -- -D warnings
cargo clippy --locked --offline --all-targets --features synth -- -D warnings
cargo clippy --locked --offline --all-targets --features mixer -- -D warnings
cargo clippy --locked --offline --all-targets --features music -- -D warnings
cargo clippy --locked --offline --all-targets --features events -- -D warnings
cargo clippy --locked --offline --all-targets --features file-stream -- -D warnings
cargo test --locked --offline
cargo test --locked --offline --all-features
cargo test --locked --offline --release --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --locked --offline --no-deps --all-features
cargo package --locked --offline --allow-dirty
cargo +1.85.0 test --locked --offline --all-features
```

Default tests: 52 passed / 1 ignored. Each all-features run (stable debug, stable
release, Rust 1.85): 282 passed / 1 ignored, including doctests. Package verification
built the package successfully. Other local checks:

```text
cargo test --locked --offline --release --target wasm32-wasip1 --all-features --test deterministic --test synth --test sfx --test assets --test music --test metadata --test effects --test colour_effects --test drums --test chip --test spatial --test events --test synthwave
cargo check --locked --offline --target wasm32-unknown-unknown --all-features
cargo fmt --manifest-path site/wasm/Cargo.toml --check
cargo clippy --manifest-path site/wasm/Cargo.toml --locked --offline --all-targets -- -D warnings
cargo build --manifest-path site/wasm/Cargo.toml --locked --offline --release --target wasm32-unknown-unknown
node --test site/tests/engine.test.mjs
```

WASI used `CARGO_TARGET_WASM32_WASIP1_RUNNER=node --no-warnings scripts/wasi-run.mjs`;
all 13 selected test binaries passed. The final Node check ran an unchanged copy
of `engine.test.mjs` against the freshly rebuilt WASM in a temporary site layout:
5 passed, 0 failed. No hosted workflow or release was dispatched. Linux/macOS
were not tested locally. Broader representative-music profiling remains #78's
shared performance work; this change addresses #63's demonstrated PCM growth and
trimming allocations.
