# How human-like can procedural gibberish sound? (#118)

**Question.** How close can the 0.6 procedural voice engine get to a natural,
human-like *timbre* while producing invented, unintelligible syllables? Intelligible
pronunciation is explicitly not a goal (owner, 2026-10-29).

**Status.** Engine side complete; owner listening of the 0.6 fixtures pending. The
technical part of the decision is below. The perceptual part (which variants are
useful, which remain too robotic) is only recorded from the owner's listening, never
from waveform metrics or an assistant's impression.

## What the engine can vary

Everything goes through the single 0.6 renderer ([voice.md](voice.md)); this study
adds no DSP path.

| Axis | Control | Range |
| --- | --- | --- |
| Excitation shape | `source=smooth` (glottal-flow derivative) vs `source=saw` | |
| Open phase | `open` | 0.3..=0.9 |
| Spectral tilt | `tilt` | 0..=1 |
| Breath | `breath` (aspiration in voiced sound) | 0..=1 |
| Resonance and bandwidth | `resonance`, `bandwidth` | 0.5..=2, 0.25..=4 |
| Micro-variation | `jitter`, `shimmer`, `vibrato` | ≤ 50 cents, ≤ 0.5, ≤ 100 cents |
| Delivery (separate from timbre) | `Delivery::Rigid` / `Expressive` | planner v1 bounds |

## Fixtures that isolate timbre from delivery

`cargo run --release --example voices --features voice` (48 kHz mono, seed 7,
render v1, profile format v1, planner v1). Each has a raw and an RMS-0.05 copy, the
exact `.voice` profile and a `report.csv` row with the applied gain.

| Comparison | Files | Held constant |
| --- | --- | --- |
| Saw baseline vs smooth source | `vowels-saw`, `vowels-smooth` | pitch 155 Hz, timing, vowels, no vibrato/jitter/shimmer |
| Five characters, own pitch | `all-characters`, `character-*` | text, delivery seed |
| Five characters, same pitch | `all-same-pitch` | text, delivery, 155 Hz, no pitch modulation |
| Rigid vs expressive | `delivery-soft-rounded-{rigid,expressive}`, `delivery-robot-*` | profile |
| Articulation | `articulated` | profile |
| Pitched phrase | `music-vowel-harmony` (`voice_music` example) | profile, vowels per chord |

Equal RMS is a comparison control, not equal perceived loudness.

## Measurements

- **Levels** (raw, from `report.csv`): smooth vowels peak 0.40 / RMS 0.134, saw vowels
  0.48 / 0.136; characters peak 0.35 (soft-rounded) to 0.69 (airy-breathy); DC below
  1e-5 everywhere.
- **Cost**: 0.55–0.6% of a Ryzen 7 1700X core per formant voice at 44.1/48 kHz, 0.4%
  for robot voices ([voice.md](voice.md#cost)). The softer variants cost the same as
  the saw baseline: the excitation is a few multiplies per sample.
- **Memory**: one renderer is a fixed-size struct plus shared `Arc`s; nothing grows
  while rendering.
- **Determinism**: all variants are pinned across native, Rust 1.85 and WASI targets
  and are independent of chunking.
- **Low rates**: formants above 0.42 of the rate drop out for the whole utterance. At
  8 kHz a small creature (`resonance` 1.45) keeps only F1 and F2; the
  voice gets duller and less vowel-like. At 16 kHz and above, every preset keeps at
  least F1–F3. Use 22.05 kHz or more for formant voices.

## Decision

**Go, as the 0.6 endpoint:** a bounded procedural palette, from deliberately
robotic (`robot`, saw sources) to softer (`soft-rounded`) and breathier
(`airy-breathy`) synthetic voices, with expressive delivery. These are the most
human-like timbres this design offers, and they cost no more than the robotic ones.

**No-go for human-indistinguishable timbre** with this architecture. A four-formant
cascade with a parametric glottal pulse and white aspiration lacks what makes real
voices sound alive: time-varying source spectra coupled to loudness, nasal and
subglottal resonances, consonant bursts with real spectra, and natural
coarticulation over whole syllables. Adding those parameter by parameter is the path
of full formant synthesizers (Klatt, eSpeak), which document the same naturalness gap
against recording-based systems. More random variation is not naturalness either.

**Smallest justified next step, if the owner's listening says the palette is still
too robotic:** a separately scoped feasibility study of a licensed, prerecorded
syllable bank played through the existing sampler and mixer, with no new runtime
dependency. Nothing is added to the crate without that decision.

## Owner listening record

| Date | Material | Finding |
| --- | --- | --- |
| 2026-10-29 | October prototype parallel-formant AEIOU | "Está bien pero suena muy robotico" |
| 2026-10-29 | October prototype expressive AEIOU | "Suena bien la muestra expresiva" |
| 2026-10-29 | October prototype production, robot and five-character examples | approved ("Si, están bien", "Si, suenan muy bien", "Están bien") |
| pending | 0.6 fixtures listed above | to be recorded in #118 |

The October prototype was never merged and its code was lost, so 0.6 is a new
implementation of the same specification; its approvals describe the direction, not
these files.
