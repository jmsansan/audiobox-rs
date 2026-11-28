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

- **Frames.** Frame `i` starts at input frame `i × hop` and reads
  `PitchTrack::span_frames`: an integration window of one period of `min_hz`
  (at least 96 samples, so that a narrow range of high pitches still integrates
  over several periods), compared with the samples up to one period later and a
  few more, for the margin below `min_hz`. Short periods also read 17 samples of
  the input on each side of the span, for interpolating them. A pitch describes
  that span, so it
  lags the sound by up to that much: at 60 Hz, 33 ms. Only whole frames are
  measured; an input shorter than one frame is a single frame, zero-padded.
- **States.** `Silent` when the RMS of the frame's integration window, with its
  mean removed, is at or below `silence_db` (−60 dBFS by default): a DC offset or
  a level stuck at one value is silence. Otherwise YIN takes the first dip of its
  normalized difference, from the shortest period, whose refined bottom is below
  `threshold` (0.15 by default). `Voiced` when that dip's period lies in the
  range: the pitch from `min_hz` to `max_hz`, the ends included. A tone outside
  the range has no pitch, rather than the nearest one in it or one of its
  period's multiples. So a weak fundamental in the range under a strong harmonic
  above `max_hz` has no pitch either: a ceiling above the harmonic gives the
  harmonic as the pitch and the fundamental as the alternative. `Unvoiced`
  otherwise, as noise, a chord or a transition. Only voiced frames have a pitch;
  nothing is guessed for the others.
- **Pitch.** That dip's period, refined between samples. A period of 48 samples
  or more (up to 1 kHz at 48 kHz) is refined by the parabola through the raw
  difference function, which is symmetric about a whole period, unlike the
  normalized one: within 0.1 cents for sines and about 1 cent for rich tones
  there, 2 cents just above 48 samples at the most. A parabola misjudges shorter
  periods, in place and in depth, so every dip of one (from 2 samples), in the
  range or above it, is refined before the threshold decides: the difference
  over the whole window is evaluated between samples, its correlation term
  interpolated from the whole lags' by a 32-tap Kaiser-windowed sinc (the
  correlation of the window with a band-limited sound is band-limited too) and
  its energy term linearly, which errs by about 1 / window of it. The depth is
  read there, normalized as at whole lags, so a sound that changes within the
  window, as an attack, is judged by all of it. A short dip below the threshold
  then has its period found once more exactly, with the signal itself
  interpolated by the same
  sinc over four periods, or 64 samples if longer, in the middle of the window
  as far as it allows, before the range and the threshold decide on it, at the
  exact period (up to two such searches a frame; a dip that fails gives way to
  the next). The taps are scaled to sum to 1, so a DC offset cancels. That is
  within 0.03 cents for sines and harmonic tones in the tests, as long as the
  sound has no strong content within a tenth of the Nyquist frequency, which no
  interpolation follows. An estimate within 5 cents past an end of the range is
  set to that end. `max_hz` is at most `sample_rate / MIN_PITCH_LAG`, so that a
  pitch spans at least 8 samples (6 kHz at 48 kHz). **Confidence** is 1 minus
  the normalized difference at the refined bottom, from 0 to 1: of the pitch's
  dip for a voiced frame, of the deepest dip in the range for an unvoiced one (0
  without one, as for a tone below the range); 0 when silent.
- **Alternative.** For a voiced frame, the other octave: the deepest dip in the
  range within a semitone of half or twice the pitch's period, refined as any
  dip is (by the correlation table when short, by its parabola otherwise). A
  sound with a weak fundamental and a strong octave has both
  octaves as dips, so the pitch and the alternative are the two candidates:
  when their confidences are close, the octave is ambiguous, and the caller
  decides. Any periodic sound also repeats at twice its period, so that dip
  counts only when deeper than the pitch by more than 0.01 and the parabola's
  uncertainty (how much it lowered either depth): a clean tone has no rival.
  Other periods, such as a fifth's, are not offered.
- **Smoothing.** `median` (odd, 1 to 15) replaces each voiced pitch with the median
  of the voiced pitches within that many frames; silent and unvoiced frames stay
  without one. It reads up to `median / 2` frames ahead, which adds that many hops
  to the latency, and it changes the pitch only: confidence and alternative still
  describe the frame's own estimate. `PitchTrack::median_frequency` gives the
  median pitch of a range.
- **Channels.** One channel by index, or the mean of all channels.

It is a tool for one voice, not melody extraction: in a chord it finds the period
the notes repeat at together (a fifth on 220 and 330 Hz gives 110 Hz), or a lower
confidence when they share none, and it does not choose a part in a mix. Note
names and quantization are the caller's. Like any YIN tracker, it can take a
period that only a sound's strongest harmonics repeat at when they outweigh a
weak fundamental: with the 6th and 7th harmonics 4 to 5 dB above it, a low note
can be read at 7/6 of its pitch, and a note whose 6th harmonic dominates at 6/5.

Bad options, a missing channel, a range outside the input and non-finite samples
return `InvalidArgument`. `min_hz` is at least `sample_rate / MAX_PITCH_LAG`
(8192) and `max_hz` at most `sample_rate / MIN_PITCH_LAG` (8). The default
`Limits` apply, plus at most `MAX_PITCH_FRAMES` (1,048,576) frames and
`MAX_PITCH_WORK` (2³⁵, a work estimate of frames × (L × (log₂ L + 1) + 3 × 2¹⁶ +
34 × W), with L the power of two at or above the frame's span, W the
integration window, 3 × 2¹⁶ for refining short periods between samples and 34 × W
for the correlations that needs beyond the transforms'),
counted before any frame is measured, or `LimitExceeded`. Ten minutes at
48 kHz from 60 Hz to 2 kHz every 10 ms is about 15 billion (about 11 s here for
a high-pitched sound). Several channels
are mixed once, into one buffer of the input's length. Silent frames skip the
transforms. The FFT uses the platform's math, so pitches are for reports and
not bit-identical across hosts; the tests' tolerances (5 cents for steady
tones, 15 for glides and vibrato) leave room for that.

Evaluation on the generated fixtures of `tests/pitch.rs`, free to share, at 48 kHz:

| Fixture | Settings | Voiced frames | Largest error |
| --- | --- | --- | --- |
| Sines at 82.41, 440 and 1760 Hz, 1 s | 60 Hz–2 kHz, 10 ms hop | all | 0.001, 0.009 and 0.002 cents |
| Sawtooth, 30 harmonics, at 147 and 523.25 Hz | 60 Hz–2 kHz, 10 ms hop | all | 0.04 and 0.7 cents |
| Glide 220 → 440 Hz in 1 s | 100 Hz–1 kHz, 10 ms hop | all | 5.0 cents from the pitch at the frame's middle |
| 440 Hz with ±50 cents of vibrato at 5 Hz | 100 Hz–1 kHz, 5 ms hop | all | 6.4 cents, likewise |
| White noise, 0.5 s | 60 Hz–2 kHz, 10 ms hop | none of 47 | — |
| Tritone (220 Hz and 220 × √2 Hz) | 60 Hz–2 kHz, 10 ms hop | mean confidence 0.90, against 1.00 for a sine | — |

A wider range lengthens the frame, which averages more of a moving pitch: the
same vibrato from 60 Hz to 2 kHz, every 10 ms, errs by up to 11.5 cents.

[`Envelope`]: https://docs.rs/audiobox/latest/audiobox/analyze/struct.Envelope.html
[`analyze::spectrum`]: https://docs.rs/audiobox/latest/audiobox/analyze/fn.spectrum.html
