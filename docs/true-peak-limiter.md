# Offline true-peak limiting

The 0.5 development branch adds `Audio::limit_true_peak` for #65. It uses
`TruePeakLimiterOptions` and is independent of the existing sample-peak
`Audio::limit` / `LimiterOptions`; their settings and sample bits stay unchanged.
The new path needs no feature flag, dependency or unsafe code.

```rust
use audiobox::{Audio, TruePeakLimiterOptions};

fn limit_master(master: &Audio) -> audiobox::Result<Audio> {
    master.limit_true_peak(TruePeakLimiterOptions {
        ceiling_db: -1.0,
        lookahead_ms: 5.0,
        release_ms: 50.0,
        safety_margin_db: 0.3,
        max_working_bytes: 1 << 30,
    })
}
```

## Signal path and ceiling

The detector reconstructs eight positions per input frame with a 64-tap,
phase-normalized Blackman-windowed sinc. Phase zero includes the actual sample
exactly. For phase `p/8`, each offset `j = -31..32` uses:

```text
t = j - p/8
sinc(t) = sin(pi*t)/(pi*t)
window(t) = 0.42 + 0.5*cos(2*pi*t/64) + 0.08*cos(2*pi*t/32)
kernel[p,j] = sinc(t)*window(t) / sum_j(sinc(t)*window(t))
```

Phase zero bypasses this formula; phases 1..7 use the crate's deterministic
trigonometry. Input is zero outside its PCM range. Reconstruction includes the
32-frame zero-extended regions before and after the clip. The outside maxima
participate in the first/last detector frames as well as the final output check.

At each frame the detector takes the maximum absolute value across phases and
**all channels**. A monotonic queue retains the largest peak from the current
frame through the configured lookahead. The target gain is `min(1, C/peak)`, or
one for zero peak; attack is immediate. When gain can recover, it follows
`target + (previous-target)*exp(-1/(release_ms*rate/1000))`. Gain is linked across
channels, cannot boost above one, and multiplies each input sample in f64 before
rounding once to f32.

`C` is the nominal ceiling minus `safety_margin_db`, converted deterministically
to linear amplitude, with a further linear reserve of `8*f32::EPSILON` (about
0.0000083 dB) for final f32 rounding. No interpolation/downsampling filter is
applied to the returned PCM: oversampling is used for detection/reconstruction.

Changing gain can create a new reconstructed maximum. After the dynamic pass,
the engine checks the complete output with both this 8x/64-tap reconstruction and
the existing deterministic 4x/32-tap-per-phase true-peak estimator from #64. If
necessary it applies one additional **uniform attenuation** to bring that maximum
back to `C`. This guard is part of the operation and may reduce the whole clip
further, especially with short lookahead or very fast release. It does not
re-run loudness normalization or claim to maximize loudness.

The returned-f32 ceiling (nominal minus the selected margin) is checked with
0.0001 dB tolerance under the
engine's existing 4x estimator; the independent controls below allow 0.05 dB
against the nominal ceiling with default settings. The explicit default margin
is 0.3 dB because different reconstruction windows/rates measure different peaks.
A zero margin is supported for callers selecting that tradeoff, with the internal
rounding reserve still applied. These finite interpolators do not prove a bound
on every possible continuous-time reconstruction.

## Duration, alignment and tails

Output preserves the exact sample rate, channel count, frame count and leading
sample alignment. This is a complete-buffer offline operation: lookahead and the
detector's FIR context are read from the available future input, so no delay is
prepended or trimmed from output. A 5 ms setting rounds to `round(rate*0.005)`
source frames; lookahead is clamped to the clip length. The detector additionally
needs up to 32 future source frames for reconstruction, independently of that
control window.

All existing PCM, including an authored decay or quiet tail, is processed. Release
gain continues while those frames exist, then ends at the input boundary. No
release frames, FIR audio tail, extra leading zeros or device latency are added.
The reconstructed zero-extension response is checked but is not appended to the
returned Audio. A quiet signal whose reconstructed peak needs no attenuation
retains its sample bits; silence and empty input keep their length.

The result retains Audio's metadata through the same immutable-DSP convention as
other transforms. Apply this after gain/EQ or other peak-changing DSP and before
the final integer export/dither. Subsequent processing, quantization and lossy
encoding can change measured peaks; this method's ceiling describes its returned
float PCM. The broader MP3 exact-byte contract remains in #100.

## Validation and resource policy

| Option | Accepted values / default |
| --- | --- |
| `ceiling_db` | Finite nominal dBTP; the linear ceiling after margin must be positive and finite. Default -1. |
| `lookahead_ms` | Finite and nonnegative, rounded and clamped to input frames. Default 5. |
| `release_ms` | Finite and positive exponential time constant. Default 50; extremely large values can round the coefficient to one. |
| `safety_margin_db` | Finite, 0..=12 dB. Default 0.3. |
| `max_working_bytes` | Requested workspace payload budget in bytes. Default 1 GiB. |

