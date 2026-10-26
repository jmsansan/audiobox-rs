# Deterministic normalization and mastering exports

The 0.5 development branch implements #64 through the existing `Audio::normalize`
and `normalize_with_report` APIs. No new option or feature is needed. They prepare
their measurements using the crate's deterministic sine, cosine, power and log
series, then apply gain with the same deterministic DSP path used elsewhere.
The public report-only `loudness`, `true_peak_db`, `peak_db`, `rms_db`, spectra and
`analyze::to_db` retain their existing platform math and numeric behavior.

```rust
use audiobox::{Audio, NormalizeOptions, NormalizeReport, NormalizeUnit};

fn master(audio: &Audio) -> audiobox::Result<(Audio, NormalizeReport)> {
    audio.normalize_with_report(NormalizeOptions {
        to: -16.0,
        unit: NormalizeUnit::Lufs,
        peak_ceiling_db: -1.0,
    })
}
```

Use the returned Audio for a whole render. For LUFS/true-peak normalization, its
reported `gain_db` may also drive `gain_db` on consecutive chunks of the original
signal: the global measurement is taken once, and applying that gain gives the
same PCM as the whole normalization. Retain a single `PcmEncoder::with_dither`
across those chunks if the final integer export uses dither. Normalizing every
chunk independently measures different signals and is a different operation.

## Levels and boundaries

`to` and `peak_ceiling_db` are finite f64 values. Units of `to` and the reported
`measured` are dBFS for sample peak, dBTP for true peak and LUFS for integrated
loudness. `gain_db` is the gain actually selected, in decibels.

| Mode | Measurement and gain |
| --- | --- |
| Peak | Maximum absolute sample. PCM retains the existing target-linear/peak ratio. The report uses the deterministic logarithm; converting its dB gain back to linear need not reproduce that exact ratio. |
| TruePeak | Maximum of sample peak and four reconstructed phases, with the existing 32-tap-per-phase Blackman-windowed sinc and zero padding beyond the signal. Gain is target minus measurement. |
| Lufs | Existing K-weighted, overlapping-block integrated measurement. Gain is the smaller of target minus loudness and true-peak ceiling minus measured true peak. |

The true-peak kernel/scanning order is shared with the report-only measurement;
only its coefficient math and final logarithm change for rendering. This remains
a four-times oversampled estimate, with the existing edge behavior. LUFS's
`peak_ceiling_db` is used only in LUFS mode. It limits the global gain instead of
applying dynamic limiting. `limited_by_peak` is true only when requested gain is
strictly greater than the ceiling's gain; equality leaves it false. The separate
offline/mixer limiter still limits sample peaks; the true-peak limiter is #65.

The K-weighting shelf is at 1681.974450955533 Hz and the high-pass stage at
38.13547087602444 Hz. LUFS normalization requires at least 4000 Hz so that both
fit below Nyquist with room for stable coefficient preparation. Other modes retain
Audio's supported sample rates. Nonfinite target/ceiling values, lower LUFS rates,
filter overflow or gain/output overflow return `InvalidArgument`. Validation
precedes empty/silent-input handling, so an invalid LUFS rate is rejected there too.

The LUFS policy retains the previous algorithm:

- A block is `round(0.4 * sample_rate)` frames; the hop is
  `round(0.1 * sample_rate)`. Full blocks are used, leaving an incomplete final
  block out of integrated gating.
- Input shorter than one block uses its entire available weighted power without
  gating. This includes very quiet short sounds. Empty input has negative-infinite
  loudness. This short-input policy is retained for compatibility.
- Full-block absolute gating admits levels **>= -70 LUFS**. The relative gate is
  the absolute survivors' mean-power loudness minus 10 LU, again inclusive.
  All these decisions use unrounded deterministic f64 levels.
- If no block survives, `measured` is negative infinity, `gain_db` is zero,
  `limited_by_peak` is false and Audio is returned unchanged. Exact silence follows
  the same rule in every mode. No gain is invented for an unmeasurable signal.
- Existing channel weights stay the same: quad uses 1/1/1.41/1.41, five channels
  1/1/1/1.41/1.41, and six channels 1/1/1/0/1.41/1.41. Other channel counts use unit
  weights. This does not infer speaker layouts from a channel count or a WAV mask.
  An LFE-only six-channel signal is unmeasurable in LUFS and is returned unchanged.
- There is no additional maximum-boost option. LUFS's ceiling bounds its gain;
  peak/true-peak modes use their requested target. The existing f32 overflow checks
  reject a gain that cannot be rendered finitely.

