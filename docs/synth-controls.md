# Held-note controls (0.5 development, #68)

Synth controls are an opt-in, typed plan of source-frame events. They change
supported values on existing voices, preserving oscillator phase, filter history,
ADSR, glide and LFO/macro age. Existing Score text and uncontrolled PCM are retained.
Arrangement syntax and source-position diagnostics remain Studio responsibilities.

## API and scope

```rust
use audiobox::synth::{ControlCurve, ControlEvent, ControlParameter, ControlTarget};

// `synth` is a fresh Synth, including one built with Synth::from_frames_stereo.
let target = ControlTarget::Instrument { track: 0, instrument: 0 };
let sweep = ControlEvent {
    ramp_frames: 24000,
    curve: ControlCurve::Exponential,
    ..ControlEvent::new(48000, target, ControlParameter::CutoffHz, 6000.0)
};
synth.prepare_controls(&[sweep], 64)?;
```

`prepare_controls(events, capacity)` validates and installs a whole plan before
the first fill or after reset. Events must be frame-ordered, with equal frames in
input order. `schedule_control(event)` inserts a future event between fills under
exclusive source ownership, preserving previous IDs. It may allocate/recompile;
both methods belong on the control thread. They are not callback-safe or a shared
concurrent handle. An insertion also revalidates affected future exponential ramps.
Any failure preserves the existing renderer, queue, pending events and DSP state.

`ControlId` is a submission index within one plan. Initial events have IDs matching
input indices; later insertions do not renumber them. Reset replays the same IDs.
Replacing a plan starts a new ID space. `poll_control_event()` yields one bounded
`ControlReport` per executed event, with actual source frame and `Applied` or
`TargetInactive`. It allocates/frees nothing. Reports are accessible while the
caller owns Synth; moving it into a mixer Stream adds no shared control/status
handle. Prepare the automation before handing that source to the mixer.

`control_value(target, parameter)` returns the currently applied controller at the
next source frame (`frames_rendered()`), before queued events at that frame run.
It evaluates an existing ramp at that cursor; it does not execute pending events.
Until a lane's first event it returns the instrument/base value. Note queries
require an active note. These are unrounded f64 controller units, not rendered
level measurements or final modulated/clamped filter coefficients.

## Capability table

| Continuous parameter / parser name | Unit and inclusive range | Supported source |
| --- | --- | --- |
| `Expression` / `expression` | Linear multiplier 0..1; base 1 | All instruments, including chip/drums |
| `Pan` / `pan` | Linear pan -1..1; overrides base pan before pan LFO | All instruments |
| `CutoffHz` / `cutoff-hz` | Base Hz 1..96000 | Synthwave with a configured filter |
| `ResonanceQ` / `resonance-q` | Q 0.5..10 | Synthwave with a configured filter |
| `PitchCents` / `pitch-cents` | Additive oscillator offset -2400..2400 cents; base zero | Synthwave |
| `PitchLfoDepth` / `pitch-lfo-depth` | 0..2400 cents | Synthwave with that LFO configured |
| `CutoffLfoDepth` / `cutoff-lfo-depth` | 0..9600 cents | Synthwave with that LFO/filter configured |
| `AmplitudeLfoDepth` / `amplitude-lfo-depth` | 0..100 percent | Synthwave with that LFO configured |
| `PanLfoDepth` / `pan-lfo-depth` | 0..100 percent | Synthwave with that LFO configured |

The strong enum includes these controls only. Its `FromStr` resolver accepts the
listed exact names and returns `InvalidArgument` for unsupported or structural
selectors such as voices, unison, waveform, filter mode, envelope stages or LFO
rate. An existing instrument without the required filter/LFO rejects the control;
it does not create the missing structure on audio. Continuous pitch/filter/LFO
controls do not apply to chip/wavetable/drum sources, including a wavetable that
borrows a voice pool. Expression and pan remain available there.

At note boundaries callers can choose a different existing instrument, pitch,
volume or velocity through FrameNote / Score. Waveform, voice/unison count,
filter type, ADSR, sample/table data and LFO topology/rate belong in that prepared
instrument. Live structural edits and arbitrary modulation graphs are unsupported.

Expression scales the existing voice output independently of authored velocity,
instrument/channel volume and ADSR; zero expression still advances DSP state.
Pan follows synthwave's linear balance: center preserves both sides and a full
side attenuates the other to zero. Mono rendering averages those sides, so a full
pan attenuates its mono fold. The mixer's separate panning law remains unchanged.

Cutoff controls precede filter-envelope/LFO/keytracking modulation and the existing
1..0.45*sample_rate Hz clamp. Resonance changes damping without resetting filter
history. Pitch controls add an oscillator offset after the existing glide and
pitch modulation; the filter retains its existing base-note/glide keytracking
policy. LFO depth changes retain phase/rate, including the tempo-map clock in #67.
There is no automatic limiting or new headroom guarantee for arbitrary render gain.

## Targets, precedence and event order

- `Instrument { track, instrument }` affects matching held/releasing voices and
  future notes on that track. That instrument must be used by at least one authored
  note there. Another track using the same instrument has independent controls.
- `Note { track, note }` identifies an authored note by its index in the track's
  `Score::channels()[track].notes()` / `FrameScore::tracks` list. Its event must start inside the
  authored gate `[on, off)`. If still active, its lane overrides that parameter's
  instrument lane, including during release. Other parameters continue to inherit.
