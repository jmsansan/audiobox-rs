# FM presets

Eleven starting patches for the FM instrument, written for this crate. None is copied
or converted from a factory or third-party bank. They are starting points: each entry
says how the sound is built and what to change first.

They have been measured, not yet approved by ear. The owner's listening decides
which stay as they are (#147).

## Using one

```text
fm keys preset=epiano
end

fm bright-keys preset=epiano feedback=6
  op 2 level=70        ; a stronger tine; every other value of operator 2 stays
  lfo amp=0            ; no tremolo
end
```

`preset=NAME` on the `fm` line starts from that patch, wherever it stands on the
line. Every other key on the line, and every `op`, `lfo` and `pitch` line, changes
only the values it names. An `op` line for an operator the preset leaves off starts
that operator from the defaults. `level=0` silences an operator. In code,
`FmPatch::preset("epiano")` returns the patch and `FM_PRESETS` lists the names.

Printing a patch that came from a preset writes it out in full, so a project can
keep either the one-line reference or the whole patch.

## Reading an FM patch

- **Carriers and modulators.** The algorithm decides which operators you hear
  (carriers) and which only shake another operator's phase (modulators). The table
  in [`fm-voice.md`](fm-voice.md#algorithms) lists both for each algorithm.
- **`ratio` sets the kind of tone.** A modulator at a whole multiple of its carrier
  gives a harmonic tone: 1 is saw-like, 2 hollow like a square, 3 and up nasal and
  thin. A modulator off the whole numbers (3.5, 9.1) gives the clang of bells and
  metal. A very high ratio (10 to 14) adds a short "tine" or click.
- **A modulator's `level` is brightness.** About 60 is a touch of colour, 75 clearly
  bright, 85 harsh. Eight units double the effect.
- **A modulator's envelope is how brightness moves.** Falling fast gives a pluck or a
  strike. Rising after the carrier gives brass. Rising slowly gives a pad that opens.
- **`feedback`** turns one operator from a sine towards a saw (1 to 5) and into noise
  (6 and 7 at full level).
- **`velocity` on a modulator** makes harder notes brighter, not only louder. On a
  carrier it makes them louder.
- **`scaling=39,0,N` on a modulator** lowers it towards the top of the keyboard. Use
  it on bright patches: high notes need less modulation and fold badly with too
  much (see [`fm-aliasing.md`](fm-aliasing.md)).
- **`rate-scaling`** shortens envelopes on high notes, as struck instruments do.
- **Level.** Each carrier at `level=99` peaks at 0.5 and carriers add, so patches with
  several carriers set a lower `volume`.

A safe way to edit: find the carriers, change one modulator's `level` or `ratio`,
listen, and compare with the last version before changing anything else.

## The presets

### `epiano`
Algorithm 5: three carrier and modulator pairs. Pair 1 is the tine: a modulator at
14 times the note that dies in a fraction of a second. Pairs 2 and 3 are the body,
detuned three steps either way for width. A light tremolo.
First edits: operator 2 `level` for more or less tine; operators 4 and 6 `level` for
the bark of hard notes; `lfo amp` for the tremolo.
Response: velocity opens all three modulators, so soft notes are round and hard ones
bark. The tine fades towards the top of the keyboard.

### `epiano-soft`
The same structure with the tine at ratio 10 and much lower, softer modulators and a
slower attack. A ballad piano.
First edits: operators 4 and 6 `level` to bring some bite back; `lfo amp`.
Response: gentle; velocity changes level more than tone.

### `bell`
Algorithm 5. Two pairs with modulators near 3.5 times the note, a few steps apart so
they beat slowly, and a third pair two octaves up for the strike. Everything decays
to silence on its own; the note's length hardly matters.
First edits: the `ratio` of operators 2 and 4 (try `3,50` or `7`) for a different
bell; operator 5 and 6 `level` for the strike; operator 1 `rates` for the ring time.
Response: harder notes are brighter. High notes ring shorter.

### `marimba`
Algorithm 5 with two pairs. Operator 2 at ratio 4 is the wood; operator 4 at ratio 10
is the click of the mallet and is gone almost at once.
First edits: operator 2 `ratio` 3 for a hollower bar, 5 or 7 towards a vibraphone
with a longer carrier decay; operator 4 `level` for the click.
Response: velocity mostly changes the click and the first instant. High notes are
much shorter (`rate-scaling=4`).

### `brass`
Algorithm 2: one pair with feedback on the modulator. The modulator rises more slowly
than the carrier, so each note brightens after it starts. Two unison copies and a
vibrato that comes in late.
First edits: operator 2 `level` for the amount of rasp and its first `rates` value
for how fast the note opens; `feedback`; `lfo delay` and `pitch`.
Response: soft notes stay mellow, hard ones blare. Not a quick-note sound: the swell
needs time.

### `bass-round`
Algorithm 5 with two pairs. Operators 1 and 2 are a sub an octave below the note;
operators 3 and 4 the note itself with a soft modulator. Mono with a short glide.
First edits: operator 4 `level` for presence; operator 1 `level` for the weight of
the sub; `glide`.
Response: velocity adds a little edge. Meant for the two octaves below middle C.

### `bass-hard`
Algorithm 2. Operators 1 and 2 are a hard pair with feedback. Operators 3 and 4 add a
second voice an octave down whose modulator at ratio 3 gives a bite on the attack.
First edits: `feedback` and operator 2 `level` for aggression; operator 4 `level`
and second `rates` value for the length of the bite.
Response: velocity changes the tone strongly. Mono with a short glide.

### `organ`
Algorithm 32: six carriers and no modulation, used as drawbars at 0.5, 1, 2, 3, 4 and
6 times the note. A little feedback roughens the highest one. A slight tremolo and
vibrato together suggest a rotary speaker.
First edits: the six `level`s are the drawbars; `feedback`; `lfo speed`.
Response: none to velocity, like an organ. The two highest drawbars fade towards the
top of the keyboard.

### `clav`
Algorithm 2 with both stacks. A bright pair that decays to a hollow tone, and a
second pair whose modulator at ratio 5 adds the pluck.
First edits: operator 2 `ratio` (2 is rounder, 4 thinner); `feedback`; the second
`rates` value of operators 1 and 3 for the length.
Response: velocity changes brightness a lot, which suits rhythmic parts.

### `strings`
Algorithm 2 with both stacks, three unison copies and slow envelopes. The second
stack's modulator at ratio 2 rises over several seconds, so a held chord opens.
First edits: operators 1 and 3 first `rates` value for the attack; operator 4 `level`
and `rates` for how far and how fast the pad opens; `unison-detune`.
Response: velocity is mostly level. Costly: three copies of four operators per note.

### `metal`
Algorithm 5 with every operator at a fixed frequency, chosen not to line up. The
note you play does not change the pitch; it only changes level through the keyboard
response. A gong-like hit or a machine noise.
First edits: any `fixed` value for a different object; `feedback` for more noise; the
carriers' second `rates` value for the length.
Response: much brighter and louder when hit hard. For a pitched metallic sound, swap
the carriers to `ratio`.

## Measurements

One note per preset (C2 for the basses, C5 for the bell and marimba, C4 otherwise)
at velocities 60 and 127, at 48 kHz with a gain of 1. The centroid is the
power-weighted mean frequency, a rough measure of brightness, taken 20 ms into
struck sounds, 0.6 s into the brass and organ and 2 s into the strings.

| Preset | Peak at 60 | Peak at 127 | Centroid at 60 | Centroid at 127 | Seconds |
| --- | --- | --- | --- | --- | --- |
| `epiano` | 0.58 | 0.92 | 271 Hz | 765 Hz | 5.1 |
| `epiano-soft` | 0.59 | 0.97 | 264 Hz | 311 Hz | 6.3 |
| `bell` | 0.36 | 0.70 | 558 Hz | 1033 Hz | 4.0 |
| `marimba` | 0.36 | 0.61 | 527 Hz | 627 Hz | 4.0 |
| `brass` | 0.27 | 0.46 | 273 Hz | 556 Hz | 4.7 |
| `bass-round` | 0.32 | 0.50 | 40 Hz | 49 Hz | 4.2 |
| `bass-hard` | 0.35 | 0.68 | 62 Hz | 114 Hz | 4.2 |
| `organ` | 0.45 | 0.45 | 313 Hz | 313 Hz | 4.0 |
| `clav` | 0.36 | 0.62 | 277 Hz | 661 Hz | 4.0 |
| `strings` | 0.24 | 0.46 | 648 Hz | 630 Hz | 7.8 |
| `metal` | 0.35 | 0.83 | 520 Hz | 806 Hz | 4.0 |

```sh
cargo run --release --features synth --example fm_presets -- out_dir
```

writes this table and one WAV per preset: a phrase played soft, then loud.

## What is not here

- Imitations of particular records or instruments.
- Drums: the engine has drum kits, and FM percussion is in `sfx::FmEffect` presets.
- `oversample=2`: none of these needs it in its intended register. Add it when a
  bright one is played above about C6.