Invalid numeric settings or an unrepresentable linear ceiling return
`InvalidArgument`, including for empty input. Size overflow, insufficient workspace
or a failed fallible reservation return `LimitExceeded` before rendering.

The workspace check includes output PCM, its channel-vector descriptors, the
64x7 f64 kernel, and `min(frames, lookahead_frames+1)` peak-queue entries. It
excludes the caller's input, copied metadata, allocator bookkeeping and other
fixed call-stack state. Integer size arithmetic is checked before heap allocation;
buffers use fallible reservations. The queue is reserved once and expires old
entries before inserting new ones, including monotonically decreasing peaks.
No complete oversampled audio buffer or input-length peak array is allocated.

Work is linear in frames/channels with fixed reconstruction factors. The dynamic
pass uses one input reconstruction, and the final guard scans the limited output
at 8x plus the existing 4x estimator. The additional edge scans have constant
length. This API allocates and performs whole-output checks; it is not a realtime
callback limiter, a new default mixer sound, or a streaming API.

## Independent verification

`tests/true_peak.rs` uses a separately written direct reconstruction: **16 phases,
96 taps, Kaiser beta 9**, platform sine/Bessel/window math and explicit zero
padding across the full finite filter response. It shares no engine detector or
resampler code. The actual source samples remain part of its maximum. This
independent filter differs in phase density, window and length from production.

Synthetic controls include a two-samples-up/two-down waveform whose sample-limited
output is below -1 dBFS but whose independent reconstructed peak exceeds -0.5 dBTP.
The new limiter must measure at most -0.95 dBTP independently and -0.9999 dBTP with
the existing engine estimator at 1/8/32/44.1/48/96/192/768 kHz. Other controls cover
end impulses, DC, bounded noise, bursts, 0.45/0.49-cycle-per-frame tones, fast release,
stereo gain ratios, empty/short/subnormal/extreme samples and workspace rejection.
They check unchanged duration and impulse position, including complete end treatment.

The optional interoperability test reconstructs padded float-WAV controls using
FFmpeg's independent **swr, 16x, filter_size 128, Kaiser beta 9, cutoff 1**, with
an exact rational phase count. The old sample limiter must fail the intersample
ceiling control; the new path must meet the same -0.95 dBTP independent tolerance.
The filter options follow the [FFmpeg resampler documentation](https://ffmpeg.org/ffmpeg-resampler.html).
This runs in the existing release/manual-dispatch FFmpeg job, without adding
FFmpeg to routine push/PR requirements or to crate dependencies.

Fourteen f32 output hashes cover mono/stereo/quad/5.1/7.1, seven sample rates and
0/5 ms lookahead. They must match on Linux, macOS, Windows and Node WASI; Rust 1.85
remains the minimum. Existing sample-peak limiter and normalization hashes are
unchanged. These tests describe their measured tolerances and fixtures, not a
universal reconstruction or listening-quality certification.

## Example and benchmark

```text
cargo run --example true_peak_limiter -- target/true-peak-limiter
cargo bench --bench engine --all-features -- dsp/limit/10s dsp/limit-true-peak/10s --quick --runs 5
```

The example writes source, sample-limited and true-peak-limited float WAVs from a
synthetic 12 kHz control with level changes, reporting frame count and the existing
sample/4x peak measurements. The benchmark reuses the established 10-second stereo
fixture, boosted by 3x during preparation for the new case so reduction is active.
It separates generation and checksum work from the timed transform and
records allocations/peak workspace in a separate run.

## Local validation (2026-10-26)

Windows x86_64, AMD Ryzen 7 1700X, Rust 1.98.1 release, all features; one warm-up,
five timed runs and a separate allocation-counted run, without concurrent tests.
Raw results: [performance/true-peak-windows.csv](https://github.com/jmsansan/audiobox-rs/blob/v0.5.1/docs/performance/true-peak-windows.csv).
The sample-peak baseline retains the existing benchmark fixture; the new case
boosts that fixture by 3x before timing to exercise reduction.

| 10-second stereo 48 kHz operation | Median | Allocations | Reallocations | Additional peak heap bytes |
| --- | ---: | ---: | ---: | ---: |
| Existing sample-peak limiter, baseline fixture | 15.66 ms | 4 | 5 | 3,842,096 |
| True-peak limiter, boosted fixture | 555.83 ms | 4 | 0 | 3,843,904 |

The new path's requested buffer budget in that configuration is 3,847,488 bytes,
including its 3,584-byte stack kernel. The measurement excludes the already retained
input and is one host/run, not a universal CPU or realtime guarantee.

The example produced 96,000 frames for all three outputs. Its existing 4x estimator
reported +2.110 dBTP for the sample-limited control and -1.300 dBTP for the new path;
this is the declared synthetic waveform, not a listening-quality evaluation.
Complete native debug/release suites, Rust 1.85 output checks, WASI hashes, Clippy,
rustdoc, example execution and package verification passed locally. The independent
FFmpeg reconstruction is verified by the release/manual-dispatch CI job.