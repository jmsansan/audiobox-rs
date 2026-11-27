# Procedural voices (`voice` feature)

`audiobox::voice` makes small synthetic vocal sounds for games and music: robot
dialogue blips, vowel-like gibberish that follows the rhythm and punctuation of
dialogue text, and explicitly pitched vocal notes. It is **not text-to-speech**.
Nothing is pronounced, and the output is meant to be unintelligible: think of the
murmur of a character speaking in a made-up language.

The feature enables `synth` (it reuses the synthesizer's oscillators) and does not
need `mixer`. With `mixer`, a prepared voice is a mixer `Stream`; with `events`, a
designer can name a prepared line as a sound-event source.

Versions: render algorithm `RENDER_VERSION = 2`, profile text and presets
`PROFILE_FORMAT_VERSION = 1`, text planner `PLANNER_VERSION = 2`. A change that
alters their output bumps the version.

## Three stages

```rust
use audiobox::voice::{GibberishOptions, PreparedUtterance, VoiceOptions, VoicePreset,
    VoiceProfile, plan_gibberish};

// 1. Describe: a character and a line. May allocate.
let profile = VoiceProfile::preset(VoicePreset::SmallCreature);
let plan = plan_gibberish("¿Traes el mapa?", &profile, &GibberishOptions::default())?;
// 2. Prepare for a rate: validate, time in frames, freeze. May allocate.
let voice = PreparedUtterance::new(&profile, &plan, &VoiceOptions::default())?;
// 3. Render: offline, or in chunks on an audio thread without allocating.
let audio = voice.render()?;
let mut synth = voice.synth();
let mut block = [0.0f32; 256];
while synth.fill(&mut block) > 0 { /* hand the block to the device */ }
# Ok::<(), audiobox::Error>(())
```

| Stage | Types | Allocates | Thread |
| --- | --- | --- | --- |
| Describe | `VoiceProfile`, `VocalEvent`, `UtterancePlan`, `plan_gibberish`, `VocalNote` | yes | game |
| Prepare | `PreparedUtterance::new`, `PreparedUtterance::from_notes` | yes | game |
| Render | `PreparedUtterance::render`, `VoiceSynth::fill`, `reset`, `seek`, `marker_at` | `render` only | `fill`/`reset`/`seek`/`marker_at`: any, including audio |

Music does not go through text: `VocalNote`s supply explicit frames, pitch and
vowels. Text only ever reaches the planner.

## Rendering contract

- **Rates**: 8000..=192000 Hz. **Channels**: 1 or 2; stereo copies the mono frame.
- **Time units**: `VocalEvent::duration_us` is integer microseconds. Preparation sums
  durations exactly and rounds each absolute boundary once, to the nearest frame with
  halves up, so a plan never drifts (`1000 × 1 ms` at 44101 Hz is exactly 44101
  frames). A gesture shorter than half a frame is skipped. `VocalNote` positions are
  frames already.
- **Tail**: after the last event the envelope falls at the profile's `release` rate;
  then 40 ms of ring-out let the resonators decay, and the last 5 ms of it (at most
  256 frames) fade linearly to an exact 0. `frames()` = `body_frames()` +
  `ceil(release · rate)` + `ceil(0.04 · rate)`. An empty plan has 0 frames and no
  tail.
- **Headroom**: samples pass a fixed guard. Magnitudes up to 0.9 pass unchanged;
  above, `0.9 + 0.1·t/(1+t)` with `t = (|x|−0.9)/0.1`, so output stays inside
  (−1, 1). `VoiceSynth::peak_before_guard` reports the largest pre-guard magnitude.
  The presets peak between 0.35 and 0.7 at seed 7 (see below). A DC blocker (20 Hz
  one-pole high-pass) removes the offset of asymmetric pulses.
- **Determinism**: identical profile, plan, options and seed give identical samples
  on every platform, WebAssembly included, independent of chunk sizes and of other
  voices or render order. The DSP uses `+ - * /`, `floor`, `sqrt` and the crate's
  own `sin`, `cos`, `exp` and `log2` series. Control values (pitch, formant
  coefficients, modulation) update every 16 frames of the *absolute* frame count and
  at every segment boundary, so chunking cannot move them.
- **Randomness**: aspiration noise, frication noise, jitter and shimmer each have
  their own SplitMix64 stream derived from `VoiceOptions::seed`. A reset restarts
  them. The planner has a separate seed in `GibberishOptions::seed`.
- **Prepared changes**: a `PreparedUtterance` is immutable and cheap to clone (its
  segments, markers and text are `Arc`s). Gain, pan, rate, pitch and fades during
  playback belong to the mixer. A different profile or plan means preparing again.
- **Seeking**: `seek(0)` and `reset` are cheap. A backward seek resets and renders
  silently up to the target; a forward seek renders silently from the current
  position. Both are exact and allocation-free; a forward seek costs the distance and
  a backward seek the target position (from frame 0, even one frame back),
  so not constant-time or callback-budget operations. A target past the end moves to
  the end. A mixer `Play::start` seeks on the game's thread. `Loop::Whole` restarts
  cheaply; a `Loop::Region` with a non-zero start replays on the audio thread, so
  render to `Audio` for region loops.
- **Dropping**: a renderer shares the plan through `Arc`s, so dropping it frees
  nothing while the `PreparedUtterance` or another renderer is alive; dropping the
  last owner frees the buffers. Keep the prepared utterance on the control thread.
  The mixer returns finished voices to `MixerHandle::poll_event` to be freed there,
  as long as events are polled; if its return queue fills because they are not, it
  frees retired voices on the audio thread (see the mixer documentation).

### Errors

All errors carry a stable `ErrorCode`.

| Case | Code |
| --- | --- |
| Profile field out of range or non-finite | `InvalidArgument` |
| Rate outside 8000..=192000 Hz, channels other than 1 or 2 | `InvalidArgument` |
| Voiced pitch non-finite, outside 20..=4000 Hz, or ≥ 0.45·rate with the profile's vibrato and jitter | `InvalidArgument` |
| Level or velocity outside 0..=1 | `InvalidArgument` |
| Notes out of order or overlapping | `InvalidArgument` |
| `VoiceLimits` above the hard ceilings | `InvalidArgument` |
| More events, markers or text bytes than the limits; longer than `max_seconds` with the tail | `LimitExceeded` |
| Profile text: malformed, unknown or repeated key | `Syntax`, with line and column |
| Profile text over 16 KiB | `LimitExceeded` |

Hard ceilings: 4096 events (or notes), 16384 markers, 16 KiB of text, 120 s per
utterance. `VoiceLimits` may lower them. Text is `&str`, so it is always valid UTF-8;
unsupported scalars are skipped, not errors (see the planner).

## DSP design

### Formant voices

A source runs at the event's pitch. `VoiceSource::Smooth` is the derivative of a
raised-cosine glottal flow pulse: the flow rises for two thirds of the open phase
(`open`, 0.3..=0.9 of the period) and falls for one third; the derivative steps back
to zero at closure, and that one step is band-limited with a PolyBLEP. It has zero
mean and a negative peak of −1. `VoiceSource::Saw` is the synthesizer's band-limited
sawtooth, kept as the buzzier, more robotic baseline. An optional one-pole low-pass
(`tilt`: cutoff `12000 · 2^(−5·tilt)` Hz) softens either source.

The source plus aspiration noise goes through a **cascade** of four two-pole
resonances (F1–F4) with unity gain at DC, as in Klatt's cascade synthesizer, each
the low-pass output of a trapezoidal state-variable filter; the result is divided by F1's
approximate resonance gain (bandwidth/frequency) to even out loudness across
vowels. Fricative noise goes through a separate band-pass at the profile's
`fricative` frequency (bandwidth 0.6 of the centre).

Formant targets at `resonance = 1`:

| Vowel | F1 | F2 | F3 | F4 |
| --- | --- | --- | --- | --- |
| A | 800 | 1200 | 2500 | 3500 |
| E | 500 | 1900 | 2550 | 3500 |
| I | 300 | 2250 | 2950 | 3600 |
| O | 500 | 900 | 2450 | 3400 |
| U | 330 | 750 | 2350 | 3300 |

Bandwidths are 70, 90, 140 and 200 Hz, times `bandwidth · resonance` (so `resonance`
keeps the resonances' Q). Frequencies scale with `resonance`. A formant whose centre
would exceed **0.42 of the sample rate** for any vowel is left out for the whole
utterance (`PreparedUtterance::active_formants`): at 8 kHz with `resonance = 2` only
F1 remains; at 44.1 kHz and above all four always fit. The fricative centre is
clamped to the same limit. F1 always fits (at most 1600 Hz).

Vowel changes move the formant frequencies and bandwidths linearly over the
profile's `transition` time, starting from wherever they are, so an interrupted
transition continues smoothly. Every gesture moves toward its vowel, so a breath or
fricative before a vowel is already coloured by it.

**Why a cascade.** The 0.6 plan asked to compare a parallel bank with a cascade. A
parallel bank needs a gain per formant and per vowel and leaves deep valleys or
phase cancellations between peaks unless those gains are tuned; a cascade gives the
natural relative formant levels of a vowel from the frequencies and bandwidths
alone, so there are no per-vowel gains to tune or expose. Coefficients are
updated every 16 frames. The state-variable form matters: a direct-form resonator
can grow without bound when its coefficients switch quickly even though each set
is stable on its own (found in review: rapid A/I switching at 192 kHz), while the
state-variable filter keeps integrator states whose energy cannot grow without
input, so it stays bounded; a regression test alternates vowels every 1.9 ms with
the narrowest bandwidths.

**Frequency and bandwidth.** Each resonance's `g` and `k` are solved so its digital
poles sit exactly at Klatt's radius `exp(−π·BW/rate)` and angle `2π·F/rate`, at
every rate up to the 0.42 ceiling (a prewarped analog mapping would shift wide
resonances low and narrow those near Nyquist). As with any two-pole resonance, the
frequency is the *pole* frequency: with narrow bandwidths the response peaks there
(within 6% for the default bandwidths, tested at 16–96 kHz), while very wide
bandwidths (high `bandwidth` on a low F1) move the peak lower and flatten it, which
is part of what `bandwidth` does to the timbre.

The cost of a cascade is that the fricative needs its own parallel branch, which it
gets. The earlier October prototype used parallel three-band banks with spectral
crossfades; that code was never merged, and this implementation replaces it.

### Robot voices

`VoiceMode::Robot(Waveform)` plays a dry synthesizer oscillator (pulse of any duty,
triangle, saw, or short/long noise) with the same envelopes, pitch glides and timing
and no formant filtering; vowels do not change its sound. Breath gestures add plain
noise; fricatives use the fricative band-pass.

### Envelopes and gestures

Three envelopes (voiced, aspiration, frication) move linearly toward each gesture's
targets: up at `1/attack` per second; down at `1/attack` between gestures,
including the `Closure` events of explicit and text plans (`VocalEvent::rest`,
word gaps and pauses), and at `1/release` in the rests between notes of a note list
and in the final tail.

| Articulation | Voiced | Aspiration | Frication |
| --- | --- | --- | --- |
| `Voiced` | level | level × `breath` | 0 |
| `Breath` | 0 | level × 0.22 (robot: level) | 0 |
| `Fricative` | 0 | 0 | level × 0.3 (robot: level) |
| `Closure` | 0 | 0 | 0 |

In formant voices these factors set breath and fricative gestures 10–15 dB below a
vowel of the same level, as in speech (render version 2). They scale the envelope
before the formant filters, so the rendered gap varies with the preset and the
sound: about 10 to 21 dB. Version 1 played them at
the vowel's level, which the owner heard as hissing "ts" sounds and puffs of air
(2026-11-20).

The source phase never restarts within an utterance, so pitch steps do not click.
Pitch glides geometrically (evenly in cents) across a voiced gesture; other gestures
hold the current pitch. Vibrato (`vibrato` cents at `vibrato_rate` Hz) is a sine of
the absolute frame; jitter (`jitter` cents) and shimmer (`shimmer`, relative level)
are seeded random points every 1/30 s and 1/20 s with linear ramps between them.

## Profiles

| Field | Key | Unit | Range |
| --- | --- | --- | --- |
| mode | `mode=formant source=smooth\|saw` or `mode=robot wave=pulse\|triangle\|saw\|noise` (`duty`, `noise`) | | |
| `pitch` | `pitch` | Hz | 50..=1000 |
| `range` | `range` | semitones | 0..=24 |
| `resonance` | `resonance` | ratio | 0.5..=2 |
| `bandwidth` | `bandwidth` | ratio | 0.25..=4 |
| `open` | `open` | fraction of period | 0.3..=0.9 |
| `tilt` | `tilt` | | 0..=1 |
| `breath` | `breath` | ratio | 0..=1 |
| `fricative` | `fricative` | Hz | 1000..=12000 |
| `attack` | `attack` | ms | 1..=200 |
| `release` | `release` | ms | 1..=1000 |
| `transition` | `transition` | ms | 1..=300 |
| `glide` | `glide` | ms | 0..=500 |
| `vibrato` | `vibrato` | cents | 0..=100 |
| `vibrato_rate` | `vibrato-rate` | Hz | 0.1..=12 |
| `jitter` | `jitter` | cents | 0..=50 |
| `shimmer` | `shimmer` | ratio | 0..=0.5 |
| `gain` | `gain` | linear | 0..=2 |

`pitch` and `range` are where the text planner speaks; explicit events and notes
carry their own Hz. `pitch` and `resonance` are independent: raising the notes does
not shrink the character, and a smaller character does not sing higher. The test
suite checks that resonance moves the spectral envelope while the fundamental stays,
and that doubling pitch leaves the envelope in place.

### Presets (profile format 1)

| Preset | Mode | pitch | range | resonance | bandwidth | open | tilt | breath | attack/release ms | vibrato/jitter cents | gain |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `robot` | robot, pulse 25% | 180 | 7 | 1 | 1 | 0.65 | 0 | 0 | 4 / 40 | 0 / 0 | 0.3 |
| `small-creature` | formant, smooth | 420 | 10 | 1.45 | 1.2 | 0.55 | 0.1 | 0.08 | 6 / 50 | 25 / 12 | 0.8 |
| `large-creature` | formant, saw | 85 | 5 | 0.68 | 1.4 | 0.7 | 0.35 | 0.12 | 18 / 120 | 10 / 18 | 0.9 |
| `soft-rounded` (default) | formant, smooth | 190 | 6 | 1 | 1 | 0.65 | 0.3 | 0.06 | 12 / 90 | 12 / 6 | 0.8 |
| `airy-breathy` | formant, smooth | 230 | 7 | 1.12 | 1.6 | 0.8 | 0.45 | 0.55 | 20 / 140 | 8 / 8 | 0.9 |

`VoiceProfile::to_string()` writes every field; `examples/voices.rs` writes each
fixture's exact profile next to its WAV.

### Text form

```text
# A tired giant.
version=1 preset=large-creature
pitch=70 release=200
```

`version=1` is required. `preset=` starts from a preset; every other key overrides
only what it names, wherever it appears. `#` comments. Display writes the canonical
form, which parses back bit for bit, including negative zero and subnormals. Errors
are `synth::ParseError` with a 1-based line and column.

## Text to gibberish

`plan_gibberish(text, profile, options)` compiles a line into events and markers.
The rules of planner version 2 are in its API documentation; in short:

- **Letters**: ASCII letters and digits, Latin-1 and Latin Extended-A/B, Greek and
  Cyrillic group up to three to a syllable; kana, hangul and CJK ideographs are one
  syllable each. Combining marks, variation selectors, the zero-width joiner and the
  prolonged sound mark attach to the previous letter. Apostrophes and hyphens join a
  word silently. Everything else (emoji, symbols, other scripts) is a silent word
  boundary. This is scalar classification with fixed tables, not grapheme
  segmentation, so it does not change with the Rust version's Unicode tables.
- **Phrases**: `,` `;` `:` pause inside a phrase; `.` `…` `?` `!` close it (repeated
  marks count once, `?` wins); Spanish `¿` and `¡` open a phrase and set its ending
  if no closing mark follows. A phrase marker starts at its opening mark.
- **Syllables**: each is a vowel with an optional onset (plosive, fricative, breath)
  chosen from a hash of its letters and the seed, so the same word sounds the same
  wherever it appears in a seed's dialogue.
- **Delivery**: `Delivery::Rigid` (equal syllables, base pitch) or
  `Delivery::Expressive` (bounded seeded variation). Delivery is independent of the
  profile's timbre.
- **Pauses**: word gaps 45 ms, commas 220 ms, phrase ends 380 ms, none at the end.
- **Contours**: statements fall by 0.5·range, questions rise to +0.7·range,
  exclamations start high and fall, a syllable before a comma rises slightly.

Markers (`MarkerKind::Phrase`, `Word`, `Syllable`) hold UTF-8 byte spans on character
boundaries and event ranges; prepared markers add **source-frame** ranges. They are
sorted by byte start and kind, and frame starts never decrease.
`PreparedUtterance::marker_at(kind, frame)` finds the marker under a source frame
without allocating.

## Mixer and events

`PreparedUtterance` and `VoiceSynth` convert into `mixer::Sound`. Follow animation with
the voice's **source position**, `MixerHandle::voice_status(id).position`, and
`marker_at`: it holds while paused and follows rate and pitch changes, unlike a wall
clock. The position is published by the mixer once per render, so markers are as
late as one callback. The mixer renders into buffers; opening the audio device is the
host's job (cpal, SDL, Web Audio, a game engine). `examples/voice_dialogue.rs` prints
a marker timeline and follows it through a pause.

With `events`, `Sources::insert_voice(name, &prepared)` renders a prepared line once
so an `EventBank` can name it. The bank never plans text or prepares voices.

## Musical notes

`PreparedUtterance::from_notes(profile, notes, options)` takes the notes of one
monophonic voice at explicit frames. The rules (re-articulation, legato glides,
onsets, rests, short notes) are on `VocalNote`. Harmony is several prepared voices
with their own seeds, mixed offline or played as mixer voices; they stay aligned to
the frame. Bars, tempo, lyrics and arrangement belong to the caller (Studio).
`examples/voice_music.rs` writes a robot melody and a three-voice vowel harmony.

## Validation and measurements

`tests/voice.rs`, `tests/voice_alloc.rs` and `tests/voice_mixer.rs` cover:

- identical samples for whole renders and chunks of 1, irregular, 512 and more than
  the remaining frames, at 8, 44.1, 48, 96 and 192 kHz, mono and stereo, for all
  presets; reset, seek and end-of-stream replay;
- pinned fingerprints of the five presets and of two note lists, checked natively, on
  Rust 1.85 and under WebAssembly (WASI) in CI;
- exact boundary rounding at 44101 Hz, the tail length and a final zero sample;
- finite output inside (−1, 1) for the extreme corners of every profile range and
  pitch glides from 30 to 1500 Hz at 8, 48 and 192 kHz;
- no adjacent-sample step above half the peak across vowel, gesture and pitch
  changes;
- resonator poles exactly at the requested frequency and bandwidth for extreme
  accepted values (wide F1, the 0.42 ceiling at 8 kHz, 192 kHz), and response peaks
  within 6% of every vowel's F1/F2 at default bandwidths at 16, 44.1, 48 and 96 kHz;
  breath noise putting energy where each vowel's formants are;
- pitch accuracy of steps and geometric glides (0.5%) and of sustained notes (1%),
  velocity gain, legato joins without a dip and re-articulated joins with one;
- profile text round trips and positioned errors; planner Unicode cases (Spanish
  punctuation and accents, emoji with skin tones and ZWJ sequences, combining marks,
  repeated letters and punctuation, whitespace, other scripts, empty and
  punctuation-only text), marker integrity, contour and pause rules, expressive
  bounds and limits;
- zero allocations and frees during `fill`, marker queries, end of stream, `reset`,
  `seek` (also past the end), dropping a renderer while the prepared voice lives, and
  mixer playback, a whole loop and stopping, after the prepared voice was dropped,
  with events polled;
- mixer playback equal to the offline render within 1e-6, scheduled starts, start
  positions, whole loops, pause, 44.1 kHz output, rate 2, stop fades and sound events.

These checks establish numerical behaviour only. They say nothing about whether a
voice sounds good; that needs listening.

### Cost

`cargo run --release --example voice_bench --features voice` renders a 90-character line
per voice in callback-sized blocks. On an AMD Ryzen 7 1700X (Windows 10, rustc
1.98.1, release), one voice costs, as a share of one core's real-time budget:

| Profile | 44.1 kHz | 48 kHz |
| --- | --- | --- |
| `robot` | 0.4% | 0.4% |
| `soft-rounded` | 0.6% | 0.6% |
| `airy-breathy` | 0.6–0.7% | 0.65% |

Cost scales linearly with voices (8 formant voices ≈ 5%, 32 ≈ 20% at 48 kHz) and
barely depends on the callback size (64, 256 or 1024 frames). The worst single
callback was several times the mean because of OS scheduling on a desktop. The
supported workload we state is **up to 8 simultaneous voices per callback** on
hardware like this, which keeps the mean near 5% of one core; more is possible but
should be measured on the target. Planning and preparing a 90-character line takes
about 30 µs.

## Listening

Run `cargo run --release --example voices --features voice` for the fixtures: five
held vowels with smooth and saw sources, an articulated phrase, each robot waveform
with a question and a statement, the five characters speaking the same line (once at
their own pitch and once all at 155 Hz without pitch modulation), and rigid versus
expressive delivery for the robot and soft-rounded voices. Every sound has a raw WAV,
a copy scaled to RMS 0.05 (`-rms.wav`, a comparison control, not equal loudness), the
expanded `.voice` profile and a row in `report.csv`.

History of owner listening, which is the only basis for quality claims here:

- 2026-10-29, October prototype (never merged): the first parallel-formant AEIOU
  probe was "too robotic"; the expressive probe "sounds good"; production examples,
  robot pulse/triangle examples and the five-character comparison were approved.
- 2026-11-20, this implementation: the DSP was rebuilt from the issue specifications
  because the prototype's code was lost, so those approvals do not carry over. The
  fixtures above are ready for listening; findings will be recorded in #51 and #118.

See [voice-human-like-study.md](voice-human-like-study.md) for how far toward a
human-like timbre this engine is expected to go.

## Out of scope

Natural speech, pronunciation, dictionaries, language detection, ML, voice cloning,
recordings, a vocoder, device output, plugin formats and a dialogue editor.
