# Performance baselines and profiling (#78)

This is the shared measurement protocol for the 0.4.1 performance work. Each
optimization records its before/after comparison here, with links to its issue:
#63 (whole-file MP3), #79 (analysis/DSP), #80 (mixer), #81 (synth/SFX), #82
(codecs) and #83 (file streaming and assets). Numbers describe the host they ran
on. They are not throughput or real-time guarantees.

## Running

The runner is a `cargo bench` target without a harness or dependencies,
`benches/engine.rs`. It needs the `sfx` and `mixer` features. With `assets` and
`file-stream` too, it also covers disk streaming and the loader:

```text
cargo bench --bench engine --all-features                      # full baseline
cargo bench --bench engine --all-features -- --quick           # 10 s inputs only
cargo bench --bench engine --all-features -- mixer/ synth/     # cases containing a pattern
cargo bench --bench engine --all-features -- --skip 180s       # leave cases out
cargo bench --bench engine --all-features -- --list            # case names
cargo bench --bench engine --all-features -- --runs 3 --csv results.csv
cargo bench --bench engine --all-features -- --compare base.csv new.csv
```

Normal builds, tests and CI do not run it. `cargo clippy --all-targets
--all-features` lints it, and `cargo test` builds it only when asked.

WebAssembly runs use the same cases, except disk streaming and the loader, which
need threads. They go through Node's WASI, as the checksum tests do:

```text
CARGO_TARGET_WASM32_WASIP1_RUNNER="node --no-warnings scripts/wasi-run.mjs" \
  cargo bench --bench engine --target wasm32-wasip1 --features sfx,mixer -- --quick
```

Record native and WebAssembly results separately: they measure different
compilers, runtimes and timers.

## What a run measures

Fixtures are generated before any timing: a deterministic music-like signal
(decaying harmonic notes over a bass line, with a little noise), encoded where a
case needs bytes, and three scores (chiptune, synthwave, drum kit). Inputs come in
10 s and 180 s versions. Mixer cases render 10 s and 60 s. Stereo 48 kHz is the
main layout, with mono 44.1 kHz controls for short decodes and 5.1 for spatial
mixing.

Every case runs once to warm up and then `--runs` times (default 5) with
allocation counting off. One last run counts allocations. The counting run is
never timed, so counters do not inflate times. Every run must produce the
warm-up's output checksum, or the runner stops.

Cases split into phases, so preparation, steady work and teardown stay separate:

| Phase | Meaning |
| --- | --- |
| `open`, `prepare`, `parse` | Creating a decoder, mixer, synth or stream before any output |
| `first-block`, `first-pcm` | The first output block, or the first frames a disk reader delivers |
| `decode`, `encode`, `compute`, `transform`, `render`, `steady`, `sustained` | The main work |
| `seek` | From a seek request to the first frame at the new position |
| `drop` | Destruction and retirement |

CSV columns:

- `median_ms`, `min_ms`, `max_ms`: wall time over the timed runs.
- `mframes_per_s` and `realtime_x`: audio frames processed per second, and audio
  seconds per wall second.
- `allocs`, `reallocs`, `frees`: allocator calls during the phase, on every thread.
  Reader and loader threads count while the phase runs.
- `requested_bytes`: the sum of requested sizes, the cost model for copying growth.
- `moved_bytes`: old sizes of reallocations that changed address. This estimates
  copied bytes; it does not measure them.
- `peak_live_bytes`: the highest net bytes allocated and not yet freed during the
  phase. It counts requested sizes, not allocator overhead or resident pages.
- `callbacks`, `p50_us`, `p99_us`, `p999_us`, `max_us`: times of individual
  callbacks (mixer `render`, synth `fill`) over the timed runs.
- `deadline_us` and `over_deadline`: a callback's audio duration (256 frames at
  48 kHz is 5333 µs), and how many callbacks took longer. Game-thread work between
  callbacks, such as sending commands and polling events, is excluded from timing
  and counting. For a sustained disk read, the column reports reader retries
  (`underruns=`) while the consumer drains as fast as it can. Those show
  reader-bound throughput, not playback gaps.
- `process_rss_bytes`: the whole process, including fixtures, harness and allocator
  caches. On Linux it is the high-water mark (`VmHWM`). On macOS and other Unix
  systems it is the current size from `ps`. Windows leaves it empty: use
  `scripts/profile-mp3.ps1` for a per-process `PeakWorkingSet64`.
