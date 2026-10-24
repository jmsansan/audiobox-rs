# Changelog

## Unreleased

### Fixed

- Whole-file MP3 decoding now grows PCM buffers geometrically within decode limits
  and trims gapless samples in place, avoiding per-frame exact growth and a second
  PCM allocation during trimming. Incremental decoding and decoded sample bits are
  unchanged.

## 0.4.0 — 2026-10-23

### Added

- Opt-in synthwave instruments: normalized/detuned stereo unison, per-voice
  resonant low/high/band-pass SVFs with independent envelopes/key tracking,
  free/beat-synced pitch/cutoff/amplitude/pan LFOs, mono glide and legato.
- Polyphonic chord groups (`{c e g}4`, typed `ChannelWriter::chord`), bounded
  deterministic voice stealing, full chord loop carries/ties, stereo offline
  and mixer-stream rendering, numerical/allocation/native/WASI regression tests,
  bass/pad/lead format examples and a `synthwave` WAV example.
- Optional `events` module: designer-editable named sounds, recorded/SFX/score
  source registration, seeded random/shuffle/sequence selection, gain/pitch ranges,
  bus/priority/loop/instance/repeat controls and per-trigger 2D/3D placement.
- Bounded event parsing with stable codes and line/character columns, preparation
  of every reference, transactional selection on queue errors, shared event limits,
  native/WASI replay hashes, and a `sound_events` WAV example.
- 3D point emitters and oriented/moving listeners, clamped inverse/linear/exponential
  distance models, bounded radial Doppler and scheduled, normalized movement ramps.
- Quad, 5.1 and 7.1 mixer output, all-channel bus effects/linked limiting, explicit
  stereo downmix and optional WAV extensible speaker masks (`wav_channel_mask`).
- Spatial routing/response/order/pause/voice-limit/allocation tests, native/WASI
  hashes and a `spatial_audio` orbit/flyby example with surround and stereo WAVs.
- Optional native `file-stream` feature: WAV/FLAC/MP3 reader threads with fixed
  encoded windows and PCM rings, prefill, generation-tagged asynchronous seeks,
  cancellation/lifetime handles, independent resident budgets and diagnostics.
- Recoverable `ErrorCode::Underrun` for mixer streams: silence with held source
  position, while output-time fades/stops continue. Disk loops run seeks on the
  reader and may have gaps. Callback read/seek paths allocate/free nothing.
- Disk/memory equality, seeking, loops, concurrency, malformed/late errors,
  unknown-length output limits and allocation tests, plus `stream_from_disk`.
- Opt-in tracker instruments: four-bit wavetables, volume/duty/arpeggio/fine-pitch
  macros with rational frame/tick clocks, hold/loop endings, NTSC period noise,
  low-rate unsigned PCM with text hex data, and optional four-bit gain/period pitch.
  Legacy score checksums stay unchanged. Rendering and reset allocate/free nothing.
- Tracker response, timing, PCM lifecycle, native/WASI checksum tests and a
  `tracker_instruments` example exporting separate voices and a combined phrase.
- Synthesized drum instruments in chiptune/electronic kits: kick, snare, closed/open
  hat, tom, clap and crash, with finite independent tone/noise envelopes, pitch
  sweeps, click and seed controls. Available in scores or as standalone offline hits.
- Per-note velocity (`c16!96`, `ChannelWriter::note_with_velocity`) defaults to 127;
  unsuffixed legacy scores retain their sample bits. Drum tails survive gate-off,
  retriggers replace them on their channel, and loop carries do not invent hits.
- Drum envelope/pitch/lifecycle/determinism tests, callback allocation/free checks,
  native/WASI checksum coverage and a `drum_kits` pattern example for both kits.
- Chorus, flanger, phaser, bitcrusher/sample-rate reduction and cubic soft saturation
  in the existing bus effect chains. Whole tracks use a dedicated music bus.
- Retained-phase LFO automation in Hz or explicit beat/BPM periods, bounded prepared
  modulation delays, fractional bit-depth blending, measured response/continuity and
  native/WASI checksum tests, and a `colour_effects` comparison WAV example.
- Bus effect chains (`mixer::effects`): first-order low/high-pass filters, hard-knee
  stereo-linked compression and sidechain ducking, fractional delay with beat times,
  and a comb/allpass reverb. Settings ramp per sample without resetting DSP state.
- Sample-scheduled chain edits, application-time sidechain dependency sorting and
  cycle rejection, paused DSP clocks, bounded control-thread chain reclamation,
  quantitative DSP/real-time/determinism tests and a `bus_effects` WAV example.
- Structured `Tags` in `EncodeOptions::tags` and `DecodeInfo::tags`, preserving the
  existing native metadata map. MP3 writes ID3v2.4 UTF-8 text/COMM/APIC, FLAC writes
  Vorbis comments and front-cover PICTURE blocks, and WAV INFO adds track/year/genre.
  JPEG/PNG cover bytes round-trip exactly; unsupported WAV fields are errors.
- Metadata byte/field bounds, UTF-8/size validation, tagged decode/seek tests,
  a `tag_audio` example and an optional independent Mutagen/FFmpeg verification script.
- Optional `music` module (enables `mixer`): fixed-tempo beat/bar transport,
  atomic track transitions and crossfades, one-shot cues, and synchronized intensity
  layers. Queued operations are quantized on audio-thread receipt, and group
  capacity failures preserve the previous track. Supports decoded sounds, streaming
  decoders and synthesized scores, with an `adaptive_music` example.
- Optional `assets` module: a single background file/byte loader with a bounded
  request queue, input/decode limits and one-shot polling/waiting handles returning
  shared `Arc<Audio>`. Available on native targets, independently of the mixer.
- `AudioCache`: a retained-data byte budget and LRU eviction that skips sounds
  still referenced by callers or voices. Cache admission is best effort during
  loading; an oversized sound or a pinned cache does not fail a successful decode.
  The standalone cache also supports WebAssembly.
- A `load_assets` example and asset-loading/cache documentation.

### Changed

- FFmpeg interoperability runs on releases and manual CI dispatches, leaving
  routine push/PR checks independent of FFmpeg installation.

### Breaking

- Full `Play` literals need `emitter`; `MixerConfig` needs `listener` and `spatial`;
  `EncodeOptions` needs `wav_channel_mask`. Defaults preserve existing behavior.
- Full struct literals for `Instrument` need `chip`, `drum` and `synthwave`; `Note`
  needs `velocity`. Constructors/defaults disable these sources and use velocity 127.
- Full struct literals for `EncodeOptions` and `DecodeInfo` need the new `tags`
  field, or `..Default::default()`. Existing `metadata` map callers keep their keys.

## 0.3.0

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
- An optional look-ahead peak limiter on the master bus (`MixerConfig::limiter`,
  `MasterLimiter`), with its latency and gain reduction readable
  (`MixerHandle::latency`, `limiter_gain`). Off by default, so existing output is
  unchanged.

### Breaking

- `MixerEvent` is `#[non_exhaustive]` and gains `Limited`; a `match` on it needs a
  wildcard arm.

### Changed

- The offline transforms that render audio give the same samples on every platform,
  WebAssembly included: `pan`, `gain_db`, equal-power `fade`, `crossfade_to`,
  `filter`, `limit`, `resample`, `tempo`, `pitch`, `trim_silence`, peak `normalize`
  and `analyze::from_db` use the crate's own math. Results match the previous ones
  within about 1e-14 before rounding to `f32`. CI pins them with checksums on Linux,
  macOS, Windows and WebAssembly.
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
