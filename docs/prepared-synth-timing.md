# Prepared synth timing (0.5 development, #67)

## Design recommendation

Keep the existing integer-tempo `Score` format and its floor-rounded tick timing
as the compatibility path. Extending that type with a tempo map would change its
public construction, parser and existing PCM, and would mix Studio's arrangement
syntax with the engine's rendering contract. Instead, add a typed `FrameScore`
adapter for the existing `Synth`, plus a reusable `TempoMap` for preparing absolute
note-on/off frames. Studio expands sections, repeats and ties before preparation;
multiple bounded Synth streams compose through the existing mixer.

The adapter uses the same instruments, envelopes, oscillator/filter state, voice
allocation and callback as Score. It does not introduce another renderer or raise
the eight-track limit per source. Its event timing uses absolute nearest-frame,
halves-up rounding with 0.001-BPM precision, shared with Music::Transport. A map
accumulates exact reduced rational frame positions across tempo segments; it never
sums rounded note lengths. Checked u128 rational arithmetic rejects combinations
whose exact representation overflows rather than silently changing their timing.

Beat-synced LFOs follow the tempo changes scheduled at their rounded output frames.
Their integrated phase remains continuous, including on held notes and release.
Tick-clock chip macros integrate that same clock relative to the note's first
sample. Millisecond envelopes, glide, free-running LFOs and simulation-frame
macros retain their units. The actual frame-clock phase may differ from ideal
musical time by the sub-frame rounding of a tempo-change boundary; it does not
snap phase to an authored beat there.

## API and absolute positions

```rust
use audiobox::synth::{FrameNote, FrameScore, Instrument, Pitch, RenderOptions,
                      Synth, TempoChange, TempoMap};

let map = TempoMap::new(48_000, 48, &[
    TempoChange { tick: 0, bpm: 126.375 },
    TempoChange { tick: 192, bpm: 111.125 },
])?;
let end = map.frame_at(384)?;
let score = FrameScore {
    instruments: vec![Instrument::default()],
    // A held note crosses the tempo change without a new attack.
    tracks: vec![vec![FrameNote::new(0, end, Pitch::C4, 0)]],
    end_frame: end,
};
let mut synth = Synth::from_frames_stereo(&score, &map, &RenderOptions::default())?;
let mut callback_buffer = [0.0f32; 512 * 2];
let written_frames = synth.fill(&mut callback_buffer);
Ok::<(), audiobox::Error>(())
```

`TempoChange.tick` is an absolute tick and `bpm` counts quarter notes per minute.
TPQ is fixed for one map; meter is caller bookkeeping. The first change must be
at zero and later ticks strictly increase. Finite BPM is 1..=1000, rounded once
to the nearest 0.001 BPM, positive halves upward. `bpm_at(tick)` returns that
normalized value. Supported sample rates are 1000..=768000 Hz and TPQ 1..=960.
Neither map construction nor queries mutate the caller's changes.

For every segment, duration in frames is the exact rational
`delta_ticks * sample_rate * 60000 / (milli_bpm * TPQ)`. Prefixes are added as
reduced checked u128 fractions. `frame_at(tick)` adds that segment's unrounded
offset to its unrounded prefix, then rounds the absolute result once, nearest
frame with halves up. Mapping two note boundaries and subtracting gives a gate
length; summing independently rounded note/segment durations is a different
operation. At 1000 Hz, TPQ 16 and 120 BPM, tick 1 is frame 31 and tick 2 is frame
63, not two separately rounded 31-frame intervals. The fixed-tempo rounding and
BPM normalization functions are shared with Music::Transport.

`FrameNote` contains explicit `on`, exclusive `off`, MIDI `Pitch`, instrument
index, channel volume 0..=15 and velocity 0..=127. Its constructor defaults to full
volume/velocity; the instrument's own volume is multiplied independently. The
adapter does not infer a gate ratio or re-convert these frame values. All notes
require `on < off <= end_frame`; quantization that collapses a gate to zero is
rejected instead of inventing one sample. Map/body frames are relative to source
frame zero, with no added leading delay or audio retiming.

## Ordering, ties and tails

- Track order is explicit. Onsets within a track must be nondecreasing; equal
  onsets retain input order. At each frame the existing scheduler triggers onsets,
  then applies gate-offs to the resulting held voices. Thus adjacent full-gate
  mono-legato notes can retain state. An old voice whose off equals a new onset
  still counts as held during allocation of that onset, matching Score.
- A single legacy voice replaces its previous note, so the last simultaneous
  note wins. Existing synthwave/wavetable voice pools support chords and release
  overlap; the same idle/oldest-release/oldest-held stealing policy and slot tie
  order apply. More simultaneous notes do not enlarge a pool automatically.
- A tie spanning a section or tempo boundary is one `FrameNote` with an extended
  off. No event at that boundary resets oscillator, filter, amplitude envelope,
  glide, note age or macro age. Adjacent same-instrument mono-legato notes retain
  the existing synthwave behavior; an ordinary retrigger remains a retrigger.
- `end_frame` is the exclusive musical body end, including authored trailing
  rests. The synth writes those frames even if no note is sounding. Gate-offs
  cannot extend past it; releases and finite drum tails drain afterward until
  inactive. There is no post-body event triggering. A drum retains its existing
  finite one-shot treatment, including ignoring a gate-off for its sonic tail.
- `max_frames()` is an upper bound: body plus maximum instrument release/drum
  length plus one. `fill` returns the actual frame count, which can be smaller;
  it leaves unused samples/incomplete stereo frames untouched. Silence with no
  body returns zero frames. There is no automatic padding to that upper bound.
