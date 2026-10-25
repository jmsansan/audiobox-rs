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
- `checksum`: a hash of the case's whole output, including the frames after a
  seek, which must also equal the frames delivered at that position. `--compare`
  flags changed outputs, so an optimization that changes samples cannot pass
  unnoticed.

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
| Disk FLAC: open, sustained read, seek to middle | 1.4 / 38 / 17 ms | 1.5 / 764 / 308 ms | Seeks replay from the start |
| Disk MP3: open, sustained read, seek to middle | 1.9 / 50 / 23 ms | 2.7 / 998 / 435 ms | MP3 seeks replay from the start (documented) |
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
   costs 308 ms for FLAC and 435 ms for MP3. Sustained reads keep up with
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

## 0.4.1 changes

Each entry compares the change with `main` before it, on the baseline host with the
same runner. `--compare` reported identical output checksums for every case listed.
Other workloads on the host (an iOS simulator) interfered with some later runs.
Where they did, the tables use rounds without interference and per-callback
medians, and say so.

### Analysis and DSP (#79)

`analyze::spectrogram` called `spectrum` on every column's suffix. Each call
re-checked every remaining sample for finiteness, which is quadratic in the input,
and built a new window and buffers. The spectrogram now checks the input once,
after the same hop, FFT size and output-limit checks as before. It then transforms
every column with one window and one pair of buffers, through the same arithmetic
`spectrum` uses. `tests/analysis.rs` compares every column with `spectrum` of its
suffix bit for bit, across empty, short, silent and noisy inputs and hops from 1
to longer than the input. It checks that a non-finite sample far past the first
window still fails and that the errors keep their order. It also counts allocations
per column (old code: four, now one) and runs a million-column spectrogram that the
old code would take hours to finish.

`Audio::true_peak_db` evaluated a 32-tap kernel for each of four phases, with a
bounds check per tap. Away from the edges, every tap is in range, so the four sums
now run side by side over the same samples, each still adding its terms in the
original order. The edges keep the old loop. A test compares the result bit for
bit with the 0.4.0 algorithm at lengths around the 16-sample edges.

| Case | Before | After | Ratio |
| --- | ---: | ---: | ---: |
| Spectrogram 2048/512, 10 s | 138 ms | 30.5 ms | 0.22 |
| Spectrogram 1024/128, 10 s | 455 ms | 55.1 ms | 0.12 |
| Spectrogram 2048/512, 180 s | 31,904 ms (baseline, one run) | 521 ms | 0.016 |
| Spectrogram 1024/128, 180 s | 126,731 ms (baseline, one run) | 945 ms | 0.0075 |
| True peak, 10 s / 180 s | 72.9 / 1,309 ms | 32.8 / 592 ms | 0.45 |
| Loudness, filter, limit, resample, tempo, pitch (controls) | | | 0.96–1.08 |

Allocations fell from 3,758 to 944 and from 15,006 to 3,756 at 10 s: one
magnitude vector per column, which is the required output. Peak memory is
unchanged, because the output dominates it.

Loudness, the offline filters and limiter, resampling, tempo and pitch are
unchanged. Sampled profiles (macOS `sample`, 180 s inputs) put 99% of resampling in
`sinc_sample`, the 64 or more interpolated taps per output sample. Tempo spends all
of its time in the WSOLA search and overlap-add loop. Pitch splits about 76/24
between resampling and tempo. The determinism contract fixes that arithmetic and
its order, and these transforms allocate only their output and a few working
buffers. Loudness and the filters already run at 700–1,700×rt.

### In-memory codecs (#82)

Whole-file FLAC decoding called `try_reserve_exact(n)` per frame and channel, as
MP3 did before #63, so reallocations and requested bytes grew with the square of
the length. It now uses the same fallible geometric growth as MP3, moved into
`Limits::reserve_output`. Speculative capacity stops at the decode limits and, when
STREAMINFO declares a total, at that total. More samples than declared were already
an error, so a declared total is a hard bound, and such streams end with exactly
sized channels: peak memory does not change. A declared total is never reserved up
front, so a false one cannot force a large allocation. The asset loader decodes
through the same function.

`tests/flac_output_alloc.rs` checks the reallocation count over 30 s, exact capacity
with a declared total, geometric growth within limits without one, frame-limit
boundaries, false declared totals (too large: still a decode error, without a large
allocation; too small: still an error), injected allocation failure (`LimitExceeded`)
and bit identity with the incremental decoder. The two growth tests fail on the old
code.

| Case | Reallocations | Requested bytes | Moved (estimate) | Peak live | Time |
| --- | ---: | ---: | ---: | ---: | ---: |
| FLAC decode 10 s, before / after | 234 / 24 | 230 MB / 15.2 MB | 6.5 / 0.6 MB | 3.9 / 3.9 MB | 33.6 / 34.0 ms |
| FLAC decode 180 s, before / after | 4,218 / 38 | 73.0 GB / 263 MB | 243 / 59 MB | 69.2 / 69.2 MB | 621 / 619 ms |
| Asset load (FLAC) 180 s, before / after | 4,238 / 58 | 73.0 GB / 330 MB | 271 / 68 MB | 102.8 / 102.8 MB | 641 / 656 ms |

