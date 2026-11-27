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
  `sample_rate / fft_size` Hz apart; `fft_size` is a power of two from 16 to 1,048,576 and `hop` at least 1. Only
  whole frames are measured, so that no frame is padded with silence: the last few
  frames of input may fall in none. An input shorter than one frame is a single
  frame, zero-padded. A frame shorter than a few periods of the lowest note sees
  the waveform move within it, so its centroid and flux vary from frame to frame
  even on a steady sound.
- **Centroid** (Hz): the amplitude-weighted mean frequency of the frame's magnitude
  spectrum, DC and Nyquist included. Higher is brighter. It matches a sine's
  frequency within 1 % and a harmonic tone's analytic value within 2 % in the
  tests, in every frame: the window's leakage spreads a little magnitude over
  other bins, which shifts the centroid slightly. A DC offset pulls it down.
- **Flux** (0 to 1): the distance between this frame's and the previous frame's
  magnitude spectra, each scaled to unit length, divided by √2. It ignores level,
  so a steady sound that swells has next to none; 0 is the same shape, 1 is no bin
  in common. A sine's flux is not exactly 0 when the hop is not a whole number of
  periods, but stays below 0.01 in the tests.
- **Silence.** A frame whose RMS is at or below `silence_db` (−80 dBFS by default)
  has neither measurement. The RMS is of the frame's unwindowed samples, over the
  samples it holds (not a short input's padding) and pooled over the channels
  measured, so a quiet channel lowers it. Neither has a frame above it whose windowed spectrum is
  empty, as a short input's lone first sample (the window is 0 there). A frame
  after one without a centroid has no flux, and the first frame has none.
- **Channels.** One channel by index, or all of them: then each bin's magnitude is
  the RMS of the channels' magnitudes, so channels in opposite phase do not cancel.
- **Summary.** Over the frames that start in a range of input frames: the count of
  frames and of measured ones (those with a centroid), the mean, lowest and highest centroid and the mean
  and highest flux, `None` when no frame has the value. Flux compares with the frame
  before, so the first frame after a change still shows it.

Bad options, a missing channel, a range outside the input and non-finite samples
return `InvalidArgument`. The default `Limits` apply, plus at most
`MAX_SPECTRAL_FRAMES` (1,048,576) frames and `MAX_SPECTRAL_WORK` (2³², a work estimate of
frames × channels × `fft_size` × log₂ `fft_size`, twice the FFT's butterflies); beyond them
the result is `LimitExceeded`. Both count every frame, silent or not, before any
is measured; ten minutes of 48 kHz stereo with 2048-sample frames every 1024 is
about 1.3 billion. Silent frames are not transformed. The work is a scan of the
samples plus O(frames × channels × fft_size × log fft_size); storage is
O(fft_size) plus one entry per frame. An empty input has no frames. The FFT uses
the platform's math, so these numbers are for reports and are not bit-identical
across hosts.

## Pitch track

`Audio::pitch_track` follows the pitch of one voice by the YIN method (de
Cheveigné and Kawahara, 2002): per analysis frame, whether the sound is voiced,
its pitch, how periodic it is, and the best other period.

```rust
use audiobox::analyze::{PitchOptions, PitchState};
use audiobox::{Audio, Result};

fn melody(voice: &Audio) -> Result<()> {
    // 60 Hz to 1.5 kHz, a frame every 10 ms at 48 kHz.
    let track = voice.pitch_track(&PitchOptions::new(60.0, 1500.0, 480))?;
    for frame in &track.frames {
        if frame.state == PitchState::Voiced {
            println!("{} {:?}", frame.start_frame, frame.frequency_hz);
        }
    }
    Ok(())
}
```

- **Frames.** Frame `i` starts at input frame `i × hop` and reads two periods of
  `min_hz` (`PitchTrack::span_frames`): its first period is compared with the
  samples up to one period later. A pitch describes that span, so it lags the
  sound by up to that much: at 60 Hz, 33 ms. Only whole frames are measured; an
  input shorter than one frame is a single frame, zero-padded.
- **States.** `Silent` when the RMS of the frame's first period is at or below
  `silence_db` (−60 dBFS by default); `Voiced` when YIN's normalized difference
  falls below `threshold` (0.15 by default) between `max_hz` and `min_hz`;
  `Unvoiced` otherwise, as noise, a chord or a transition. Only voiced frames have
  a pitch; nothing is guessed for the others.
- **Pitch.** The first dip below the threshold, at its bottom, refined between
  samples. **Confidence** is 1 minus the normalized difference at the best period,
  from 0 to 1, for voiced and unvoiced frames; 0 when silent.
- **Alternative.** The deepest other dip more than a semitone from the pitch. A
  sound with a weak fundamental and a strong octave has both octaves as dips, so
  the pitch and the alternative are the two candidates: when their confidences
  are close, the octave is ambiguous, and the caller decides.
- **Smoothing.** `median` (odd, 1 to 15) replaces each voiced pitch with the median
  of the voiced pitches within that many frames; silent and unvoiced frames stay
  without one. `PitchTrack::median_frequency` gives the median pitch of a range.
- **Channels.** One channel by index, or the mean of all channels.

It is a tool for one voice, not melody extraction: in a chord it finds the period
the notes repeat at together (a fifth on 220 and 330 Hz gives 110 Hz), or a lower
confidence when they share none, and it does not choose a part in a mix. Note
names and quantization are the caller's.

Bad options, a missing channel, a range outside the input and non-finite samples
return `InvalidArgument`. `min_hz` is at least `sample_rate / MAX_PITCH_LAG`
(8192) and `max_hz` at most half the sample rate. The default `Limits` apply,
plus at most `MAX_PITCH_FRAMES` (1,048,576) frames and `MAX_PITCH_WORK` (2³², a
work estimate of frames × L × log₂ L, with L the power of two at or above two
periods of `min_hz`), counted before any frame is measured, or `LimitExceeded`.
Silent frames skip the transforms. The FFT uses the platform's math, so pitches
are for reports and not bit-identical across hosts; the tests' tolerances (5
cents for steady tones, 15 for glides and vibrato) leave room for that.

Evaluation on the generated fixtures of `tests/pitch.rs`, free to share, at 48 kHz
with frames every 10 ms from 60 Hz to 2 kHz:

| Fixture | Voiced frames | Largest error |
| --- | --- | --- |
| Sines at 82.41, 440 and 1760 Hz, 1 s | all | 0.001, 0.08 and 1.0 cents |
| Sawtooth, 30 harmonics, at 147 and 523.25 Hz | all | 0.03 and 0.6 cents |
| Glide 220 → 440 Hz in 1 s; 440 Hz ±50 cents of vibrato at 5 Hz | all | 6.4 cents from the pitch at the frame's middle |
| White noise, 0.5 s | none of 47 | — |
| Tritone (220 Hz and 220 × √2 Hz) | mean confidence 0.90, against 1.00 for a sine | — |

[`Envelope`]: https://docs.rs/audiobox/latest/audiobox/analyze/struct.Envelope.html
[`analyze::spectrum`]: https://docs.rs/audiobox/latest/audiobox/analyze/fn.spectrum.html
