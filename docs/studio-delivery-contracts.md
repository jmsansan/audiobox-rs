# Analysis and file contracts for Studio (0.5)

This covers engine issues #97, #99, #102, #103 and #104. Studio owns musical
regions, project containment, target thresholds and delivery decisions. The engine
owns these generic measurements, input limits and resampling rules. These APIs
are available starting with audiobox 0.5.0.

## Measuring the final PCM

```rust
use audiobox::{Audio, Result};

fn meets_ceiling(final_pcm: &Audio, ceiling_db: f64) -> Result<bool> {
    let measured = final_pcm.measure_deterministic()?;
    Ok(measured.true_peak_db <= ceiling_db)
}
```

`Audio::measure_deterministic` returns `analyze::DeterministicMeasurements`:

| Field | Unit and meaning |
| --- | --- |
| `peak_db` | Maximum absolute input sample, in dBFS. |
| `true_peak_db` | Existing deterministic normalization 4x reconstructed estimate, in dBTP. |
| `loudness.integrated` | Existing deterministic normalization K-weighted gated integrated LUFS. |
| `loudness.range` | Legacy gated 400-ms block percentile range, in LU. |
| `loudness.peak_momentary` | Maximum block level in LUFS, or whole-input short fallback. |

The call measures the supplied PCM as it stands, including any limiter, resampler
or decoded quantization already applied by the caller. It does not normalize,
change gain, clip, quantize, clone PCM or alter metadata. Measure the decoded final
file if the decision concerns that file. `NormalizeReport.measured` continues to
describe normalization's **input**, not its returned or subsequently processed PCM.

All returned fields use the normalization path's deterministic coefficients,
accumulation and logarithms with unrounded f64 precision. They may drive repeatable
target/ceiling comparisons on supported native and WASM targets. There is no hidden
decimal rounding or acceptance tolerance: callers choose their own threshold and
tolerance policy. A threshold equal to the result passes `<=`; an adjacent lower
f64 threshold fails. Identical values across hosts do not establish physical
measurement accuracy or a universal continuous-time peak bound.

The four-times estimator includes sample peak and four 32-tap-per-phase Blackman
sinc sums at each input frame, omitting taps outside the signal as zero. It retains
the existing scan boundary behavior. It is shared with normalization and the
limiter's final 4x guard, but differs from the limiter's 8x/64-tap detector. The
limiter's safety margin helps accommodate different finite reconstructions; see
[the limiter contract](true-peak-limiter.md).

The loudness policies are those in [normalization](normalization.md):

- Require at least 4000 Hz for K-weighting, including empty/silent input.
- Full 400-ms blocks, 100-ms hops, inclusive -70 LUFS absolute gate, inclusive
  relative gate at the absolute survivors' mean-power level minus 10 LU.
- Short input uses its entire ungated weighted power. There are no synthetic
  padding frames or invented minimum duration.
- Empty, silent and wholly gated input return negative infinity for integrated
  and peak-momentary loudness and zero range. Sample/true peak are independently
  measured, so a gated quiet signal may still have finite peaks.
- Quad weights: 1/1/1.41/1.41; five-channel: 1/1/1/1.41/1.41;
  six-channel: 1/1/1/0/1.41/1.41; other counts: all ones. An LFE-only six-channel
  signal is unavailable for loudness but has measurable peaks. Channel count
  does not resolve speaker layout.
- `range` remains a block-percentile estimate, not certified EBU Tech 3342 LRA.

Invalid rate, nonfinite input or filter overflow returns `InvalidArgument`. Default
`Limits` bound the input shape **before** workspace allocation, including Audio
created by a decoder with larger custom limits; excess is `LimitExceeded`.
Defaults permit 512 MiB PCM, six hours, 64 channels and 1000..=768000 Hz (the
loudness minimum tightens that to 4000 Hz). The operation is offline: time and
storage grow with samples/block count. It reuses normalization's filtered-channel
and gating workspaces, approximately one filtered PCM copy plus one temporary
channel and four f64 arrays of block count; it is not allocation-free. Allocation
behavior is the existing normalization implementation, with its ordinary Vec
allocations. No additional per-call workspace-budget option is introduced.

