# Virtual voices and resampler quality (#22)

Virtualization is opt-in. `MixerConfig::default()` still uses destructive voice
stealing at its existing limit and `Play::default()` still uses the 16-tap sinc
resampler. Existing default-path PCM fixtures are unchanged.

```rust
use audiobox::mixer::{Mixer, MixerConfig, Play, ResamplingQuality, VirtualVoices};

let (mut mixer, mut handle) = Mixer::new(MixerConfig {
    max_voices: 16, // interpolated/mixed voices
    virtual_voices: Some(VirtualVoices {
        capacity: 256, // total retained logical voices, including those 16
        threshold: 0.0001,
    }),
    ..Default::default()
})?;
let play = Play { resampling: ResamplingQuality::Linear, ..Default::default() };
# Ok::<(), audiobox::Error>(())
```

## Capacity, selection and clocks

The physical budget `max_voices` is 1..=1024. Logical capacity must be at least
that budget and at most `MAX_LOGICAL_VOICES` (4096). Paused and virtual voices
occupy logical capacity and count against per-sound instance limits. Exceeding
logical capacity retains the existing priority/oldest stealing and rejection
rules; virtualizing a retained voice emits neither `Stolen` nor `Rejected`.
Music's atomic multi-layer admission also uses logical capacity.

Selection runs on globally aligned `VIRTUAL_VOICE_QUANTUM` boundaries (32 output
frames) and at applied command frames. It is independent of callback size. Eligible
voices rank by descending priority, then estimated level, then ascending start
order. At most `max_voices` live voices are physical. Already physical stop fades
may continue within a total budget of `max_voices + 8`; excess tails are culled
and do not return. Storage also has eight bounded stop-tail slots beyond logical
capacity. Finished sources release capacity without waiting for event polling.

The level estimate is the memory source's whole-file absolute sample peak times
a conservative product of gain, fade, distance and pause-ramp maxima through the
rest of the current quantum. Streams use a source reference of 1. Eligibility
requires the estimate to be strictly greater than `threshold` (finite 0..=1,
default 0.0001). Zero PCM is therefore virtual even at threshold zero. Increasing
a quiet source's gain can make it eligible again. Future ramp maxima can promote
a voice before it crosses the floor, preserving fade-ins. Source peak does not
track local silent passages; sinc overshoot, perceived loudness, cancellation,
pan, effect gain and bus faders/mute/solo are outside this estimate. Ignoring bus
gain also preserves audible pre-fader sends. This is a selection heuristic,
not a silence detector or an audibility guarantee.

When virtualization is enabled, first preparation of an `Audio` scans its PCM
on the control thread. A metadata-only cache of at most `capacity` source IDs and
peaks avoids repeat scans while cached; round-robin replacement bounds its memory
and does not retain sample allocations. Cache misses cost O(source samples).
Disabled virtualization performs no peak scan.

`VoiceState::Virtual` means the playback clock is advancing without interpolation,
panning or accumulation. `Paused` freezes that clock; `Stopping` takes precedence
during a stop fade. `voice_status` positions use source frames and wrap through
loops. `active_voices()` counts all retained voices, including pauses and tails;
`physical_voices()` and `virtual_voice_count()` report the latest selection,
excluding paused voices. These are snapshots, not per-callback peak counters.

Constant-speed memory voices skip source frames in O(1) per render segment.
Rate/pitch/Doppler ramps advance their fixed-point clock per output frame. On
promotion, memory playback rebuilds bounded interpolation history around that
position, including whole/region loops and a nonzero start. It resumes the sample
the continuously rendered reference would play. Voice pauses, bus pauses and
stop fades retain their existing timing rules.

Stateful streams (including synthesizers and incremental decoders) must still
consume and discard PCM while virtual to preserve their state. Their DSP/I/O
cost is not bounded by the physical budget. There is no arbitrary seek shortcut.
An `Underrun` keeps the existing policy: silence and a held source position,
with another read on a subsequent segment. Background I/O timing is not a
cross-host deterministic input. Stream implementations must obey the mixer's
existing real-time source contract.

Selection gates output immediately. Use source/movement fades and appropriate
priorities to manage transitions; switching a physical slot can click. Shared
bus effects keep running, but receive only physical input, so effect histories
can differ from rendering all sources. CPU cost includes scanning/sorting the
logical pool every quantum and processing bus graphs/effects, not just the
selected voices. A larger pool can exceed a callback deadline even when few
voices are physical.

## Distance culling

2D `Space2d` already reaches zero at its maximum distance. With virtualization,
3D emitters at or beyond `Attenuation::max_distance` also get a zero target
distance gain for every distance model, including inverse and exponential.
Movement/listener commands recompute placement; the engine does not integrate
emitter velocity into position. An explicit movement fade may keep the voice
physical while its distance gain fades out. Moving inside the range can promote
it on the command frame, subject to the physical budget and threshold.

Without virtualization, existing 3D attenuation and the public spatial placement
helpers retain their old behavior, including the nonzero inverse/exponential
gain floor. Cull decisions use the mixer's listener and emitter coordinates.

## Per-voice resampling

`Play::resampling` is fixed when the voice is created; it works with or without
virtualization. `Sinc` retains the existing 16-tap windowed-sinc interpolation
and downsampling filter. `Linear` interpolates two adjacent source samples in
f64 with the same fixed-point clock. Unit-speed integral playback still copies
the exact source sample. Linear ends at the nominal source boundary, with zero
extension of its final adjacent sample; sinc retains its existing filter tail.
Both use the bounded source prefetch ring; selecting Linear does not turn stream
reads into single-sample reads.