- `checksum`: a hash of the case's output. `--compare` flags changed outputs, so an
  optimization that changes samples cannot pass unnoticed.

Each CSV begins with `#` provenance lines: crate version, commit (marked
`(modified)` with uncommitted changes), rustc, profile, features, target, CPU,
logical CPUs, run counts, scale and time. Before a comparison, note anything else
the runner cannot see: power mode, background load and thermal state. Keep the
machine otherwise idle, and run base and new builds one after another on the same
host.

Callback percentiles from one host and one OS state describe that run only. A
p99.9 below the deadline is an observation, not a real-time guarantee.

## Comparing two commits

Build each commit in its own worktree. Rebuild the same runner in the old
worktree by copying `benches/engine.rs` and its `[[bench]]` entry, so both sides
measure identical cases:

```text
git worktree add ../audiobox-base <base-commit>
cp benches/engine.rs ../audiobox-base/benches/   # plus the [[bench]] entry in Cargo.toml
(cd ../audiobox-base && cargo bench --bench engine --all-features -- --csv ../base.csv)
cargo bench --bench engine --all-features -- --csv ../new.csv
cargo bench --bench engine --all-features -- --compare ../base.csv ../new.csv
```

## Profiling

External profilers are development tools, not dependencies. Each runs the
release benchmark binary with a case filter. `cargo bench --no-run` prints its
path (`target/release/deps/engine-<hash>`):

- **macOS:** `xctrace record --template 'Time Profiler' --launch -- <binary>
  mixer/spatial --runs 20`, then open the trace in Instruments. For a quick text
  call tree, run the binary and `sample <pid> 5 -file profile.txt`.
- **Linux:** `perf record -g <binary> mixer/spatial --runs 20`, then `perf
  report`. The allocation columns come from the runner itself. `heaptrack`
  attributes allocations to call stacks.
- **Windows:** Windows Performance Recorder (`wpr -start CPU`, run the binary,
  `wpr -stop trace.etl`) and Windows Performance Analyzer for sampled stacks.
  Visual Studio's CPU Usage and Memory tools also work.
  `scripts/profile-mp3.ps1` shows how to sample a separate process's peak working
  set.
- **WebAssembly:** `node --cpu-prof scripts/wasi-run.mjs <engine.wasm> mixer/`
  writes a `.cpuprofile` for Chrome DevTools.

Say whether a profiler was attached when you report its numbers. Sampling
perturbs timing, so take before/after times from runs without a profiler.

## Compatibility rules for optimizations

These come from the 0.4.1 issues and apply to every change recorded here:

- Keep the public API and defaults, safe Rust, zero dependencies, limits, errors,
  timing, tails and determinism. No fast-math, unsafe intrinsics, reduced-quality
  modes, larger unbounded caches or reordered floating-point reductions.
- Compare outputs directly: identical bits where the crate promises them, the
  existing codec contract elsewhere. Never loosen a tolerance to accept a change.
- Each optimization shows a reproducible gain on its target workload and no
  regression on the others. When no safe gain exists, record the profile and the
  decision instead of making a change.
- Long benchmarks stay manual. Normal tests may check deterministic allocation or
  scaling regressions, never wall-clock thresholds.

## Baseline: 0.4.0 plus the #63 fix (2026-10-25)

