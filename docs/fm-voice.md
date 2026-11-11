# FM voice contract (#138)

This is the contract for the six-operator FM instrument planned in #137. It fixes the
parameter model, the units, the text form, the limits and the conversion curves, so
that every later issue implements the same thing.

**Status: six operators render.** A score takes `fm NAME` … `end` blocks, and
`Synth` renders all 32 algorithms with feedback, operator envelopes and the pitch
envelope, velocity sensitivity, keyboard level and rate scaling and the LFO, offline
or as a prepared stream, without allocating. Every key of the patch format is
rendered. [Implementation slices](#implementation-slices) says which issue
adds them.

## Decisions

The owner decided these on 2026-11-11 (recorded in #137):

| Question | Decision |
| --- | --- |
| Fidelity | Faithful to the DX7 voice parameters, clean sound, no hardware emulation |
| Units | The instrument's integer ranges ("DX units") are canonical |
| Text form | A multi-line block, one line per operator, closed by `end` |
| Routing | The 32 named algorithms only; no user-defined routings |
| Engine extras | Pan, unison with detune and spread, glide, legato, tempo-synced LFO. No per-voice filter |
| Level | Carriers add as on the DX7, with no normalization by carrier count |
| Sources | Constants may come from permissively licensed projects, with a notice |

## Parameter model

`Fm` is `Copy` and fixed-size, like `Synthwave`, so a prepared renderer needs no
allocation. A patch always has six operator slots. A slot is `None` when the operator
is switched off: it is silent and modulates nothing.

### Voice

| Field | Text key (`fm` line) | Range | Default |
| --- | --- | --- | --- |
| `algorithm` | `algorithm` | 1..=32 | 1 |
| `feedback` | `feedback` | 0..=7 | 0 |
| `key_sync` | `key-sync` | `on`, `off` | `on` |
| `transpose` | `transpose` | -24..=24 semitones | 0 |
| (`FmPatch::volume`) | `volume` | 0..=15, the instrument volume | 15 |
| `voices` | `voices` | 1..=16 | 8 |
| `unison` | `unison` | 1..=8 | 1 |
| `unison_detune` | `unison-detune` | 0..=100 cents | 0 |
| `spread` | `spread` | 0..=100 percent | 0 |
| `pan` | `pan` | -100..=100 percent | 0 |
| `glide_ms` | `glide` | 0..=60000 ms, needs `voices=1` | 0 |
| `legato` | `legato` | `on`, `off`, needs `voices=1` | `off` |
| `oversample` | `oversample` | 1 or 2; see [Aliasing](#aliasing) | 1 |

`voices * unison` is at most 32 (`MAX_FM_UNISON_VOICES`), which bounds one instrument
to 192 operators. #148 may raise or lower this from benchmarks.

| Field | Text key (`pitch` line) | Range | Default |
| --- | --- | --- | --- |
| `pitch_envelope.rates` | `rates` | four values 0..=99 | 99,99,99,99 |
| `pitch_envelope.levels` | `levels` | four values 0..=99; 50 is no offset | 50,50,50,50 |

| Field | Text key (`lfo` line) | Range | Default |
| --- | --- | --- | --- |
| `lfo.wave` | `wave` | `triangle`, `saw-down`, `saw-up`, `square`, `sine`, `sample-hold` | `triangle` |
| `lfo.speed` | `speed` | 0..=99 | 35 |
| `lfo.delay` | `delay` | 0..=99 | 0 |
| `lfo.pitch_depth` | `pitch` | 0..=99 | 0 |
| `lfo.amplitude_depth` | `amp` | 0..=99 | 0 |
| `lfo.key_sync` | `sync` | `on`, `off` | `on` |
| `lfo.pitch_sensitivity` | `pitch-sensitivity` | 0..=7 | 3 |
| `lfo.beats` | `beats` | `N,D` quarter-note beats per cycle, both 1..=64; replaces `speed` | none |
| `lfo.seed` | `seed` | 0..=4294967295, for `sample-hold` | 1 |

### Operator

| Field | Text key (`op N` line) | Range | Default |
| --- | --- | --- | --- |
| `frequency` (ratio) | `ratio=C` or `ratio=C,F` | `C` is `0.5` or 1..=31; fine `F` 0..=99 | `ratio=1` |
| `frequency` (fixed) | `fixed=HZ` or `fixed=HZ,F` | `HZ` is 1, 10, 100 or 1000; fine `F` 0..=99 | — |
| `detune` | `detune` | -7..=7 | 0 |
| `level` | `level` | 0..=99 | 99 |
| `envelope.rates` | `rates` | four values 0..=99 | 99,99,99,99 |
| `envelope.levels` | `levels` | four values 0..=99 | 99,99,99,0 |
| `scaling` | `scaling=B,L,R` | break point, left depth, right depth, each 0..=99 | 39,0,0 |
| `scaling` curves | `curves=LEFT,RIGHT` | `-lin`, `-exp`, `+exp`, `+lin` | `-lin,-lin` |
| `rate_scaling` | `rate-scaling` | 0..=7 | 0 |
| `velocity` | `velocity` | 0..=7 | 0 |
| `amplitude_sensitivity` | `amp-sensitivity` | 0..=3 | 0 |

`ratio=C,F` is the coarse and fine pair as the instrument shows it: the multiplier is
`C * (1 + F/100)`, so `ratio=3,50` is 4.5 and `ratio=0.5,50` is 0.75. The pair is
kept as written because several pairs give the same multiplier and DX7 voice data
stores the pair. `fixed=100,25` is `10^(2 + 0.25)` Hz, about 177.8 Hz.

A break point `B` is MIDI note `B + 21`; the default 39 is MIDI 60, the engine's C4.

## Text form

```text
; An electric piano: three two-operator stacks (algorithm 5).
fm e-piano algorithm=5 feedback=6 voices=8
  lfo wave=sine speed=34 delay=33 amp=20
  op 1 ratio=1  detune=3  level=99 rates=96,25,25,67 levels=99,75,0,0 velocity=2
  op 2 ratio=14           level=58 rates=95,50,35,78 levels=99,75,0,0 velocity=7
  op 3 ratio=1            level=99 rates=95,20,20,50 levels=99,95,0,0 velocity=2
  op 4 ratio=1            level=89 rates=95,29,20,50 levels=99,95,0,0 velocity=6
  op 5 ratio=1  detune=-7 level=99 rates=95,20,20,50 levels=99,95,0,0 velocity=0
  op 6 ratio=1  detune=7  level=79 rates=95,29,20,50 levels=99,95,0,0 velocity=6
end
```

The values above show the form; they are not a tested sound. Presets are #147.

Grammar:

- `;` starts a comment that runs to the end of the line. Blank lines and indentation
  are free.
- The block opens with `fm NAME` and optional `key=value` pairs, and closes with a
  line holding only `end`. `NAME` takes ASCII letters, digits, `_` and `-`.
- Inside, each line is `op N` (N is 1..=6), `pitch` or `lfo`, followed by `key=value`
  pairs in any order. Each of `pitch`, `lfo` and each `op N` appears at most once, in
  any order.
- An operator without an `op` line is switched off. An `op N` line with no pairs is
  an operator at its defaults.
- The algorithm needs an `op` line for at least one of its carriers.
- A key set twice on a line, an unknown key, `ratio` together with `fixed`, text
  after `end` and a block without `end` are errors.
- Text is at most 64 KiB (`MAX_FM_TEXT_BYTES`).

A minimal patch is three lines: `fm sine`, `op 1`, `end`.

Errors are `ParseError`s with the 1-based line and column of the offending value (or
of the key, for an unknown or repeated key), and the usual codes: `Syntax` for
malformed text, `InvalidArgument` for a value out of range, `LimitExceeded` for the
text size and the `voices * unison` bound. Cross-field errors point at the text that
caused them (the `glide` or `legato` key, the `unison` value); a missing carrier and
a missing `end` point at `fm`.

`Display` writes the canonical form: `algorithm` always, `ratio` or `fixed` and
`level` on every operator, and every other key only when it differs from its
default. Parsing that text gives back an equal `FmPatch`, and printing it again
gives the same text.

### In a score

```text
tempo 110
fm bell algorithm=2 feedback=3 voices=4
  op 1 ratio=1    rates=95,28,25,40 levels=99,0,0,0
  op 2 ratio=3,50 level=80 rates=95,38,30,45 levels=99,0,0,0
end
channel melody = @bell o5 l4 c e g {c e g}2
```

- An `fm` block is a top-level directive next to `instrument` lines. Its name shares
  the instrument namespace and is selected with `@NAME`. Errors inside the block
  carry the score's line numbers.
- It becomes `Instrument { fm: Some(Fm), volume, .. }`, also built in code with
  `Instrument::fm`. `fm` excludes `synthwave`, `chip` and `drum`, and the legacy
  fields (`waveform`, `envelope`, `slide`, `vibrato`, `arpeggio`, `duty_sweep`) must
  stay at their defaults.
- FM voices are always pooled: `voices` slots per track, chords up to that many
  notes, the same stealing order as synthwave.
- Existing one-line `instrument` entries are unchanged, and none of their keys gain
  an FM meaning.
- Held-note controls: `Expression`, `Pan` and `PitchCents` apply. `PitchCents` does
  not move fixed-frequency operators. The filter and LFO-depth controls are rejected.

## Algorithms

Operators are evaluated 6 down to 1. In every algorithm a modulator has a higher
number than the operator it modulates, so each input is ready within the same sample;
only the feedback path is delayed. `FmRouting::algorithm(n)` returns this table.

| # | Routing (`a>b`: a modulates b) | Carriers | Feedback |
| --- | --- | --- | --- |
| 1 | 2>1, 6>5>4>3 | 1, 3 | 6 |
| 2 | 2>1, 6>5>4>3 | 1, 3 | 2 |
| 3 | 3>2>1, 6>5>4 | 1, 4 | 6 |
| 4 | 3>2>1, 6>5>4 | 1, 4 | 4 into 6 |
| 5 | 2>1, 4>3, 6>5 | 1, 3, 5 | 6 |
| 6 | 2>1, 4>3, 6>5 | 1, 3, 5 | 5 into 6 |
| 7 | 2>1, 4+5>3, 6>5 | 1, 3 | 6 |
| 8 | 2>1, 4+5>3, 6>5 | 1, 3 | 4 |
| 9 | 2>1, 4+5>3, 6>5 | 1, 3 | 2 |
| 10 | 3>2>1, 5+6>4 | 1, 4 | 3 |
| 11 | 3>2>1, 5+6>4 | 1, 4 | 6 |
| 12 | 2>1, 4+5+6>3 | 1, 3 | 2 |
| 13 | 2>1, 4+5+6>3 | 1, 3 | 6 |
| 14 | 2>1, 5+6>4>3 | 1, 3 | 6 |
| 15 | 2>1, 5+6>4>3 | 1, 3 | 2 |
| 16 | 2+3+5>1, 4>3, 6>5 | 1 | 6 |
| 17 | 2+3+5>1, 4>3, 6>5 | 1 | 2 |
| 18 | 2+3+4>1, 6>5>4 | 1 | 3 |
| 19 | 3>2>1, 6>4, 6>5 | 1, 4, 5 | 6 |
| 20 | 3>1, 3>2, 5+6>4 | 1, 2, 4 | 3 |
| 21 | 3>1, 3>2, 6>4, 6>5 | 1, 2, 4, 5 | 3 |
| 22 | 2>1, 6>3, 6>4, 6>5 | 1, 3, 4, 5 | 6 |
| 23 | 3>2, 6>4, 6>5 | 1, 2, 4, 5 | 6 |
| 24 | 6>3, 6>4, 6>5 | 1, 2, 3, 4, 5 | 6 |
| 25 | 6>4, 6>5 | 1, 2, 3, 4, 5 | 6 |
| 26 | 3>2, 5+6>4 | 1, 2, 4 | 6 |
| 27 | 3>2, 5+6>4 | 1, 2, 4 | 3 |
| 28 | 2>1, 5>4>3 | 1, 3, 6 | 5 |
| 29 | 4>3, 6>5 | 1, 2, 3, 5 | 6 |
| 30 | 5>4>3 | 1, 2, 3, 6 | 5 |
| 31 | 6>5 | 1, 2, 3, 4, 5 | 6 |
| 32 | none | 1, 2, 3, 4, 5, 6 | 6 |

A two-operator voice is not a separate mode: it is algorithm 1 (or 2, to put the
feedback on the modulator) with only operators 1 and 2 written. A four-operator
voice is any algorithm with at most four `op` lines.

The table was typed from the instrument's algorithm chart and checked against the
bus flags in msfa's `fm_core.cc`. `tests/fm.rs` checks carriers and feedback against
a second hand-typed table.

## Conversion curves

These define what the DX units mean. Rendering issues implement them with the
engine's deterministic math (`src/math.rs`) and must not substitute other curves
without updating this file. Levels are in **steps** of 6.0206/256 dB (about
0.0235 dB): 256 steps double the amplitude.

Source: Google's *music-synthesizer-for-android* (msfa), Apache-2.0, revision
`f67d41d313b7dc85f6fb99e79e515cc9d208cfff`, files `dx7note.cc`, `env.cc`, `lfo.cc`,
`pitchenv.cc`, `fm_core.cc`, `fm_op_kernel.cc`, and its wiki pages `Dx7Envelope` and
`Dx7Hardware`. See [`src/synth/NOTICE-FM.md`](../src/synth/NOTICE-FM.md). In the
formulas, `>>` and `<<` are integer shifts and `/` on integers truncates.

### Level

`scale(x)` maps a 0..=99 value to 0..=127: `28 + x` for `x >= 20`, otherwise

```text
0 5 9 13 17 20 23 25 27 29 31 33 35 37 39 41 42 43 45 46
```

Once per note, an operator's base level in steps is

```text
out   = min(127, scale(level) + level_scaling(note))
base  = max(0, 32 * out + velocity_offset(velocity))
```

and the envelope's target for an envelope level `L` is

```text
target(L) = max(16, 64 * (scale(L) >> 1) + base - 4256)
```

Full scale is 3840 steps (`level=99`, `L=99`, no offsets). The operator's linear
gain is `2^((steps - 3840) / 256)`:

- a **carrier** at full scale peaks at `FM_CARRIER_PEAK` (0.5);
- a **modulator** at full scale deviates its target's phase by 2 turns peak
  (4π radians, a modulation index of about 12.6).

### Velocity

For sensitivity `s` and velocity `v` (0..=127), with the 64-entry table `V`:

```text
velocity_offset = ((s * (V[v >> 1] - 239) + 7) >> 3) << 4      steps

V = 0 70 86 97 106 114 121 126 132 138 142 148 152 156 160 163
    166 170 173 174 178 181 184 186 189 190 194 196 198 200 202 205
    206 209 211 214 216 218 220 222 224 225 227 229 230 232 233 235
    237 238 240 241 242 243 244 246 246 248 249 250 251 252 253 254
```

At `s = 0` the offset is 0 at every velocity. At `s = 7`, velocity 127 adds 224
steps (about +5.3 dB), so a level can exceed full scale.

**FM instruments do not apply the engine's linear velocity gain.** Velocity reaches
the sound only through each operator's `velocity`, as on the instrument. A patch
with every `velocity` at 0 plays every note at the same level and tone.

### Keyboard level scaling

With `note` the played MIDI note after `transpose`:

```text
offset = note - break_point - 17
group  = |offset| / 3
depth, curve = right side if offset >= 0, else left side
linear:      amount = (group * depth * 329) >> 12
exponential: amount = (E[min(group, 32)] * depth * 329) >> 15
level_scaling = -amount for -lin and -exp, +amount for +exp and +lin

E = 0 1 2 3 4 5 6 7 8 9 11 14 16 19 23 27 33 39 47 56 66
    80 94 110 126 142 158 174 190 206 222 238 250
```

The result is in units of 32 steps (about 0.75 dB), the same as `scale(level)`.
Keys act in groups of three. As measured on the instrument, the flat zone is not at
the break point's own note (`break_point + 21`) but four semitones lower: notes
`break_point + 15` to `break_point + 19` get no offset.

### Envelope rate

```text
qrate = min(63, ((rate * 41) >> 6) + rate_scaling_delta)
decay slope  = 0.2819 * (1 + 0.25 * (qrate & 3)) * 2^(qrate >> 2)     dB per second
attack slope = decay slope * (17 - floor(current_steps / 256))
```

A falling stage is linear in dB (exponential in amplitude). A rising stage first
jumps to 1716 steps if it is below that, then follows the attack slope, which slows
as the level approaches full scale. Slopes are in real time and do not depend on the
render rate.

Rate scaling, for sensitivity `s`:

```text
x = clamp(note / 3 - 7, 0, 31)
rate_scaling_delta = (s * x) >> 3
```

### Frequency

- Ratio: `C * (1 + F/100)` times the note's frequency, with `C = 0.5` for coarse 0.
- Fixed: `10^(decade + F/100)` Hz. It ignores the note, `transpose`, glide, the pitch
  envelope, the pitch LFO and `PitchCents` controls.
- `detune`: 0.9017 cents per step in ratio mode (`12606 / 2^24` octaves). msfa notes
  that the hardware value varies slightly with pitch; this contract uses the constant.
  In fixed mode `detune` has no effect.

DX7 voice data can hold coarse 4..=31 in fixed mode, which the hardware treats as
`coarse & 3`. Import (#145) stores the reduced decade, so such a voice exports with
the reduced value. It sounds the same; this is the one field that is not
byte-identical on a round trip.

### Feedback

The phase of `feedback_to` is offset by the mean of the last two outputs of
`feedback_from`, scaled by `2^(feedback - 8)`, with outputs measured in turns of
phase (full scale is 2 turns). `feedback=0` is off. `feedback=7` at full scale is
1 turn peak. The two-sample mean is the instrument's damping against oscillation at
half the sample rate.

### Pitch envelope

Level `L` maps through the table `P` to `P[L] / 32` octaves (50 is 0; 99 is about
+3.97 octaves; 0 is -4). Rate `R` maps through `Q` to a slope of `Q[R] / 21.3`
octaves per second, linear in pitch. Stages run as in an operator envelope.

```text
P = -128 -116 -104 -95 -85 -76 -68 -61 -56 -52 -49 -46 -43 -41 -39 -37 -35 -33 -32 -31
    -30 -29 -28 -27 -26 -25 -24 -23 -22 -21 -20 -19 -18 -17 -16 -15 -14 -13 -12 -11
    -10 -9 -8 -7 -6 -5 -4 -3 -2 -1 0 1 2 3 4 5 6 7 8 9
    10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29
    30 31 32 33 34 35 38 40 43 46 49 53 58 65 73 82 92 103 115 127

Q = 1 2 3 3 4 4 5 5 6 6 7 7 8 8 9 9 10 10 11 11
    12 12 13 13 14 14 15 16 16 17 18 18 19 20 21 22 23 24 25 26
    27 28 30 31 33 34 36 37 38 39 41 42 44 46 47 49 51 53 54 56
    58 60 62 64 66 68 70 72 74 76 79 82 85 88 91 94 98 102 106 110
    115 120 125 130 135 141 147 153 159 165 171 178 185 193 202 211 232 243 254 255
```

### LFO

```text
sr = 1 if speed == 0, else (165 * speed) >> 6
sr = sr * 11 if sr < 160, else sr * (11 + ((sr - 160) >> 4))
frequency = sr * 25190424 / 2^32   Hz        (speed 0: 0.065 Hz; 35: 5.8 Hz; 99: 23.9 Hz)
```

With `beats=N,D` the cycle is `N/D` quarter notes on the score's tempo map instead,
as `LfoRate::Beats`.

Delay, with `a = 99 - delay`: at `a == 99` there is none. Otherwise

```text
a  = (16 + (a & 15)) << (1 + (a >> 4))
hold = 85.25 / a                      seconds with no modulation
ramp = 85.25 / max(128, a & 0xff80)   seconds rising linearly to full depth
```

The wave `w` runs from 0 to 1 over a phase `p` in turns:

| Wave | `w(p)` |
| --- | --- |
| `triangle` | `2p` up to one half, then `2 - 2p` |
| `saw-down` | `0.5 - p` up to one half, then `1.5 - p` |
| `saw-up` | `p + 0.5` up to one half, then `p - 0.5` |
| `square` | 1 in the first half, 0 in the second |
| `sine` | `0.5 + 0.5 sin(2 pi p)` |
| `sample-hold` | a new value in 1/256..=1 each cycle |

With `sync=on` the LFO restarts at each note that attacks, half a cycle in (`p = 0.5`),
as the instrument does; a legato note does not restart it. With `sync=off` the phase
is `frames * frequency` from the synth's frame zero, so it is the same for every
voice and does not depend on when a note starts. The delay always counts from the
note's attack, with `fade` running from 0 to 1 over the ramp.

Pitch: the peak deviation in octaves is `((pitch * 165) >> 6) * S[pitch_sensitivity]
/ 65536`, with `S = 0 10 20 33 55 92 153 255` (at most about one octave). The offset
is `peak * fade * (2w - 1)`.

Amplitude: each operator's linear gain is multiplied by

```text
1 - (amp * 0.01) * (amp_sensitivity / 3) * fade * w
```

so the LFO only lowers a level, and `amp=99` with sensitivity 3 reaches 1 % of it.
On a modulator this moves the tone rather than the loudness. msfa at the pinned
revision does not implement amplitude modulation; this form follows
*dx7-synth-js* (ISC), which states it as an approximation and not a measurement of
the hardware. Unlike that project, the depth is zero during the delay.

`sample-hold` values come from a hash of `seed` and the cycle number, so a cycle's
value does not depend on how the output is chunked. The hardware's own sequence is
not reproduced.

## Level and headroom

- One carrier at full scale peaks at 0.5 before `volume`. Carriers add. There is no
  normalization by carrier count, so algorithm 32 with six full carriers peaks at 3.0.
- Unison copies are normalized by their count, as synthwave oscillators are.
- Output is not limited. With velocity sensitivity a carrier can reach about 0.92,
  so the bound for one note is `0.92 * carriers`, before `volume` and the score gain.
  This is the same policy as synthwave resonance: the patch author manages level.
- `volume` (0..=15) and channel volume apply as for every instrument.

## Determinism and state

- Phases are kept in turns and sines come from a fixed polynomial that uses only
  multiplication and addition. The same score,
  rate and options give the same samples on every host and for every chunk size.
- `key-sync=on`: every operator phase is zero at note-on. `key-sync=off`: a voice
  slot's phases keep the value its previous note left (zero on first use and after
  `reset`), so the result still depends only on the score.
- Feedback memory is two samples per voice, cleared at note-on with `key-sync=on`
  and kept otherwise.
- Level scaling, rate scaling and velocity are computed once at note-on from the
  authored pitch plus `transpose`, before glide, pitch envelope and LFO.
- The LFO restarts at note-on with `sync=on`; with `sync=off` it follows the synth's
  frame clock like synthwave LFOs. `sample-hold` is a function of `seed` and the
  cycle index.
- Voice allocation and stealing follow synthwave: idle slots first, then the oldest
  releasing, then the oldest held, ties by slot index.

### Envelopes and voices

- A note on an idle slot starts every operator at silence. A note that takes a slot
  still sounding an FM note (stealing, or a mono retrigger) starts each envelope
  from its current level, so there is no click from a reset.
- With `legato=on`, an adjacent full-gate note of the same instrument keeps the
  envelopes, phases and pitch envelope, and only moves the pitch (through `glide`).
- Key-off moves every operator to stage four from wherever it is.
- A voice ends when every switched-on carrier has reached its last level, whatever
  that level is. A carrier whose `levels[3]` is above silence is cut there. A voice
  is also cut 60 seconds after key-off; only the slowest release rates get that far.
- `Synth::max_frames` allows for the longest release of the score's FM instruments.
- An operator level at or below 16 steps (90 dB under full scale) is silent.
- Gains follow the level by multiplication inside a stage and are set exactly at
  each stage boundary; rising stages recompute the ratio once per 256-step band.
- `feedback` 6 and 7 at full level are noise-like rather than saw-like: the loop is
  chaotic there, on this engine as on the instrument. Output stays bounded by the
  operator's own level.

**Open, owned by the issue named:**

- Loop laps: a free-running LFO or operator phase is not restarted at the loop
  point (#136).
- Held-note controls for operator level, feedback and LFO depths (#143).

## Aliasing

Phase modulation is not band-limited. By default the renderer guarantees only that
no operator's own frequency exceeds 0.45 times the sample rate: an operator above it
is silent for that note. Sidebands may fold, as on the instrument. Measured on
representative patches, folded energy is at the measurement floor up to C5 and
becomes audible from about C6 on bright patches.

`oversample=2` on the `fm` line runs the operators at twice the rate and filters
the result down. It removes the folded energy below 0.4 of the rate at C6, costs
about twice as much, and delays the voice by `FM_OVERSAMPLE_DELAY` (12) frames.
Keyboard level scaling on the modulators is the other tool, and costs nothing.
[`fm-aliasing.md`](fm-aliasing.md) has the measurements, the options compared and
the reasons for the default.

## Implementation slices

The types and the text form are final, and every key is rendered.

| Issue | Added |
| --- | --- |
| #138 | Types, validation, text |
| #122, #139, #128, #140 | All six operators and 32 algorithms, feedback, ratio and fixed frequency, detune, levels, operator envelopes, pitch envelope, key sync, transpose, voices, unison, pan, glide, legato |
| #141 | `velocity`, `scaling`, `curves`, `rate-scaling` |
| #142 | The `lfo` line and `amp-sensitivity` |
| #144 | `oversample` |

The four rendering issues landed together because the operator loop is the same for
two, four and six operators: a two-operator voice is algorithm 1 or 2 with two `op`
lines.

- #122's "modulation index" is the modulator's `level`; its "index envelope" is the
  modulator's operator envelope; its "fine detune" is `detune`.
- #128's "small documented set of algorithms" is the table above with at most four
  operators switched on.

Velocity, level scaling and rate scaling are computed once at note-on, from the
authored pitch plus `transpose` and the note's velocity, and held for the note. A
legato note does not attack, so it keeps the values of the note that did.

## Cost

`benches/engine.rs` has `synth/fm-2op` and `synth/fm-6op`: two tracks of eight
voices each. Run `cargo bench --bench engine --all-features -- --quick synth/fm`.
Measured numbers on named hardware belong to #148.
