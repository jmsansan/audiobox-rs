# Group buses and shared sends

With the `mixer` feature, `BusRouting`, `BusSend` and `SendPosition` opt into a
prepared audio graph over the named buses in `MixerConfig::buses`. A group and an
effect return are ordinary named buses: they sum their direct voices and routed
inputs, run their insert chain once, then feed their configured destinations.
Studio owns names and project syntax; the engine owns these DSP/routing rules.

```rust
use audiobox::mixer::{BusRouting, BusSend, Mixer, MixerConfig, SendPosition};

let (_mixer, mut handle) = Mixer::new(MixerConfig {
    buses: vec!["lead".into(), "bass".into(), "group".into(), "echo".into()],
    ..Default::default()
})?;
let lead = handle.bus("lead").unwrap();
let bass = handle.bus("bass").unwrap();
let group = handle.bus("group").unwrap();
let echo = handle.bus("echo").unwrap();
let edit = handle.set_routing(&[
    BusRouting {
        output: Some(group),
        sends: vec![BusSend {
            destination: echo,
            gain: 0.35,
            position: SendPosition::PreFader,
        }],
        ..BusRouting::new(lead)
    },
    BusRouting { output: Some(group), ..BusRouting::new(bass) },
])?;
# Ok::<(), audiobox::Error>(())
```

Here lead/bass feed group, group feeds master, and lead also feeds echo. Echo
feeds master once. Put a wet-only delay/reverb on echo to share it; a dry component
in that insert would intentionally add another dry contribution. There is no
implicit direct-to-master contribution from a bus whose main output was changed.
An impulse follows every enabled path with no extra routing latency. Master
inserts, master gain and the existing limiter process the final sum once.

`BusRouting::new(bus)` uses a unity post-fader main output to master, no sends,
centered pan, mute off and solo off. `output: None` creates a send-only or
detector-only bus. Each edit replaces the **whole graph and routing settings**;
omitted named buses get those defaults. Existing bus gain/fades, pause state,
voices and insert chains/history survive. Master is a sink, cannot be configured
as a routing source, and retains its existing gain/pause/effect controls.

## Signal placement and controls

For each bus, in dependency order:

1. Sum directly assigned voices and enabled incoming routes.
2. Run the bus's existing insert chain, including same-frame sidechain detection.
3. Apply the bus pause fade. A pre-fader send taps here.
4. Apply bus gain and pan. The main output and post-fader sends tap here.
5. Apply routing mute/solo gates to both taps and the post-fader sidechain key.
6. Accumulate each enabled route into its destination using the send's linear gain.

Send gain must be finite and nonnegative. It is independent of bus gain. Bus pan
is a linear stereo balance in `-1..=1`: center preserves both channels, +1 silences
left, -1 silences right. Nonzero pan requires a stereo mixer; all other supported
layouts can use centered routing without remapping channels. Pre-fader sends
exclude both bus gain and bus pan, but include inserts and pause fades.

Pan, mute, solo, main outputs and sends change immediately at the graph edit's
frame. They have no implicit ramp, crossfade or delay compensation. Schedule an
edit with `handle.at(frame).set_routing(...)`; use the existing ramped bus gain or
pause APIs when a fade is needed. Routing edits retain insert history and tails;
they do not reset effects. Like existing mixer gains, parallel paths can sum above
full scale. Use appropriate headroom and the existing optional limiter.

Mute silences that bus's outgoing audio and key, while its voices and effects
continue advancing. It is local to that bus: muting a group does not silence a
child's send that bypasses the group. Pause freezes directly assigned voices and
that bus's inserts/effect clock once its fade finishes. A paused group gates its
incoming audio but does not recursively freeze child voices; those children and
their bypass sends keep advancing. Master pause freezes all voices and bus DSP.

## Solo and sidechains

With no solo bus, all routes and directly assigned voices are eligible. With one
or more solo buses, the graph selects audio paths **through any solo bus**:

- Upstream voices and routes can reach the selected bus; its downstream routes
  can reach master. Multiple solo selections form the union of these paths.
- Soloing a group excludes a child's send that bypasses the group, unless another
  selected path includes that send.
- Soloing a shared return includes its upstream sends and excludes those sources'
  dry bypass paths. Direct voices on downstream transit buses are excluded unless
  those buses also have a path to a solo selection. Their clocks still advance.

Solo/mute do not clear DSP history. An included shared return can retain tails
from earlier input. Excluded direct input is replaced with silence, while effects
continue processing/decaying unless paused. These are path selection rules, not
isolated per-track effect histories.

Compressor keys hear the key bus's current post-insert, post-gain, post-pan,
post-pause buffer with mute/solo gating. A bus with `output: None` can still key a
compressor without audible output. Detector dependencies order processing but do
not automatically expand the solo selection. To keep an otherwise unrelated key
active during solo, also solo that key bus; use `output: None` and no sends when
only its detector contribution is wanted.

Audio routes and sidechain dependencies share one acyclic processing order.
Master is always last and cannot be a source/key. Every authored edge participates
in validation, including zero-gain, muted and solo-excluded routes. Named buses
ready at the same point are processed by ascending `BusId`. Each source's main
edge precedes its authored sends. Input descriptor order does not change output.

## Edits, rejection and resource bounds

