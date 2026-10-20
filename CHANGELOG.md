# Changelog

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