- Repeats are expanded to absolute events by the caller. The prepared path accepts
  `RenderOptions.loops == Some(0)` only; implicit finite/endless loops are rejected.
  This keeps tempo maps and held-note policy explicit across authored repeats.
  Legacy Score's loop/carry behavior is unchanged.

## Musical modulation clock

Each authored tempo change takes effect at its `frame_at(change.tick)`. When
several tick boundaries round to one frame, the last authored change wins there.
Beat phase is integrated over these scheduled **frame** intervals with the common
denominator `sample_rate * 60000`. The numerator is the sum of
`elapsed_frames * milli_bpm`. It does not reset at changes, note triggers, rests
or authored repeated sections; Synth reset returns it to zero.

For a beat LFO of period `numerator / denominator` quarter notes, the sine phase
is this integrated beat count multiplied by `denominator / numerator`, modulo
one. A held or releasing note therefore changes modulation rate at the scheduled
tempo boundary without a phase jump. Its physical phase need not land exactly on
the ideal authored tick there: rounding a tempo boundary moves it by up to half
a frame, and continuous phase takes precedence over snapping to that ideal tick.
After the body, the last scheduled tempo continues during release. Free-running
millihertz LFOs still use total rendered frames at their original frequency.

Chip tick macros subtract the integrated beat count at the note's first sample,
then select `floor(relative_beats * TPQ / macro_step_ticks)`. They follow tempo
changes even on a held/releasing note and restart on a normal note trigger. Macro
simulation-frame clocks retain their explicit Hz and note-relative frame age.
Envelope/release/glide/duty/vibrato/arpeggio millisecond settings retain their
existing units. The separate [held-note control API](synth-controls.md) (#68)
adds supported value/depth ramps while preserving these clocks and DSP state.

## Preparation, rendering and limits

Construction validates all arguments before cloning instrument payloads or events.
Hard limits per prepared source are 8 tracks, 256 instruments, 200000 notes and
a 1200-second body. Existing instrument limits apply (at most 16 voices per track,
8 unison oscillators per synthwave voice, 65536 PCM values and bounded macros).
Tempo maps allow at most 1024 changes. Every scheduled change must be within the
body, including its exclusive end. Render gain must be finite. The existing
gain/headroom policy and lack of automatic limiting remain in force.

Invalid ordering, tempos, TPQ, note ranges/indices/settings and implicit
loops return `InvalidArgument`. Unsupported sample rates, excessive resource
counts/duration and checked rational/u64 frame overflow return `LimitExceeded`.
A rejected preparation changes no existing source or input. A map's exact rational
representation is bounded independently of elapsed time: many unrelated
millitempos can exhaust u128 common-denominator/intermediate precision even for
a short song. Such a map/query fails explicitly. There is no silent approximation
or promise to accept every possible 1024-change map. `frame_at` can also fail for
an unrepresentable later query even after successful map preparation.

Map preparation reserves its segment vector fallibly. Instrument/event/voice
storage uses the existing synth's normal Vec allocation behavior under the new
hard caps. Work/storage grow with instrument payloads, tracks, notes and tempo
segments, without a duration-sized audio buffer. A successful renderer owns its
plans, events, fixed pools and map copy; `fill` and `reset` allocate nothing,
take no locks and perform no I/O. The musical clock does one bounded binary lookup
per rendered frame; exact rational conversion remains on the preparation thread.
Callback safety is not a universal wall-clock deadline guarantee for every pool.

Multiple sources use the same map/rate and a common mixer `handle.at(frame).play`
start. Source frame zero then aligns across streams; bus limiter latency or
resampling/rate changes remain the mixer's existing contract. More than eight
musical parts need not enlarge one score or duplicate a DSP scheduler.

## Compatibility and validation

The Score parser, integer tempo, TPQ, floor rounding, gates, loops and legacy
modulation calculations are retained. The adapter's nearest-frame policy is an
explicit new path; converting an old Score with a different rounding/gate rule
need not reproduce its PCM. Both paths share synthesis and voice scheduling code.

`tests/prepared_synth.rs` includes:

- Fifteen separately prepared sources, seven repeated 128-quarter sections and
  13440 notes over more than five minutes at 123.456/98.765/150.125 BPM. A separate
  common-denominator reference checks each onset/off and every audible/silent
  frame, with no accumulated timing drift. A timing/PCM hash pins the result.
- Mapping at 1/8/32/44.1/48/96/192/768 kHz; fixed-tempo agreement with Transport;
  half-frame ties, collapsed boundaries, invalid/overflow/resource controls.
- Held beat-LFO amplitudes checked against an independent sine reference within
  1e-6 amplitude and tick-macro transitions checked against independently integrated
  scheduled frame intervals. Ties/adjacent legato retain exact PCM and release length.
- Legacy, polyphonic/filter/LFO, chip/tick-macro and drum rendering with pinned
  mono/stereo PCM; whole/chunked/reset equivalence at irregular block sizes.
- Fifteen mixer streams at one scheduled frame across blocks 1/7/64/331/4096,
  plus allocation counters around prepared fill/reset and mixer render.

The new fixture joins native and WASI CI beside the existing Score/chip/synthwave/
wavetable/music hashes. These are numerical/determinism controls, not a listening
claim. `examples/prepared_synth.rs` renders the fractional/change/held-note example:

```text
cargo run --example prepared_synth --features synth --release
```

At 48 kHz it emits 194824 body frames and 8640 release frames to
`target/prepared-synth.wav`, or a supplied output path.