On this macOS allocator, large blocks mostly grow in place, so wall time does not
change: the removed copying shows up on allocators that relocate, as #63 measured
on Windows. MP3 decode, the encoders, WAV, AIFF and streaming decode are controls.
Their outputs and allocation counts are unchanged.

The encoders are unchanged. Sampled profiles at 180 s:

- **FLAC:** the predictor trials and residual sizing in `encode` and `sub_writer`
  take most of the time, MD5 14%, and memory copies about 2%. Its 48,291
  allocations (one residual vector per trial) are not where the time goes.
- **MP3:** quantization takes 60% and the platform `pow` inside it 20%, with the
  analysis transform at 15%.

Making these faster means restructuring or reordering their arithmetic, while
keeping the encoded bytes identical. That is a follow-up candidate, not a 0.4.1
patch.

### Mixer callbacks (#80)

A sampled profile of 32 Doppler emitters on 5.1 spent 20% of `Voice::render` in
`memmove`, copying the eight direction ramps per frame
(`self.direction.map(...)`). Ramp evaluations took 17% and `update_speed` 8%, which
recomputes `exp2(pitch / 12)` every frame while Doppler changes the speed. The
burst case's profile put admission (`Mixer::start`) at about 1.5%. Its time went to
rendering voices in the extra segments that scheduled starts create, each of which
calls `update_speed` and `gains` again.

Three changes, each computing the same values from the same inputs:

- The voice keeps the last pitch and its `exp2(pitch / 12)` and reuses the factor
  while the pitch is unchanged.
- Direction gains are evaluated for the output channels only, in place, without
  copying the ramps.
- When an emitter's level and direction ramps hold for a segment (none changing
  at its start), the gains, norm and level are computed once for the segment.
  Every sample still evaluates `value * gain / norm` as before.

The existing spatial checksums (native and WASI) are unchanged. A new test renders
held and moving emitters with a pitch ramp during Doppler, gain ramps and a pause
in blocks of 1 to 2,400 frames, pinned to the hash `main` produced before the change.

Medians of individual 256-frame callbacks (deadline 5,333 µs) over three
alternating rounds of base and change, 10 s each. Wall-time medians of some rounds
met interference from other workloads on the host; per-callback medians did not
move between rounds.

| Case | Before p50 | After p50 | Ratio |
| --- | ---: | ---: | ---: |
| 32 Doppler emitters, 5.1 | 921 µs | 310 µs | 0.34 |
| 16 starts per callback, stealing | 371 µs | 327 µs | 0.88 |
| 64 pitched voices | 399 µs | 386 µs | 0.97 |
| 16 / 64 voices, matched rate | 25.2 / 98.3 µs | 22.9 / 88.0 µs | 0.91 / 0.90 |
| 16 voices with gain automation | 75.8 µs | 73.3 µs | 0.97 |
| 64 resampled voices, 16 through effects, synth stream, empty | | | 0.98–1.01 |

The matched-rate cases do not run the changed code. Their 9–10% gain, steady
across rounds, comes from code generation around the reorganized loop and is not
promised. Callbacks still allocate and free nothing.

Unchanged, with reasons:

- **Resampled and pitched voices:** their 16-tap interpolation is fixed by the
  determinism contract.
- **Effects:** six effects and the look-ahead limiter add about 120 µs per
  callback on 16 voices. A sampled profile puts 63% of that case in the chorus and
  phaser (`colour::Processor`), which evaluate their LFOs, modulated delay reads
  and all-pass coefficients for every sample. The LFO phase advances every sample,
  so no result repeats. The limiter, delay, reverb, compressor and filters take
  about 11% together.
- **Mixer preparation:** 37,000 allocations and 6.3 MB with `music` on macOS. It
  runs off the audio thread, once per mixer. Changing it means a different queue
  slot design or return sizing, which is not a patch-level change.

### Synth and SFX (#81)

A sampled profile of the synthwave score spent about 70% in `WideVoice::sample` and
17% in `Oscillator::next`. Per sample and voice, `WideVoice::sample` converts the
pitch to a frequency (`exp2`) and the filter cutoff to a coefficient (`exp2`, a sine
and a cosine). Each LFO computes its phase with a 128-bit multiply and remainder
(`__umodti3` showed in the profile). Many of these inputs hold still for most of a
note: the pitch without glide or a pitch LFO, and the cutoff after its envelope
settles and without a cutoff LFO. Three changes, each producing the same values:

- Each voice keeps the last pitch input and its frequency, and reuses the frequency
  while the input is unchanged (compared by bits).
- Each voice keeps the last filter inputs (cutoff cents, base cutoff, rate) and
  their coefficient, and reuses it while all three are unchanged.
- An LFO computes its integer phase in 64 bits when the product fits, which gives
  the same integer as the 128-bit form. Reaching the 128-bit fallback would take
  about 10¹⁵ samples.

Existing synthwave, synth, chip and drum checksums are unchanged, natively and on
WASI. A new test renders static, gliding and LFO-modulated voices, a filter
envelope and a returning pitch, pinned to the hash `main` produced before the
change.