The library code is `main` at `ad23d65` (after #84). The runner came from the
commit that added it. Host: Apple M2 Pro (10 logical CPUs), macOS 26.6.2, AC power,
rustc 1.98.0, release profile, every feature. Each case ran one warm-up, five timed
runs and one counted run; the 180 s spectrograms ran once. No other builds or tests
ran, but the machine was not isolated. The full results are in
[`performance/baseline-0.4.0-native.csv`](performance/baseline-0.4.0-native.csv)
and, for WebAssembly under Node.js 24.22 WASI on the same host,
[`performance/baseline-0.4.0-wasm.csv`](performance/baseline-0.4.0-wasm.csv)
(quick scale, three runs).

Selected medians (stereo 48 kHz unless stated; "×rt" is audio seconds per wall
second):

| Case | 10 s input | 180 s input | Notes |
| --- | ---: | ---: | --- |
| WAV decode | 4.5 ms | 80 ms | Linear |
| FLAC decode | 33 ms | 613 ms | 234 / 4,218 reallocations; 73 GB requested in total at 180 s |
| MP3 decode | 47 ms | 859 ms | Geometric growth since #63 |
| FLAC encode | 47 ms | 844 ms | 48,291 allocations at 180 s |
| MP3 encode | 331 ms | 5,637 ms | 32×rt; 37,509 allocations at 180 s |
| Spectrogram 2048/512 (mono) | 129 ms | 31,904 ms | Quadratic: 247 times the time for 18 times the input |
| Spectrogram 1024/128 (mono) | 442 ms | 126,731 ms | 1.4×rt at 180 s |
| Loudness / true peak | 14 / 70 ms | 255 / 1,267 ms | Linear |
| Resample / tempo / pitch | 103 / 115 / 288 ms | 1.8 / 1.9 / 5.1 s | Linear |
| Disk FLAC: open, sustained read, seek to middle | 1.5 / 40 / 17 ms | 1.4 / 724 / 299 ms | Seeks replay from the start |
| Disk MP3: open, sustained read, seek to middle | 2.0 / 53 / 23 ms | 2.7 / 933 / 407 ms | MP3 seeks replay from the start (documented) |
| Asset load, miss / hit | 34 ms / 11 µs | 617 ms / 17 µs | The 180 s miss peaks at 103 MB: PCM plus the encoded file |

Callbacks of 256 frames at 48 kHz (deadline 5,333 µs), 10 s of output:

| Case | p50 | p99 | Per voice (p50) |
| --- | ---: | ---: | ---: |
| Empty mixer | 1.3 µs | 1.6 µs | — |
| 16 / 64 voices, matched rate | 25 / 98 µs | 29 / 135 µs | 1.5 µs |
| 64 voices resampled from 44.1 kHz / pitched | 371 / 388 µs | 515 / 489 µs | 5.8 µs |
| 16 voices through six bus effects and a look-ahead limiter | 144 µs | 183 µs | — |
| 16 voices with gain ramps every 4 callbacks | 75 µs | 107 µs | 4.7 µs |
| 32 3D emitters with Doppler, 5.1 output | 919 µs | 1,145 µs | 28.7 µs |
| 16 one-shot starts per callback, stealing at 32 voices | 375 µs | 513 µs | — |
| Chip score / drum score / synthwave score (synth `fill`) | 31 / 26 / 271 µs | 49 / 47 / 481 µs | — |

The 60 s mixer runs gave the same medians: spatial p50 916 µs, bursts 372 µs.
Single callbacks reached a few milliseconds when the OS preempted the process; a
p99.9 or maximum from this host is an observation, not a bound.

Every prepared callback allocated and freed nothing. Mixer preparation made
36,915 allocations (6.3 MB) with every feature, 4,147 (1.1 MB) without `music`,
and 33 (0.7 MB) under WebAssembly. Each queue slot holds a `std::sync::Mutex`,
which allocates when created on macOS and not with futexes or on WebAssembly. With
`music`, the return queue reserves `MAX_LAYERS + 1` returns per command (17 times
the default 1,024).

### Demonstrated bottlenecks, ranked

1. **Spectrogram rescans the input per column** (#79). Every column re-validates
   the entire remaining input and builds a new window and buffers. At 180 s that
   is 32 s and 127 s for one spectrogram.
2. **FLAC whole-file output grows exactly per frame** (#82). Reallocations and
   requested bytes grow with the square of the length. macOS mostly grows these
   blocks in place (118 MB moved at 180 s); #63 measured frequent relocation on
   Windows for the same pattern. The asset loader inherits this through `decode`.
3. **Disk seeks replay from the start** (#83). A seek to the middle of 180 s
   costs 299 ms for FLAC and 407 ms for MP3. Sustained reads keep up with
   in-memory decoding.
4. **Spatial voices cost 20 times a plain voice** (#80). Per frame they evaluate
   eight direction ramps, a norm, `level()` and, with Doppler, `exp2` for the
   speed.
5. **Synthwave voices** (#81) take 271 µs per callback at p50, 18×rt for one
   three-part score. Frequency, filter coefficients and LFOs are recomputed every
   sample.
6. **Starting and stealing voices** (#80): 16 starts per callback cost about
   320 µs above mixing their 32 voices.
7. **Encoders** (#82): MP3 encodes at 32×rt. FLAC encoding makes 48,291
   allocations at 180 s.
8. **Mixer preparation** (#80), outside callbacks: 37,000 allocations and 6.3 MB
   per mixer with `music` on macOS.

Each owning issue profiles its items before changing them. Items that yield no
safe improvement are recorded with their profiles.