- If the allocator has stolen/replaced the note or its one-shot has already
  finished, the event reports `TargetInactive`, changing no other lane or voice.
  There is no fallback to whichever note now occupies its slot. Missing authored
  track/instrument/note indices fail during preparation/submission instead.
- On each source frame, ordinary note-on/gate-off scheduling runs first, then
  controls in submission order, then DSP. Thus a note control at its onset reaches
  the newly triggered note. Later same-frame events on one lane retarget earlier
  ones; note-lane precedence over instrument lanes is always explicit.
- A fresh authored note clears the reused slot's note lanes, even for mono legato;
  persistent instrument lanes remain. A tie represented as one extended FrameNote
  retains its note lanes through a section/tempo boundary. No control retriggers
  an attack or resets filter/oscillator/LFO/macro state.

## Ramp and clock contract

Frames are relative to the Synth source's frame zero. `frame` is the first affected
frame. Zero `ramp_frames` steps to the target immediately, then holds it. A positive
N-frame ramp starts at the currently effective value on frame S, and reaches the
exact target on S+N. Another event samples that effective value at its own start
and retargets from it. A note lane initially inherits its instrument's value at S,
then overrides it until that authored note ends. There is no hidden smoothing.

Linear ramps interpolate `from + (to-from)*t`. Exponential ramps interpolate
geometrically with deterministic engine log2/exp2, and require strictly positive
source/target endpoints, including an explicitly exponential zero-frame event.
The calculation uses separate endpoint logarithms, avoiding overflow in `to/from`
even for the smallest positive f64 expression. Intermediates clamp to the endpoint
range; exact endpoints bypass approximation. Nonpositive exponential endpoints
fail instead of silently selecting a linear/hold curve. This exponential curve is
distinct from the mixer's `FadeCurve::Exponential` easing shape.

All event values are finite and range-checked. Supported ranges are convex;
linear/geometric interpolation and endpoint clamps keep every intermediate valid.
The existing final filter modulation clamp still applies. Scheduling/retarget
timing follows the mixer's absolute-frame approach, with no repeated duration
rounding. For musical positions use TempoMap on both absolute ramp boundaries and
subtract their mapped frames; tempo changes then affect authored durations as in
[prepared timing](prepared-synth-timing.md). LFO phase uses that musical clock;
control ramps themselves use the resulting fixed source frames.

Events must start before the exclusive body end and ramps must end by it. No new
events start in appended release. Final controller values remain during release;
an active note's ramp can continue into its earlier release while the body remains.
The target at the exclusive body end is observable in output only if a tail still
renders that frame. Controllers add no leading delay, frames, padding or retiming.
Implicit finite/endless loops are rejected when controls are enabled; expand
authored repeats to absolute events through the prepared timing path.

## Capacity, errors and replay

Capacity is explicit, at most 65536 events and results. `controls_len()` counts
**retained** entries, including executed ones; polling results does not free event
capacity. This finite plan retains history for allocation-free reset/replay. A full
plan returns `LimitExceeded`; reset retains its events, while preparing a replacement
after reset can choose a new capacity. Empty events with capacity zero disable it.

Invalid values, capabilities, targets, order, past/finished scheduling, body/gate
bounds or exponential endpoints return `InvalidArgument`. Excess capacity and u64
ramp-end overflow return `LimitExceeded`. Invalid future insertion cannot break a
previously queued exponential ramp or alter any accepted state. The initial plan
can be installed only before playback or after reset. Queries/results do not make
live edits to a source already owned by a concurrent mixer.

Preparation/submission may allocate: bounded event/report vectors, per-track/
instrument lane storage, one fixed lane row per voice slot, and temporary ordered
history for curve validation. Memory grows with capacity and the existing bounded
tracks/instruments/voice slots, never callback duration. Queue/report reservations
are fallible; other workspaces use normal Vec/BTreeMap allocation under those caps.
Clone preserves reserved callback capacity. `fill`, `reset` and polling allocate
nothing, take no locks and perform no I/O. Playback work is bounded by source pools,
nine parameters per voice and the due events; same-frame bursts still cost work.
No universal callback deadline or inaudibility guarantee is asserted.

## Verification

`tests/synth_controls.rs` covers sample-by-sample expression/pan timing against an
uncontrolled held carrier within 1e-7 amplitude, geometric midpoint/overlap controls
within 1e-14 in f64 units, subnormal endpoints, future insertion/revalidation,
atomic rejection/full capacity, note precedence/inactive reports and replay.
Eight rates from 1 to 768 kHz sweep cutoff 1..96000 Hz, Q 0.5..10, pitch +/-2400
cents and every LFO depth to its maximum. Those finite synthetic fixtures require
absolute sample values and adjacent-frame steps below 64 at render gain 0.125;
this deliberately loose numerical ceiling is not a click/quality assessment.

A private held-note control checks exact filter/oscillator state at ramp start,
retained envelope/note ages and no retrigger while cutoff/expression ramps run.
The public automated PCM/report fixture is pinned across native/WASI, with
whole/irregular chunks/reset equivalence and allocation counters including cloned
sources. Existing Score, prepared timing, chip, synthwave and wavetable hashes
remain compatibility controls. Listening complements numerical checks; none of
these tests automatically labels a transition inaudible.

Run the example for a held stereo note with a cutoff/expression sweep across a
fractional-tempo change:

```text
cargo run --release --example synth_controls --features synth
```

It writes `target/synth-controls.wav` (or a supplied path) and prints event results
and the actual body/release frame counts.