The runner gained two cases to separate unmodulated from modulated sound: one
unison pad with a fixed filter, and the same pad with pitch and cutoff LFOs. Medians
of individual 256-frame callbacks over three alternating rounds of base and change,
10 s each. Callbacks still allocate nothing.

| Case | Before p50 | After p50 | Ratio |
| --- | ---: | ---: | ---: |
| Unison pad, fixed filter (no modulation) | 136 µs | 28.0 µs | 0.21 |
| Synthwave score (bass, pad with cutoff LFO, lead with pitch LFO) | 264 µs | 206 µs | 0.78 |
| Same pad with pitch and cutoff LFOs | 203 µs | 201 µs | 0.99 |
| Chip score / drum kit / SFX presets (controls) | 29.8 / 25.8 µs / 36 ms | 29.8 / 26.2 µs / 37 ms | 1.00 |

Modulated parameters change every sample and are still computed every sample: the
LFO pad gains only the cheaper phase. Preparation (`parse`, `prepare`) costs under
0.1 ms for these scores. The chip, drum and SFX renderers are unchanged. Sampled
profiles of the chip score split it between voice sampling, the per-frame
scheduling in `fill` and the oscillators, about a third each. The drum kit spends
62% in `DrumVoice::sample`, and the presets nearly all in `Effect::render_with`.
Legacy voices already reuse their frequency when nothing modulates it. These run at
170–260×rt.

### File streaming and assets (#83)

Measured in the baseline: opening a disk stream takes 1.4–2.7 ms. The first frames
are available in about 2 µs, because opening prefills the PCM ring. Sustained reads
keep up with in-memory decoding (FLAC 249×rt, MP3 189×rt at 10 s). A cache hit in
the asset loader takes 11–17 µs. Twenty rounds of 64 lookups and inserts against a
budget of a third of the set, each miss inserting a fresh copy, take about 3 ms.
The costs that grow are seeks (308 ms for FLAC and 435 ms for MP3 to the middle of
180 s), because both disk decoders replay from the start, and a cache miss, which
is a whole decode.

A sampled profile of repeated disk FLAC seeks put a quarter of the decoding work in
`Md5::update`. Disk FLAC keeps its MD5 running through a replay, so that reading on
to the end still verifies the whole file, as it did before. The rest went to
`Bits::read` (most), frame and subframe decoding, and residuals. The bit reader
read one bit at a time, and unary codes called it once per bit. The changes:

- `Bits::read` and `Bits::peek` take up to 32 bits from an 8-byte big-endian load
  and a shift. They return the same values, keep the same bounds checks, and give
  zeros past the end for `peek`. MP3 decoding uses the same reader.
- FLAC unary codes count leading zeros in 32-bit windows. They return the same
  counts and the same errors, in the same order: truncated data before the one,
  or more than 2^24 zeros.
- MD5 hashes whole 64-byte blocks straight from the input and runs its 64 steps
  unrolled with fixed rotations and indices. The PCM bytes it hashes are packed
  with one slice copy per sample instead of one push per byte.

Unit tests compare `read` and `peek` with the 0.4.0 bit-at-a-time reader on random
data and widths, including every end-of-data case. They compare the unary decoder
with the old one, including the 2^24 limit and truncation. They also check MD5
against the RFC 1321 suite, fed in every split from 1 to 130 bytes. FLAC files keep
their MD5 checks, and the FFmpeg interoperability tests pass.

Seeking itself is unchanged in design. A sync-code bisection or index would skip
frames without decoding them, but it would change which corrupt-file errors a seek
reports and drop the MD5 check after a seek. #83 rules out a new seek architecture
or a public index for this release. When a file carries a SEEKTABLE, the in-memory
decoder already uses it; this crate's encoder writes none.

Also observed and left as is: when a FLAC frame straddles the end of the disk
window, the reader fails to decode it, refills the window and decodes it again.
That is one 19-byte error message on the reader thread per boundary (22 over
180 s), never on the audio thread.

Medians over three alternating rounds of base and change (three runs each):

| Case | Before | After | Ratio |
| --- | ---: | ---: | ---: |
| Disk FLAC seek to the middle, 10 s / 180 s | 17.3 / 300 ms | 11.8 / 201 ms | 0.68 / 0.67 |
| Disk FLAC sustained read, 10 s / 180 s | 38.2 / 758 ms | 27.5 / 509 ms | 0.73 / 0.71 |
| FLAC decode, 10 s / 180 s | 33.1 / 607 ms | 22.3 / 412 ms | 0.68 |
| FLAC streaming decode, 180 s | 595 ms | 400 ms | 0.67 |
| Asset load miss (FLAC), 10 s / 180 s | 33.5 / 614 ms | 22.5 / 420 ms | 0.67 / 0.68 |
| FLAC encode, 10 s / 180 s (MD5) | 47.0 / 846 ms | 42.3 / 771 ms | 0.91 |
| MP3 decode, streaming decode, disk read and seek (bit reader) | | | 0.94–0.98 |

Outputs and allocation counts are unchanged in every case.
