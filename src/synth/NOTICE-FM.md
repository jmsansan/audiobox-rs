# FM voice source provenance

The FM voice contract in `docs/fm-voice.md` and the renderer in `src/synth/fm/voice.rs`
take their conversion curves from
*music-synthesizer-for-android* (msfa), Copyright 2012 Google Inc.,
https://github.com/google/music-synthesizer-for-android at revision
`f67d41d313b7dc85f6fb99e79e515cc9d208cfff`, under the Apache License 2.0
reproduced in `LICENSE-MSFA-APACHE`. No Cargo dependency, FFI or build-time
download is introduced, and no msfa source file is included.

What is taken, and where it is used:

| From (under `app/src/main/jni/`) | Taken | Used in |
| --- | --- | --- |
| `env.cc` | Output-level table, level and rate formulas, attack shape | `docs/fm-voice.md`, `src/synth/fm/voice.rs` |
| `dx7note.cc` | Velocity table, keyboard level and rate scaling formulas and exponential table, detune constant, pitch sensitivity table | `docs/fm-voice.md`, `src/synth/fm/voice.rs` |
| `pitchenv.cc` | Pitch envelope level and rate tables | `docs/fm-voice.md`, `src/synth/fm/voice.rs` |
| `lfo.cc` | LFO speed and delay formulas, wave shapes and start phase | `docs/fm-voice.md`, `src/synth/fm/voice.rs` |
| `fm_op_kernel.cc` | Full-scale modulation depth and feedback scaling | `docs/fm-voice.md`, `src/synth/fm/voice.rs` |
| `patch.cc` | Packed bank voice layout | `docs/fm-voice.md`, `src/synth/fm/dx7.rs` |
| `fm_core.cc` | Algorithm bus flags, used only to check the routing table | `src/synth/fm.rs`, `ALGORITHMS` |
| `wiki/Dx7Envelope.wiki`, `wiki/Dx7Hardware.wiki` | Units and envelope description | `docs/fm-voice.md`, `src/synth/fm/voice.rs` |

Changes: the formulas are restated in prose in the contract and reimplemented in
floating point in the renderer, in the engine's units and independent of the render
rate; the level, pitch-level and pitch-rate tables are copied as data. The routing table in `fm.rs` is written in this
crate's own representation from the instrument's algorithm chart. The types, text
form, parser, limits and engine extensions are original to audiobox.

SHA-256 of the upstream files consulted:

```text
bcd52b05c6d316f413088b6ff31486c491abd9ad4a114d0f3309bb555b8fdf24  dx7note.cc
83bf4debc5ea480466892ccf572adcc27b55ee2ac3c21979485e6fa73e5b9ccc  env.cc
5039002970c22183944aa9719b3342231ceee441244b8a7dfdf039d2fafaa07d  fm_core.cc
47a34fc38eab4cfc46376732a6ddfd24631b8ef27a67ec4177300a264e5ff867  lfo.cc
da4d85e5957f103e9aae7418fc803ecc61933b42c1d4add67f4ea1dcc8b597e3  pitchenv.cc
e91aa1e067bf67c799871344eb1f3cdd92c2c0e4797b2efc2403ac7bb7113af5  fm_op_kernel.cc
b4a70d7812052dc52621c2d80f480dcc17b6ff0044cc6e215caae88df41d07b7  patch.cc
```

## LFO amplitude modulation

The form of the LFO's amplitude modulation (linear gain reduced by depth times
sensitivity over three) follows `src/lfo-dx7.js` of *dx7-synth-js* by Matt Montag,
https://github.com/mmontag/dx7-synth-js at revision
`f269f0e02fc67b2f824b01a8416339cd5c4829e0` (SHA-256
`8f54d450bf14ebc85ca37c2aafb3120b668568038dfcc5c5721c1762be34e7a6`), whose
`package.json` declares the ISC licence. No code or table is copied; the formula is
restated in `docs/fm-voice.md` and implemented in `src/synth/fm/voice.rs`, with the
depth held at zero during the LFO delay.

```text
ISC License

Copyright (c) Matt Montag

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT,
OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA
OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```