`NormalizeReport` retains f64 precision with no decimal rounding before decisions.
Its measurement is the input level, not a fresh measurement of the returned Audio.
F32 output rounding can move a post-render measurement by a few ULPs; the regression
ceiling tolerance is 0.00001 dB against the existing report-only estimator. The
legacy block-percentile loudness range remains an approximation, not an EBU
Tech 3342 certification. Public report-only measurements may differ in their last
bits between hosts; feeding those numbers into a custom gain decision does not
use this normalization contract.

## Export audit

| Stage | Contract |
| --- | --- |
| Normalization PCM and report | Same samples/values for identical Audio and options on supported native/WASM targets; all units use deterministic measurement/control math. Peak PCM is unchanged from 0.4.1. |
| Gain, fades, filtering, resampling, sample-peak limiting | Existing deterministic PCM contract. Any upstream host-dependent analysis that chooses their settings remains the caller's responsibility. |
| Raw integer PCM, WAV, FLAC | Same destination integers/bytes for identical PCM and format/options/tags. Seeded dither is added once at the final quantizer; FLAC's predictor, Rice coding, CRC and MD5 then operate on integers. |
| MP3 encode/decode | Exact encoded/decoded checksums are tested for the named mastering fixtures. Its transform tables and quantizer retain platform transcendental functions. A general arbitrary-input exact-byte guarantee is not established here; [#100](https://github.com/jmsansan/audiobox-rs/issues/100) tracks the validation/contract follow-up. |
| Public loudness/true-peak/spectral reports | Existing numeric/reporting API, using platform math. Normalization reuses the algorithms with deterministic preparation/logarithms rather than rounding those public reports. |

The tests compare normalized PCM first, encoded bytes second and decoded PCM third
so a container/codec difference cannot be mistaken for a normalization difference.
No MP3 encoded-byte divergence was reproduced by the Windows/WASI fixtures.
The MP3 implementation is unchanged. #100 calls for broader adversarial/bitrate
comparisons and requires an actual reproducer before any proposed codec fix.
Studio can select LUFS directly through this API and use WAV/FLAC when its export
policy requires a general exact-byte promise. Independent-decoder MP3 gapless
timing remains tracked separately in #95.

## Verification and resources

`tests/normalization.rs` compares measurements with the existing report-only
implementation at 4/8/32/44.1/48/96/192/768 kHz, with mono through eight-channel,
DC, quiet and short controls. Agreement must be within 0.0001 dB/LU and the
resulting PCM within 0.00001 amplitude units. Empty/silent input, ignored LFE,
short-input fallback, low rates and overflow have separate tests.

The gates have unit tests with adjacent f64 powers bracketing -70 LUFS, exact
equality, and values around the relative gate. A separate native/WASI checksum
pins PCM/reports for signals at -70 +/- 0.001 LUFS, gain requests within 1e-9 dB
of the ceiling transition, and a two-level relative-gate control.

Six half-second stereo masters at 32/44.1/48 kHz use rational/seeded input, both
LUFS and true-peak normalization, +24 dB gain and active sample-peak limiting.
They pin six normalized PCM hashes, six limited-master PCM hashes and reports,
eighteen tagged WAV/FLAC/MP3 encoded hashes
and eighteen decoded hashes independently. WAV/FLAC use TPDF seed 7; MP3 leaves
dither disabled. Identical fixtures run on Linux/macOS/Windows and Node WASI in CI,
and the minimum compiler remains Rust 1.85. Global gain/quantization tests compare
chunks of 1, 7, 256, 4096 and whole-input frames.

These are offline, whole-signal operations. They reuse the existing true-peak
scan and LUFS filtering/gating workspaces; memory is proportional to the accepted
Audio and block count. No realtime or allocation-free normalization API is added.
The existing benchmark runner adds `dsp/normalize-lufs` and
`dsp/normalize-true-peak` cases; fixture preparation and output hashes remain
outside the timed processing phase:

```text
cargo bench --bench engine --all-features -- dsp/normalize-lufs/10s dsp/normalize-true-peak/10s --quick --runs 5
```

## Local validation (2026-10-26)

Windows x86_64, AMD Ryzen 7 1700X, Rust 1.98.1 release, all features; one warm-up,
five timed runs and a separate allocation-counted run. The 10-second stereo
48 kHz fixture is prepared before timing. Raw results are in
[performance/normalization-windows.csv](performance/normalization-windows.csv).

| Operation | Median | Allocations | Additional peak live bytes during the phase |
| --- | ---: | ---: | ---: |
| Normalize to -16 LUFS, -1 dBTP ceiling | 55.59 ms | 13 | 5,760,048 |
| Normalize to -1 dBTP | 34.69 ms | 3 | 3,840,048 |

These observations exclude the retained input fixture and are one host/run rather
than a universal throughput claim. No unrelated tests ran during measurement.
The complete native debug/release suites, the normalization fixtures on Rust 1.85
and Node 24 WASI, Clippy, rustdoc and package verification passed locally.