# Measurements of a sound

These measurements describe one sound, such as a rendered note or a patch
audition, without changing it. They report numbers; what counts as a good attack
or a bright enough patch is the caller's decision. Studio's patch audition report
(audiobox-studio#103) reads them.

## Envelope times

`Audio::envelope` follows the amplitude of one channel or all of them, one value
per window of frames, and the [`Envelope`] reads times from it:

```rust
use audiobox::analyze::{EnvelopeMethod, EnvelopeOptions, EnvelopeThresholds};
use audiobox::{Audio, Result};

fn note_times(note: &Audio, gate_off_frame: usize) -> Result<()> {
    // 10 ms windows at 48 kHz.
    let envelope = note.envelope(&EnvelopeOptions::new(EnvelopeMethod::Rms, 480))?;
    let times = envelope.times(&EnvelopeThresholds {
        gate_off_frame: Some(gate_off_frame),
        ..EnvelopeThresholds::default()
    })?;
    if let Some(attack) = times.attack {
        println!("attack {:.1} ms", 1000.0 * attack.seconds(times.sample_rate));
    }
    Ok(())
}
```

- **Method.** `Peak` takes each window's largest absolute sample; `Rms` its root
  mean square. With all channels, the peak is the largest of any channel and the
  RMS covers every channel's samples together.
- **Window.** Every time has the resolution of one window: times are the starts of
  windows, except that a tail starts at its gate frame. A level exactly at a
  threshold counts as reaching it, despite the rounding of dB conversions. A `Peak` envelope of a tone needs windows of at least one period of its
  lowest note, or it follows the waveform rather than the amplitude.
- **Attack:** from the first window at or above `attack_from` × the peak to the
  first at or above `attack_to` × the peak (10 % to 90 % by default).
- **Decay:** from the first window that reaches the peak to the first after it at or
  below the peak lowered by `decay_db` (20 dB by default). The search runs to the
  end of the input, so a note that holds a sustain above that level gets the time
  until its release falls through it, sustain and release included. Measure a cut
  that ends at the gate (`Audio::cut`) for the decay to the sustain alone.
- **Tail:** from the gate-off frame the caller gives until the envelope stays at or
  below `tail_floor_db` relative to the peak (−60 dB by default) to the end of the
  input. It ends at the end of the last window above the floor, and it is empty
  when no window holding frames from the gate on is above it, as when the gate is
  at the end of the input.
- **No result.** Each time is `None` when its threshold is never crossed: silent or
  empty input, a note that never decays far enough, or one that still rings above
  the floor in its last window. Nothing is extrapolated.
- **Results** carry the method, window, channel, sample rate and thresholds they were
  measured with, and spans in frames with their length in seconds.

Bad arguments, a missing channel and non-finite samples return `InvalidArgument`.
The default `Limits` apply. The work is linear in the input and the storage is one
value per window. The values are the same on every host: sums run in a fixed order,
square roots are exactly rounded, and dB thresholds use the crate's own math.

## Spectral centroid and flux

`Audio::spectral_timeline` measures, per analysis frame, how bright a sound is and
how much its spectrum changed, and `SpectralTimeline::summary` sums up a range:

```rust
use audiobox::analyze::SpectralOptions;
use audiobox::{Audio, Result};

fn brightness(audio: &Audio, section: std::ops::Range<usize>) -> Result<()> {
    // 2048-sample frames every 1024 frames, over all channels.
    let timeline = audio.spectral_timeline(&SpectralOptions::new(2048, 1024))?;
    let summary = timeline.summary(section)?;
    println!(
        "centroid {:?} Hz, flux mean {:?}",
        summary.centroid_mean_hz, summary.flux_mean
    );
    Ok(())
}
```

- **Frames.** Frame `i` starts at input frame `i × hop` and reads `fft_size`
  samples, with the Hann window and magnitudes of [`analyze::spectrum`]. Bins are
  `sample_rate / fft_size` Hz apart; `fft_size` is a power of two from 16. Only
  whole frames are measured, so that no frame is padded with silence: the last few
  frames of input may fall in none. An input shorter than one frame is a single
  frame, zero-padded. A frame shorter than a few periods of the lowest note sees
  the waveform move within it, so its centroid and flux vary from frame to frame
  even on a steady sound.
- **Centroid** (Hz): the amplitude-weighted mean frequency of the frame's magnitude
  spectrum, DC and Nyquist included. Higher is brighter. It matches a sine's
  frequency within 1 % and a harmonic tone's analytic value within 2 % in the
  tests: the window's leakage spreads a little magnitude over every bin and pulls
  the centroid slightly toward the middle of the spectrum. A DC offset pulls it
  down.
- **Flux** (0 to 1): the distance between this frame's and the previous frame's
  magnitude spectra, each scaled to unit length, divided by √2. It ignores level,
  so a steady sound that swells has next to none; 0 is the same shape, 1 is no bin
  in common. A sine's flux is not exactly 0 when the hop is not a whole number of
  periods, but stays below 0.01 in the tests.
- **Silence.** A frame whose RMS is at or below `silence_db` (−80 dBFS by default)
  has neither measurement. Neither has a frame above it whose windowed spectrum is
  empty, as a short input's lone first sample (the window is 0 there). A frame
  after one without a centroid has no flux, and the first frame has none.
- **Channels.** One channel by index, or all of them: then each bin's magnitude is
  the RMS of the channels' magnitudes, so channels in opposite phase do not cancel.
- **Summary.** Over the frames that start in a range of input frames: the count of
  frames and of measured ones, the mean, lowest and highest centroid and the mean
  and highest flux, `None` when no frame has the value. Flux compares with the frame
  before, so the first frame after a change still shows it.

Bad options, a missing channel, a range outside the input and non-finite samples
return `InvalidArgument`. The default `Limits` apply, plus at most
`MAX_SPECTRAL_FRAMES` (1,048,576) frames and `MAX_SPECTRAL_WORK` (2³¹) samples
transformed (frames × channels × `fft_size`); beyond them the result is
`LimitExceeded`. Silent frames are not transformed. The work is
O(frames × channels × fft_size × log fft_size); storage is O(fft_size) plus one
entry per frame. The FFT uses the platform's math, so these numbers are for reports
and are not bit-identical across hosts.

[`Envelope`]: https://docs.rs/audiobox/latest/audiobox/analyze/struct.Envelope.html
[`analyze::spectrum`]: https://docs.rs/audiobox/latest/audiobox/analyze/fn.spectrum.html
