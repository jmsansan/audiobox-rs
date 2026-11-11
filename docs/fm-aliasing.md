# FM aliasing: measurements and policy (#144)

Phase modulation has no band limit. A bright patch on a high note produces partials
above half the sample rate, and those fold back as tones that do not belong to the
note. This file records how much, what was tried, and what the engine does.

## Method

`examples/fm_aliasing.rs` holds one note per patch and measures the steady tone.
Every patch uses whole or half frequency ratios, so its true partials lie on
multiples of half the note's frequency. Energy between those lines is folded energy.
The figure is that energy below 0.4 of the sample rate (19.2 kHz at 48 kHz),
relative to the whole signal, from a Blackman-Harris window over 65536 samples.
About -90 dB is the method's floor: the sine rows show it.

- **44.1, 48, 96 kHz:** the patch rendered at that rate.
- **48 kHz, ideal 2x:** rendered at 96 kHz and resampled with the crate's
  resampler, as an offline render could do for any score.
- **48 kHz, `oversample=2`:** the instrument's own option at 48 kHz.

```sh
cargo run --release --features synth --example fm_aliasing
```

## Results

| Patch | Note | 44.1 kHz | 48 kHz | 96 kHz | 48 kHz, ideal 2x | 48 kHz, `oversample=2` |
| --- | --- | --- | --- | --- | --- | --- |
| sine | C2 | -92 dB | -89 dB | -88 dB | -89 dB | -89 dB |
| sine | C4 | -92 dB | -92 dB | -88 dB | -92 dB | -92 dB |
| sine | C5 | -88 dB | -88 dB | -92 dB | -88 dB | -88 dB |
| sine | C6 | -93 dB | -93 dB | -88 dB | -93 dB | -93 dB |
| sine | C7 | -88 dB | -88 dB | -93 dB | -88 dB | -88 dB |
| bright bell | C2 | -91 dB | -91 dB | -89 dB | -91 dB | -91 dB |
| bright bell | C4 | -91 dB | -91 dB | -90 dB | -91 dB | -91 dB |
| bright bell | C5 | -92 dB | -92 dB | -91 dB | -92 dB | -92 dB |
| bright bell | C6 | -26 dB | -28 dB | -92 dB | -91 dB | -91 dB |
| bright bell | C7 | -4 dB | -4 dB | -28 dB | -49 dB | -49 dB |
| bright bell, scaled | C2 | -91 dB | -91 dB | -89 dB | -91 dB | -91 dB |
| bright bell, scaled | C4 | -91 dB | -91 dB | -91 dB | -91 dB | -91 dB |
| bright bell, scaled | C5 | -89 dB | -89 dB | -90 dB | -89 dB | -89 dB |
| bright bell, scaled | C6 | -91 dB | -92 dB | -88 dB | -92 dB | -92 dB |
| bright bell, scaled | C7 | -58 dB | -58 dB | -93 dB | -89 dB | -89 dB |
| feedback bass | C2 | -89 dB | -90 dB | -90 dB | -90 dB | -90 dB |
| feedback bass | C4 | -89 dB | -89 dB | -90 dB | -89 dB | -89 dB |
| feedback bass | C5 | -90 dB | -90 dB | -89 dB | -90 dB | -90 dB |
| feedback bass | C6 | -89 dB | -89 dB | -90 dB | -89 dB | -89 dB |
| feedback bass | C7 | -84 dB | -88 dB | -89 dB | -90 dB | -90 dB |
| six-carrier organ | C2 | -90 dB | -90 dB | -91 dB | -90 dB | -90 dB |
| six-carrier organ | C4 | -87 dB | -89 dB | -89 dB | -89 dB | -89 dB |
| six-carrier organ | C5 | -61 dB | -60 dB | -89 dB | -89 dB | -89 dB |
| six-carrier organ | C6 | -44 dB | -42 dB | -60 dB | -89 dB | -89 dB |
| six-carrier organ | C7 | -24 dB | -24 dB | -42 dB | -54 dB | -54 dB |
| electric piano | C2 | -88 dB | -89 dB | -90 dB | -89 dB | -89 dB |
| electric piano | C4 | -88 dB | -88 dB | -91 dB | -88 dB | -88 dB |
| electric piano | C5 | -86 dB | -88 dB | -88 dB | -91 dB | -91 dB |
| electric piano | C6 | -37 dB | -40 dB | -88 dB | -88 dB | -88 dB |
| electric piano | C7 | -90 dB | -90 dB | -40 dB | -61 dB | -61 dB |