Linear is cheaper and skips preparation of source-rate-specific sinc tables,
but has no anti-alias filter. The rational 36 kHz / 96 kHz test downsampled to
48 kHz produces a strong 12 kHz alias with Linear; sinc suppresses its mean-square
energy by more than 30 dB relative to Linear. Choose it explicitly for sources
whose bandwidth/quality requirements allow that tradeoff. Virtual memory
promotion matches a continuously playing reference separately for each quality.

## Resource and compatibility contract

Selection storage, logical slots and the bounded source metadata cache are
allocated before playback. The callback does not allocate or free for the tested
pool, promotion, command, stop or retirement paths. Poll `handle.poll_event()` on
the control thread regularly to retire sources and edits, as with the existing
mixer. Exhausting both existing return buffers without polling can still force
destruction in `render`. If the handle disconnects, pending finished voices now
remain in the bounded retirement storage until final mixer destruction; drop
the mixer off the audio thread.

Full `MixerConfig` literals need `virtual_voices`, full `Play` literals need
`resampling`, or use `..Default::default()`. Exhaustive `VoiceState` matches need
the new `Virtual` arm. These are 0.5 API changes. No multi-threaded rendering or
automatic quality downgrade is introduced.

## Verification and measurements

`tests/virtual_voices.rs` checks exact promotion samples; warm resampler history
through loops, rate/pitch changes and pauses; strict threshold/peak behavior;
priority and tie ordering; distance culling; stream state; pre-fader sends;
logical limits, 4096-voice retirement and allocator activity. The complex memory
fixture pins separate native/WASI hashes for both qualities over block sizes
1/7/32/64/333/4096. Existing mixer/music/spatial/routing fixtures cover the
default path. The example writes a three-second scene:

```text
cargo run --example virtual_voices --features mixer -- target/virtual-voices.wav
```

The existing #78 benchmark runner supplies feature-specific cases, without
automatic benchmark CI:

```text
cargo bench --bench engine --all-features -- --quick --runs 5 --csv results.csv mixer/scale/ mixer/effect/ mixer/effects-16/ mixer/voices-16/ mixer/empty/
```

The scale cases share resident looping mono PCM at 44.1 kHz, stereo 48 kHz
output and 256-frame callbacks. They compare 16/256 physical voices, sinc/linear,
256/1024/4096 logical pools with 16/64 physical budgets, and 240 distant emitters
plus 16 near emitters. The reused empty/16-voice/effect-chain cases expose baseline
and effect costs, with separate low-pass/high-pass/compressor/chorus/phaser/delay/
reverb cases. Control commands/source preparation remain outside render
timing; the first callback includes installing all queued starts. See the
[shared protocol](performance.md) for checksum, phase and allocation semantics.

Raw observations: [Windows scale CSV](https://github.com/jmsansan/audiobox-rs/blob/v0.5.1/docs/performance/virtual-voices-windows.csv) and
[individual effects CSV](https://github.com/jmsansan/audiobox-rs/blob/v0.5.1/docs/performance/virtual-voices-effects-windows.csv). Timing
and callback deadlines describe that host/run, not real-time guarantees.

Windows x86_64, Ryzen 7 1700X, Rust 1.98.1 release/all features, one warm-up,
five timed runs and one separately counted run, no profiler or concurrent tests:

| Case (logical / physical) | Median render ms / 10 s | Callback p50 / p99 / max µs | Over 5333 µs (9375 callbacks) |
| --- | ---: | ---: | ---: |
| 16 / 16, sinc, 44.1 → 48 kHz | 371.6300 | 189.1 / 355.6 / 632.7 | 0 |
| 256 / 256, sinc | 6065.0667 | 3064.1 / 5003.3 / 19152.9 | 56 |
| 256 / 256, linear | 1938.3058 | 977.2 / 1676.0 / 2415.0 | 0 |
| 256 / 16, sinc | 551.8120 | 283.0 / 529.1 / 2786.1 | 0 |
| 256 / 16, linear | 314.5739 | 160.5 / 300.4 / 754.6 | 0 |
| 256 / 16, 240 distant, sinc | 567.4571 | 271.0 / 606.5 / 1506.1 | 0 |
| 1024 / 64, sinc | 2444.9595 | 1209.0 / 2312.1 / 12337.1 | 5 |
| 4096 / 64, sinc | 7992.3498 | 3468.6 / 9320.3 / 203924.7 | 721 |
| Empty mixer | 3.5747 | 1.9 / 2.3 / 24.2 | 0 |
| 16 / 16, same-rate direct | 104.4732 | 55.7 / 64.6 / 84.2 | 0 |
| 16 / 16, six-effect chain + limiter | 517.2114 | 275.7 / 318.7 / 2012.9 | 0 |

All listed render phases counted zero allocations, reallocations and frees.
256 logical / 16 physical sinc saved about 90.9% of all-256 sinc render time;
all-256 Linear saved about 68.0%, with intentionally different output quality.
Those comparisons change the selected sources/quality, not a bit-preserving
optimization. Checksums remain stable over repeated runs of each individual case.

4096 is a storage ceiling, not a recommended callback configuration on this host:
that case exceeded the deadline repeatedly. The first callback also installs all
4096 queued starts, including the existing repeated admission scans; this explains
the large startup maximum. The 1024 case has startup/deadline outliers too. Size
the logical pool for the target and measure it; physical limits alone do not cap
total callback time. Source preparation, OS scheduling and background load can
change these observations.