`set_routing` prepares storage and validates the audio-only graph on the control
thread. Missing buses, self routes, duplicate source definitions, duplicate
destinations from one source, audio cycles, invalid gains/pans and excessive
routes return an error before enqueue. A main output and a send cannot share a
destination, nor can two sends to one destination use different taps; use another
named bus if separate paths are needed. IDs refer to this mixer's named bus slots,
just like the existing mixer APIs.

A successful submission returns `RoutingId`, not confirmation of application.
At the scheduled frame, the audio thread validates against the actual insert
chains' detector dependencies. Poll `MixerEvent::RoutingApplied(id, frame)` or
`RoutingRejected(id, ErrorCode::InvalidArgument)` for the outcome. Rejection
preserves the whole previous graph, bus routing settings and processing order.
Insert-chain replacements and sidechain parameter edits likewise validate against
the active audio graph and report `EffectsRejected` without changing prior state.
Commands scheduled for the same frame apply in submission order; separate routing
and effect commands are individually atomic, not one transaction. Remove an old
dependency before adding an incompatible replacement.

Passing an empty slice restores legacy flat routing and default pan/mute/solo
settings. With no routing edit, the original flat mixing paths and PCM remain
unchanged. An explicit graph can change summation rounding even when its audible
levels match the default topology. Graph output is deterministic across render
block sizes; the graph fixture pins the same sample hash in native and WASI tests.

The existing configuration permits at most 255 named buses plus master.
`MAX_BUS_ROUTES` is 1,024 main outputs plus sends in the **final** graph, including
omitted buses' default main outputs. Descriptor order cannot alter this budget.
An active graph uses the mixer's existing bus buffers, prepared bounded edge and
incoming-dependency lists, and one `BLOCK * channels` pre-fader scratch buffer.
Application-time combined topology checks use fixed stack storage. Rendering
uses no routing allocation, reallocation, deallocation or blocking wait. Retirement
uses the existing safe-Rust SPSC queue with nonblocking slot access.

At most `MixerConfig::queue` routing edits may be in flight or awaiting event
polling. Each accepted edit owns one credit until polled. Queue/credit exhaustion
returns `LimitExceeded` without changing the active graph or consuming an ID.
An applied edit carries the previous graph back to the control thread; a rejected
edit carries its prepared graph back. A separate bounded return queue and prepared
retirement storage retain these objects even if event polling stops or the handle
disconnects, so render never frees a graph. Destroy the mixer off the callback.
Poll events regularly to reclaim graph memory. Voice, effect and routing queues
are independent; their combined polling order need not be chronological. Existing
voice admission/retirement policy is unchanged.

## Controls and measured cost

`tests/routing.rs` covers serial impulse gains/alignment, two dry paths sharing a
wet-only delayed return, pre/post placement, pan, mute/solo path selection,
sidechain order and rejected graph/chain/parameter cycles, malformed inputs,
queue exhaustion, stopped polling/disconnection, pause clocks, supported layouts,
and the maximum 255-bus/1,024-route graph. A seeded stereo fixture with scheduled
edits/inserts is invariant at blocks 1, 7, 64, 333, 1,000 and 4,096, with sample hash
`8139826998022970413`. The test allocator checks both allocations and frees during
render. The routing suite is also part of the WASI CI job.

Release measurements on 2026-10-26 used Rust 1.98.1, Windows x86_64, AMD Ryzen 7
1700X (16 logical CPUs), stereo 48 kHz, and 256-frame callbacks. These are one
host's observations, not a real-time guarantee on other hosts. Each case has one
warm-up, five timed runs and a separate allocation-counted run. Graphs use serial
main outputs and forward shared sends; voices are looped and prepared beforehand.
These routing-only cases install no insert effects.
The flat baseline has 255 named buses and no audio graph. Timings exclude fixture
and descriptor construction. Full phases, allocation counts, bytes and metadata
are in [the raw CSV](performance/group-routing-windows.csv).

| Named buses / routes / voices | Graph prep ms | Apply ms | Render 10 s, median ms | Callback p50 us | p99 us | Max us | Graph peak bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 16 / 32 / 16 | 0.0127 | 0.0161 | 150.2324 | 70.60 | 164.70 | 705.20 | 5,416 |
| 64 / 256 / 32 | 0.0428 | 0.0336 | 482.3330 | 224.40 | 482.40 | 1,592.80 | 15,784 |
| 255 / 1,024 / 64 | 0.1381 | 0.0817 | 1,471.2193 | 724.20 | 1,347.00 | 2,186.90 | 52,968 |
| 255 / flat / 64 | — | 0.0590 | 662.6532 | 320.70 | 617.60 | 1,103.40 | — |

Apply/render allocated, reallocated and freed zero times in every counted case.
No measured callback exceeded the 5,333.3 us block duration. Graph peak bytes cover
the preparation phase's extra live memory; mixer/bus buffers and input descriptors
are separate. Apply includes the queued voice starts. Reproduce with:

```sh
cargo bench --locked --offline --all-features --bench engine -- mixer/routing --quick --runs 5 --csv docs/performance/group-routing-windows.csv
cargo run --release --locked --offline --features mixer --example group_sends -- target/group-sends.wav
```

The example renders two tracks through a group low-pass and a shared wet delay,
using both pre/post taps, stereo bus balance and a tail after the voices finish.
The engine adds no dependency, plugin host, feedback graph or Studio file format.
