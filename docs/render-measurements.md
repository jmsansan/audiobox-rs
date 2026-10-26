# Numerical render measurements (0.5 development)

Engine #86 supplies the generic measurements requested by Studio's review tools.
These read the supplied PCM without changing samples, frame alignment or metadata.
Studio owns section ranges, thresholds, positioned findings, interpretation and
listening. A measured step is not proof of an audible click; an intentional drum
attack can produce the same number. No automatic repair or quality ranking occurs.

Existing `peak`, `true_peak_db`, `mean_square`, `rms`, `loudness` and spectra remain
available with their existing behavior. The new APIs live in the same `analyze`
module and `Audio` implementation, without a Studio DSP copy or new dependency.

```rust
use audiobox::{Audio, Result};
use audiobox::analyze::MeasurementBoundary;

fn inspect(audio: &Audio, section_join: usize) -> Result<()> {
    let dc = audio.dc_means()?;
    let silence = audio.threshold_silence(0.001)?;
    let joins = audio.measure_boundaries(&[
        MeasurementBoundary::Frame(section_join),
        MeasurementBoundary::LoopSeam,
    ])?;
    // Exactly two channels are required; no automatic downmix/channel selection.
    if audio.channels() == 2 {
        let stereo = audio.stereo_measurements()?;
        println!("{stereo:?}");
    }
    println!("{dc:?} {silence:?} {joins:?}");
    Ok(())
}
```

`analyze::dc_mean(&[f32])` and `analyze::stereo_measurements(left, right)` also
accept borrowed channel slices, allowing callers to measure selected ranges
without cloning PCM. The stereo slices must have equal lengths. For multi-channel
silence/joins, `Audio` methods preserve all channels; `Audio::cut` can explicitly
select a section, and returned indices then refer to that section's frame zero.
Map those indices back to the original file using the supplied section origin.

## Units and no-result states

| API/result | Definition |
| --- | --- |
| `dc_means()` | One signed arithmetic mean per channel in original order, in input-sample amplitude units. `None` per channel for zero frames; `Some(0)` for silence. `analyze::dc_mean` measures one channel with the same rules. |
| `threshold_silence(t)` | A frame is quiet iff **every** channel has `abs(sample) <= t`. `t` is finite, nonnegative, linear sample amplitude, not dBFS/RMS. Zero tests exact digital silence; values above one are allowed for audio with headroom. |
| `ThresholdSilence.active_range` | First through last frame exceeding the threshold on any channel, as a half-open range. Interior quiet frames are included. Empty/wholly quiet input gives `None`. |
| `leading_frames` / `trailing_frames` | Counts before/after that active range. Empty input has two zero counts. For wholly quiet input both equal the total frame count: the counts overlap and must not be added as disjoint durations. |
| `StereoMeasurements.correlation` | Mean-centered Pearson `sum((L-meanL)*(R-meanR)) / sqrt(sum((L-meanL)^2)*sum((R-meanR)^2))`, clamped to [-1,1] for rounding. Fewer than two frames, silence or either constant channel gives `None`. This differs from uncentered normalized cross-power. |
| `left_mean_square` / `right_mean_square` | Unweighted mean square in squared input-sample amplitude, retaining DC. `None` for empty input, `Some(0)` for silence. Same power concept as `Audio::mean_square`, with a separate accumulation order. |
| `mid_mean_square` / `side_mean_square` | Mean square of `M=(L+R)/2` and `S=(L-R)/2`, retaining DC; no sqrt(2) convention or loudness weighting. Empty gives `None`; silence gives `Some(0)`. In exact arithmetic `Lpower+Rpower=2*(Mpower+Spower)`. |

Identical varying stereo yields correlation +1 and zero side power; inverted
stereo yields -1 and zero mid power. A one-frame stereo signal has measurable
powers but no Pearson result. Constant nonzero channels have measurable DC/power
and no Pearson result. These statistics alone do not establish perceived width,
mono compatibility for all processing, musical balance or acceptable quality.