Existing `loudness`, `true_peak_db`, `peak_db`, `rms_db`, spectra and `to_db` retain
their platform math and reporting behavior. The explicit deterministic API is the
supported choice when a final delivery decision must agree across hosts.

## Linear RMS and aggregation

`Audio::mean_square()` returns `sum(sample^2) / (frames * channels)` in squared
input-amplitude units. `Audio::rms()` is its square root in input-amplitude units.
All channels have equal weight. Finite values above full scale retain headroom;
there is no clamping, K-weighting or logarithm. Empty and silent PCM return zero.
The sample denominator is the existing `frames() * channels()`; the frame
denominator for channel-averaged power is `frames()`.

For unequal regions, combine power with its sample denominator. For powers p1/p2
and sample counts n1/n2, combined power is `(p1*n1 + p2*n2)/(n1+n2)` if the total
count is nonzero. For zero extension to a common frame count M at an unchanged
channel count, multiply the original mean square by `frames/M`. Handle M=0
explicitly. This is bookkeeping over isolated powers; correlation/cancellation
means their shares do not prove their contribution to the mixed signal's power.

The methods reuse the former `rms_db` planar f64 square accumulation. `rms_db`
retains its exact operation order and results. Work is O(samples), with no
allocation. The public numerical policy remains report-only: normal f64 summation
and square-root rounding apply, and no perceptual or calibrated loudness claim is
made. Regression constant controls allow relative error 1e-11 even at f32::MAX;
binary-exact powers and unequal-region controls have exact expectations.

## Offline resampling boundaries and alignment

`audio.resampled_frame_index(source_boundary, destination_rate)` maps absolute
source boundaries in `0..=audio.frames()`, including the exclusive end. It uses
the same count helper as `Audio::resample` and performs the same default output
shape checks without rendering or allocating PCM.

The compatible 0.4 policy remains:

```text
ratio = destination_rate as f64 / source_rate as f64
mapped = (source_boundary as f64 * ratio).round() as usize
output_frames = mapped(input_frames)
```

Positive representable half ties round upward. This explicitly preserves floating
arithmetic rather than promising exact rational ties. For example, 500 frames at
1000 -> 1001 Hz produce 500 output frames: the floating product is slightly below
500.5, while exact rational nearest/halves-up would give 501. No sample values,
filter positions or legacy frame counts change in this update.

Map absolute body/full boundaries, then subtract. At 48000 -> 44100 Hz, source
boundaries 0/80/160 map to 0/74/147. The output body is 74 frames and tail is 73.
Independently resampling two 80-frame pieces instead yields two 74-frame buffers;
their 148-frame concatenation is a different operation with different edge filters.

Mapping is monotonic, zero maps to zero, and every accepted boundary is <= the
actual whole output length. Invalid source boundaries return `InvalidArgument`.
Default `Limits` constrain the complete destination shape before allocation,
including rate, duration, channels, frames and decoded bytes; rejection is
`LimitExceeded`. Floating-to-integer saturation cannot authorize an oversized
render because these limits are checked. Empty input stays empty, a very short
downsample may become empty, and identity resampling clones all PCM bits.

The Kaiser-windowed sinc evaluates output frame i at source position `i/ratio`.
Frame zero is source-time zero. Filtering reads future context offline, clamps
out-of-range taps to the first/last input sample, and normalizes tap weights.
There is no leading delay, inserted preroll, emitted release or additional filter
tail beyond `output_frames`. Near an edge or under anti-alias filtering, an
impulse's peak position/amplitude is not a latency proof. Authored tail is just
part of the input and follows the same mapping. These are offline `Audio`
contracts, not a new realtime scheduling or musical-timing API.

