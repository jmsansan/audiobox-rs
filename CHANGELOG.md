# Changelog

## Unreleased

### Added

- Mixer voices have a playback rate and a pitch offset, set when they start and
  changed while they play with a ramp (`Play::rate`, `Play::pitch`,
  `MixerHandle::set_rate`, `set_pitch`), and seeded random variation of pitch and
  gain for repeated sounds (`Variation`, `MixerConfig::seed`, `MixerHandle::reseed`).
- Pause and resume mixer voices and buses, with optional fades (`pause`, `resume`,
  `pause_bus`, `resume_bus`); voices on a paused bus keep still, and others play on.
- Start a voice partway through its sound (`Play::start`), and read a voice's state
  and position without blocking (`MixerHandle::voice_status`).
- Instance limits per sound (`Play::instances`, `Instances`, `OverLimit`): a maximum
  number of voices at once with a policy for one more, and a minimum gap between
  starts. Refused starts report `MixerEvent::Limited`.
- Pan and distance fade from a 2D position relative to the listener
  (`Play::position`, `MixerHandle::set_position`, `Space2d`, `Distance`,
  `MixerConfig::space`).

### Breaking

- `MixerEvent` is `#[non_exhaustive]` and gains `Limited`; a `match` on it needs a
  wildcard arm.

### Changed

- The mixer's commands and finished voices travel through a bounded
  single-producer, single-consumer queue of the crate's own instead of std's
  channel, so `render`'s real-time guarantees rest only on documented std behavior.

## 0.2.1

### Changed

- Crate metadata for discovery: a description that covers game audio, `gamedev` and
  `synthesizer` keywords, the `multimedia::encoding`, `game-development` and `wasm`
  categories, and links to the website and docs.rs.
- A crate overview on the docs.rs front page, and feature badges on the `mixer`,
  `synth` and `sfx` items.
- The README's opening: what the crate does, badges, examples for files and games,
  and when to use it or another crate.

### Website

- A playground that generates sound effects from presets and seeds, with every
  parameter on a slider and its text form, and renders chiptune scores with errors at
  their line and column. Both run this crate through WebAssembly and export WAV.
- A dark theme, now the default, with a toggle that remembers the choice.
- Guide sections on incremental decoding and game audio, and the API reference with
  every feature.

## 0.2.0

Every addition keeps the crate free of dependencies and `unsafe`. New modules sit
behind optional features, so a project compiles only what it uses.

### Added

- `stream::Decoder`: incremental decoding of WAV, FLAC and MP3 held in memory, with
  seeking, bounded memory and allocation-free reads. Its output equals `decode`
  sample for sample.
- `synth` feature: an 8-bit style synthesizer with a text score format, offline and
  allocation-free incremental rendering, and deterministic output.
- `sfx` feature: a parametric sound-effect generator with a round-tripping text form
  and seeded presets.
- `mixer` feature: a real-time mixer for games with shared and streaming voices,
  buses, fades, loops, constant-power pan, resampling, a voice limit with priorities,
  sample-accurate scheduling, and an allocation-free, lock-free `render`.
- `ErrorCode::Syntax` for malformed text.

### Changed

- FLAC MD5 verification and MP3 decoding no longer allocate per frame. Decoded samples
  are unchanged.

## 0.1.0

First Rust release.
