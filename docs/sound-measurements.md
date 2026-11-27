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
  below the peak lowered by `decay_db` (20 dB by default). A note that falls to a
  sustain above that level has no decay time at that threshold.
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

[`Envelope`]: https://docs.rs/audiobox/latest/audiobox/analyze/struct.Envelope.html