## Spectrum interpretation

`analyze::spectrum` returns `fft_size/2+1` bins with spacing
`sample_rate/fft_size` Hz, from DC through Nyquist. It applies the symmetric Hann
`0.5 - 0.5*cos(2*pi*i/(fft_size-1))`, divides complex magnitudes by the full Hann
sum, doubles interior bins and leaves DC/Nyquist undoubled. Magnitudes are linear
tone-amplitude estimates in input-sample units; they are not calibrated band RMS
or PSD. A bin-centered tone's magnitude approximates its peak amplitude.

Only the first FFT-length samples are transformed. Short input is right-padded
with zeros using the original full window and normalization. Its first and last
window positions are zero: a sole nonzero sample at index zero produces all-zero
magnitudes. FFT size two has a zero-sum Hann and retains existing NaN magnitudes.
Neither result proves that the unwindowed PCM is silent. Spectrogram columns use
the same policy at their respective offsets.

Sample rate must be nonzero and FFT size a power of two in 2..=1048576. All input
samples, including those after the truncated window, must be finite; invalid
arguments return `InvalidArgument`. Time is O(input samples + FFT size * log2 FFT
size), storage O(FFT size). Window/FFT math remains report-only. Controls allow
1e-12 DC/Nyquist amplitude error and 1e-5 for a 0.5-amplitude bin-centered tone;
this is a numerical regression tolerance, not a cross-host bit guarantee.

## Bounded regular-file input

`read_audio_file` now uses `ReadAudioOptions::default()`: at most 512 MiB encoded
bytes plus the existing separate default decoded limits. Use
`read_audio_file_with_options(path, &options)` to choose `max_encoded_bytes` or
the nested `decode: DecodeOptions` (including format override).

Before opening, metadata must identify a regular file and fit the encoded cap.
Symlinks to regular files are supported without project containment. Static
directories, devices and FIFOs, including FIFO symlinks, are `InvalidArgument`.
The opened handle is checked again. Paths must remain stable between these steps:
safe portable std I/O does not provide atomic protection from a hostile change to
a FIFO just before open. Studio's project containment remains Studio's job.

Actual reads are independently bounded to cap+1 bytes, so file growth cannot
bypass the size cap. The sentinel byte is not stored: the single growable encoded
buffer stores at most cap bytes, uses fallible bounded growth, and is borrowed
directly by the decoder. Exact-cap files are accepted; cap zero permits an empty
file to reach the decoder and rejects any nonempty file. Excess bytes/failed
reservation are `LimitExceeded`, filesystem/read failures are `Io`, and invalid
encoding or decoded-shape errors keep the decoder's categories. Encoded size,
decoded PCM and codec-specific limits are independent; raising one does not raise
the others. Successful WAV/FLAC/MP3 decodes keep their samples and metadata.

## Regression coverage

`tests/delivery.rs` covers endpoint/DC/Nyquist/short-window spectral controls,
linear power and unequal-length aggregation, all pairs of 32/44.1/48/96/192 kHz
at non-block lengths, floating tie compatibility, identity/alignment/edges,
normalization/report agreement within 0.0001 dB/LU, no-result/error cases and
bit-preserved PCM. Deterministic mapping and limiter/resample/seeded WAV/FLAC
round-trip measurements are pinned separately. Adjacent f32 amplitudes bracket
the absolute gate, while adjacent f64 thresholds distinguish ceiling acceptance.
These fixtures run natively and under WASI alongside existing render hashes.

`tests/read_audio_file.rs` covers exact encoded caps, decoded caps, regular-file
WAV/FLAC/MP3 identity, invalid/missing/directory inputs and extended oversized
files. Unix coverage adds sparse default-oversize rejection, regular symlinks and
static direct/linked FIFOs in child processes with five-second deadlines. Private
reader controls prove cap+1 actual reads against an endlessly growing source,
interrupted-read retry and I/O error propagation.