Unlike the existing window-RMS `detect_silence` / `trim_silence`, threshold silence
has no window, minimum duration, padding or automatic trim. Its strict active
comparison includes an adjacent-f64 threshold control in the tests.

## Explicit discontinuities

`MeasurementBoundary::Frame(f)` means the join from `f-1` to `f`, requiring
`0 < f < audio.frames()`. File edges have no invented zero neighbor and are
invalid frame boundaries. `LoopSeam` means the last existing frame to frame zero;
with zero or one frame its `BoundaryMeasurement.samples` is `None`.

Results retain request order and duplicates. A present `BoundarySamples` identifies
`before_frame`, `after_frame`, a `ChannelStep` per original channel and the maximum
absolute step over channels. Each channel reports `before`, `after` and signed
`difference=after-before`, in input-sample amplitude units. Values outside +/-1
retain their headroom. A zero step is a valid measurement, distinct from `None`.

This measures only two adjacent samples at an explicit join. It does not search
for transients, inspect a local window, compare slopes, infer a codec delay or
assign audibility. Review aligned final PCM and account for known source/codec
latency before selecting boundaries. Studio remains responsible for recording
the region, threshold and limitation alongside any finding.

## Validation, resources and numerical policy

Every sample used by these APIs must be finite. Stereo checks both entire slices;
the Audio silence/boundary calls check all channels, including active interiors
and calls with an empty boundary list. Invalid samples, thresholds, frame indices,
unequal stereo lengths or non-stereo Audio for a stereo call return
`InvalidArgument`. Audio methods also apply default `Limits` before output
allocation, including to Audio decoded with custom larger limits; excess input
shape returns `LimitExceeded`.

DC, sums of squares and covariance use compensated f64 accumulation in input
order; Pearson uses two passes and platform `sqrt`. Input f32 values are widened
before sums/differences/products, retaining finite results for finite extremes
and subnormals. Constant-channel detection explicitly prevents a spurious
correlation caused by roundoff. Existing analysis accumulation is unchanged.

Repeated measurements in the same executable return the same values. The native
and WASI tests compare an independent integer/rational reference within
`1e-12 * (1 + abs(reference))` on the seeded fixtures, alongside exact dyadic
controls. This is tested numerical accuracy, not a universal conditioning bound
or a promise of portable serialized report bytes. It does not expand or alter
the exact rendered-PCM, normalization or lossless-export contracts. Callers own
threshold tolerances and report serialization precision.

Valid slice DC/stereo and Audio threshold/stereo calls allocate no memory.
`dc_means` allocates one Vec of at most 64 channel options. Boundary calls permit
at most `analyze::MAX_MEASUREMENT_BOUNDARIES` (4,096) requests and 64 channels,
bounding output to 4,096 result records and 262,144 `ChannelStep` values. Output
Vecs reserve fallibly; oversize/allocation failure gives `LimitExceeded`. There
is one outer Vec and one channel Vec per present boundary, with no PCM clone or
per-frame output. All ranges/samples are validated before boundary allocation.

Time is O(samples) for DC/silence/stereo, and O(samples + requests * channels)
for boundaries. Scratch storage is O(1) beyond the bounded returned output. These
are offline scans, with no callback deadline guarantee. Existing default input
limits permit 512 MiB decoded PCM, six hours, 64 channels and 1000..=768000 Hz.

`tests/measurements.rs` covers DC/headroom/cancellation, exact threshold frames,
empty/silent/one-frame states, identical/inverted/orthogonal stereo, large DC,
finite extremes/subnormals, an explicit seam and an intentional drum attack,
invalid arguments, repeated results, custom decode limits, the maximum output
shape and allocation counts independent of PCM length. An internal defensive
test also checks nonfinite Audio state. The integration suite runs in native,
MSRV and WASI CI.

Print an example report for synthetic PCM, or supply a decoded input file:

```sh
cargo run --locked --offline --example render_measurements
cargo run --locked --offline --example render_measurements -- input.wav
```