The patches: a bell with a modulator at 4.5 times the note and level 85; the same
bell with right-hand keyboard scaling (`scaling=39,0,25`); a two-operator bass with
feedback 5; six carriers at ratios 1 to 8 with feedback 4; and an electric piano
with a modulator at 14 times the note. The piano's C7 row at 44.1 and 48 kHz is
clean only because that modulator is above 0.45 of the rate and so is silent.

## What the numbers say

- Up to C5 nothing needs doing: every patch is within a few dB of the floor.
- From C6 a bright patch folds audibly: -28 dB for the bell, -40 dB for the piano,
  and the bell at C7 is mostly folded energy.
- Rendering at 96 kHz moves the problem up one octave.
- Oversampling by two removes it at C6 and leaves -49 to -61 dB at C7.
- Keyboard scaling on the modulator, which is how DX7 patches are normally
  written, takes the bell from -4 to -58 dB at C7 at no cost, by making the top
  notes less bright.

## Options compared

| Option | Benefit | Cost |
| --- | --- | --- |
| Nothing | As the instrument: clean to C5 | None |
| Keyboard scaling in the patch | -58 dB or better in the example, by the author's choice of tone | None at run time |
| `oversample=2` | Folded energy below 0.4 of the rate gone at C6, 45 to 60 dB lower at C7 | About twice the voice's cost, and 12 frames of delay |
| Limiting a modulator's level by note automatically | Like scaling, but hidden from the author | None at run time; changes imported patches silently |
| Render the score at twice the rate and resample | Like `oversample=2` for every instrument | Twice the whole render; offline only |

Automatic limiting was not built: it does what keyboard scaling already does, but
without telling the patch's author, and an imported voice would no longer follow its
own data.

## Policy

1. **Default: no mitigation.** An operator whose own frequency is above 0.45 of the
   rate is silent; sidebands may fold. This matches the instrument, costs nothing,
   and is clean for the notes most parts play.
2. **`oversample=2` on the `fm` line is opt-in per instrument.** Use it on bright
   patches that play from about C6 upwards and are not tamed by keyboard scaling.
   Starting presets that need it carry it.
3. **The patch author manages the rest** with keyboard level scaling on modulators.

The owner's decision was "on by default if the measurements justify it". They
justify it for a minority of notes, and it would roughly double the cost of every
FM note and add 12 frames of delay, so it is off by default and one key away.

## How `oversample=2` works

The operator loop runs twice per output frame with half the phase step. Envelopes,
the pitch envelope and the LFO still advance once per frame. The two results pass
through a 49-tap Kaiser-windowed half-band low-pass (flat to 0.4 of the output
rate, about 80 dB down from 0.6) and one value is kept. What folds from the
transition band lands above 0.4 of the rate.

- **Delay:** `FM_OVERSAMPLE_DELAY`, 12 frames (0.25 ms at 48 kHz), relative to the
  same patch without it and to every other instrument.
- **Feedback** runs at the doubled rate, so a patch with feedback sounds slightly
  brighter with `oversample=2`, as it does when the whole score is rendered at
  twice the rate. Without feedback the passband matches within 0.2 %.
- An operator may now run up to 0.9 of the output rate before it is silenced.
- Rendering still allocates nothing and does not depend on chunk size.

## Cost

In the development container, `synth/fm-6op` (two tracks of eight six-operator
voices) ran at about 11 times real time and `synth/fm-6op-2x` at 5.4 to 6.7 times
over two runs:

```sh
cargo bench --bench engine --all-features -- --quick synth/fm-6
```

One noisy machine; tables on named hardware belong to #148.

## Not done

The optional "vintage" mode from this issue was dropped by the owner on 2026-11-11
(see #137): the target is a clean sound.
