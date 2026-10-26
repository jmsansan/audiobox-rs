#![cfg(feature = "mixer")]
use audiobox::mixer::effects::{Compressor, Delay, Effect, EffectTime};
use audiobox::mixer::*;
use audiobox::{Audio, ErrorCode};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

struct Count;
thread_local! {
    static ENABLED:Cell<bool>=const {Cell::new(false)};
    static ALLOCS:Cell<usize>=const {Cell::new(0)};
    static FREES:Cell<usize>=const {Cell::new(0)};
}
fn enabled() -> bool {
    ENABLED.try_with(Cell::get).unwrap_or(false)
}
unsafe impl GlobalAlloc for Count {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if enabled() {
            ALLOCS.with(|a| a.set(a.get() + 1));
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if enabled() {
            ALLOCS.with(|a| a.set(a.get() + 1));
        }
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if enabled() {
            FREES.with(|a| a.set(a.get() + 1));
        }
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if enabled() {
            ALLOCS.with(|a| a.set(a.get() + 1));
        }
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static GLOBAL: Count = Count;
fn realtime<T>(f: impl FnOnce() -> T) -> T {
    ALLOCS.with(|a| a.set(0));
    FREES.with(|a| a.set(0));
    ENABLED.with(|a| a.set(true));
    let value = f();
    ENABLED.with(|a| a.set(false));
    assert_eq!(ALLOCS.with(Cell::get), 0, "render allocated");
    assert_eq!(FREES.with(Cell::get), 0, "render freed memory");
    value
}
fn setup(ch: usize, queue: usize) -> (Mixer, MixerHandle) {
    Mixer::new(MixerConfig {
        sample_rate: 1000,
        channels: ch,
        max_voices: 16,
        queue,
        buses: vec!["a".into(), "b".into(), "group".into(), "return".into()],
        ..Default::default()
    })
    .unwrap()
}
fn id(h: &MixerHandle, name: &str) -> BusId {
    h.bus(name).unwrap()
}
fn play(h: &mut MixerHandle, bus: BusId, values: &[f32], frames: usize, looped: bool) {
    let audio = Arc::new(
        Audio::new(
            values.iter().map(|&x| vec![x; frames]).collect(),
            h.sample_rate(),
        )
        .unwrap(),
    );
    h.play(
        audio,
        &Play {
            bus,
            looping: if looped { Loop::Whole } else { Loop::default() },
            ..Default::default()
        },
    )
    .unwrap();
}
fn render(m: &mut Mixer, frames: usize, block: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames * m.channels()];
    for chunk in out.chunks_mut(block * m.channels()) {
        realtime(|| m.render(chunk));
    }
    out
}
fn events(h: &mut MixerHandle) -> Vec<MixerEvent> {
    std::iter::from_fn(|| h.poll_event()).collect()
}
fn send(destination: BusId, gain: f32, position: SendPosition) -> BusSend {
    BusSend {
        destination,
        gain,
        position,
    }
}
fn delay() -> Effect {
    Effect::Delay(Delay {
        time: EffectTime::Seconds(0.004),
        max_seconds: 0.01,
        feedback: 0.0,
        mix: 1.0,
    })
}

#[test]
fn serial_groups_apply_each_fader_once_without_a_direct_master_duplicate() {
    let (mut m, mut h) = setup(1, 16);
    let a = id(&h, "a");
    let b = id(&h, "b");
    let g = id(&h, "group");
    h.set_routing(&[
        BusRouting {
            output: Some(b),
            ..BusRouting::new(a)
        },
        BusRouting {
            output: Some(g),
            ..BusRouting::new(b)
        },
    ])
    .unwrap();
    h.set_bus_gain(a, 0.5, Fade::NONE).unwrap();
    h.set_bus_gain(b, 0.25, Fade::NONE).unwrap();
    h.set_bus_gain(g, 0.5, Fade::NONE).unwrap();
    h.set_bus_gain(BusId::MASTER, 0.5, Fade::NONE).unwrap();
    play(&mut h, a, &[1.0], 1, false);
    let out = render(&mut m, 17, 7);
    assert_eq!(out[0], 0.03125);
    assert!(out[1..].iter().all(|&s| s == 0.0));
    assert!(
        events(&mut h)
            .iter()
            .any(|e| matches!(e, MixerEvent::RoutingApplied(_, 0)))
    );
}

#[test]
fn parallel_dry_and_shared_wet_returns_have_one_insert_pass_and_exact_alignment() {
    let (mut m, mut h) = setup(1, 16);
    let a = id(&h, "a");
    let b = id(&h, "b");
    let r = id(&h, "return");
    h.set_routing(&[
        BusRouting {
            sends: vec![send(r, 0.25, SendPosition::PostFader)],
            ..BusRouting::new(a)
        },
        BusRouting {
            sends: vec![send(r, 0.5, SendPosition::PostFader)],
            ..BusRouting::new(b)
        },
    ])
    .unwrap();
    h.set_effects(r, &[delay()]).unwrap();
    play(&mut h, a, &[1.0], 1, false);
    play(&mut h, b, &[2.0], 1, false);
    let out = render(&mut m, 16, 3);
    assert_eq!(out[0], 3.0);
    assert_eq!(out[4], 1.25);
    assert!(
        out.iter()
            .enumerate()
            .all(|(i, &s)| i == 0 || i == 4 || s == 0.0)
    );
}

#[test]
fn pre_post_faders_pan_and_mute_have_declared_taps() {
    let (mut m, mut h) = setup(2, 16);
    let a = id(&h, "a");
    let b = id(&h, "b");
    let r = id(&h, "return");
    let route = BusRouting {
        output: None,
        sends: vec![
            send(b, 1.0, SendPosition::PreFader),
            send(r, 1.0, SendPosition::PostFader),
        ],
        pan: 1.0,
        ..BusRouting::new(a)
    };
    h.set_routing(std::slice::from_ref(&route)).unwrap();
    h.set_bus_gain(a, 0.25, Fade::NONE).unwrap();
    play(&mut h, a, &[1.0, 1.0], 64, false);
    h.at(8)
        .set_routing(&[BusRouting {
            muted: true,
            ..route.clone()
        }])
        .unwrap();
    h.at(12).set_routing(&[route]).unwrap();
    let out = render(&mut m, 16, 7);
    for (f, frame) in out.chunks_exact(2).enumerate() {
        assert_eq!(
            frame,
            if (8..12).contains(&f) {
                &[0.0, 0.0][..]
            } else {
                &[1.0, 1.25][..]
            },
            "frame {f}"
        );
    }
}

#[test]
fn solo_selects_paths_through_groups_or_returns_without_dry_leakage() {
    for solo_return in [false, true] {
        let (mut m, mut h) = setup(1, 16);
        let a = id(&h, "a");
        let g = id(&h, "group");
        let r = id(&h, "return");
        h.set_routing(&[
            BusRouting {
                output: Some(g),
                sends: vec![send(r, 0.5, SendPosition::PreFader)],
                ..BusRouting::new(a)
            },
            BusRouting {
                solo: !solo_return,
                ..BusRouting::new(g)
            },
            BusRouting {
                solo: solo_return,
                ..BusRouting::new(r)
            },
        ])
        .unwrap();
        play(&mut h, a, &[1.0], 16, false);
        play(&mut h, BusId::MASTER, &[100.0], 16, false);
        assert_eq!(
            render(&mut m, 16, 3),
            vec![if solo_return { 0.5 } else { 1.0 }; 16]
        );
    }
    // Group mute is route-local: a child's explicit bypass send still reaches a return.
    let (mut m, mut h) = setup(1, 16);
    let a = id(&h, "a");
    let g = id(&h, "group");
    let r = id(&h, "return");
    h.set_routing(&[
        BusRouting {
            output: Some(g),
            sends: vec![send(r, 0.5, SendPosition::PreFader)],
            ..BusRouting::new(a)
        },
        BusRouting {
            muted: true,
            ..BusRouting::new(g)
        },
    ])
    .unwrap();
    play(&mut h, a, &[1.0], 16, false);
    assert_eq!(render(&mut m, 16, 7), vec![0.5; 16]);
}

#[test]
fn audio_and_detector_dependencies_use_the_same_frame_and_cycle_validation() {
    let (mut m, mut h) = setup(1, 16);
    let a = id(&h, "a");
    let key = id(&h, "b");
    let kick = id(&h, "group");
    let routes = [
        BusRouting {
            output: Some(key),
            ..BusRouting::new(kick)
        },
        BusRouting {
            output: None,
            ..BusRouting::new(key)
        },
    ];
    h.set_routing(&routes).unwrap();
    h.set_bus_gain(key, 0.5, Fade::NONE).unwrap();
    let compressor = Compressor {
        threshold_db: -12.0,
        ratio: 4.0,
        attack_ms: 0.0,
        release_ms: 0.0,
        sidechain: Some(key),
        ..Default::default()
    };
    h.set_effects(a, &[Effect::Compressor(compressor)]).unwrap();
    play(&mut h, a, &[0.25], 16, false);
    play(&mut h, kick, &[1.0], 16, false);
    h.at(4)
        .set_routing(&[
            routes[0].clone(),
            BusRouting {
                muted: true,
                ..routes[1].clone()
            },
        ])
        .unwrap();
    let out = render(&mut m, 8, 3);
    let gain = 10f64.powf(-(20.0 * 0.5f64.log10() + 12.0) * 0.75 / 20.0);
    assert!(
        out[..4]
            .iter()
            .all(|&x| (x as f64 - 0.25 * gain).abs() < 1e-7)
    );
    assert_eq!(&out[4..], &[0.25; 4]);
    // Changing routes can conflict with an already installed detector edge key->a.
    let rejected = h
        .set_routing(&[
            BusRouting {
                output: Some(key),
                ..BusRouting::new(a)
            },
            BusRouting {
                output: None,
                ..BusRouting::new(key)
            },
        ])
        .unwrap();
    assert_eq!(render(&mut m, 1, 1), [0.25]);
    assert!(events(&mut h).contains(&MixerEvent::RoutingRejected(
        rejected,
        ErrorCode::InvalidArgument
    )));
    // Conversely an effect chain/parameter cannot introduce a cycle through audio sends.
    h.set_effects(a, &[]).unwrap();
    h.set_routing(&[BusRouting {
        output: Some(key),
        ..BusRouting::new(a)
    }])
    .unwrap();
    h.set_effects(a, &[Effect::Compressor(compressor)]).unwrap();
    render(&mut m, 1, 1);
    assert!(events(&mut h).contains(&MixerEvent::EffectsRejected(a, ErrorCode::InvalidArgument)));
    h.set_effects(
        a,
        &[Effect::Compressor(Compressor {
            sidechain: None,
            ..compressor
        })],
    )
    .unwrap();
    render(&mut m, 1, 1);
    events(&mut h);
    h.set_effect(a, 0, Effect::Compressor(compressor), Fade::NONE)
        .unwrap();
    render(&mut m, 1, 1);
    assert!(events(&mut h).contains(&MixerEvent::EffectsRejected(a, ErrorCode::InvalidArgument)));
}

#[test]
fn malformed_graphs_fail_before_enqueue_and_preserve_routing() {
    let (mut m, mut h) = setup(1, 16);
    let a = id(&h, "a");
    let b = id(&h, "b");
    let (_, other) = Mixer::new(MixerConfig {
        buses: (0..8).map(|n| format!("other{n}")).collect(),
        ..Default::default()
    })
    .unwrap();
    let missing = other.bus("other7").unwrap();
    for routes in [
        vec![
            BusRouting {
                output: Some(b),
                ..BusRouting::new(a)
            },
            BusRouting {
                output: Some(a),
                ..BusRouting::new(b)
            },
        ],
        vec![BusRouting {
            output: Some(a),
            ..BusRouting::new(a)
        }],
        vec![BusRouting::new(missing)],
        vec![BusRouting {
            output: Some(missing),
            ..BusRouting::new(a)
        }],
        vec![BusRouting::new(BusId::MASTER)],
        vec![BusRouting::new(a), BusRouting::new(a)],
        vec![BusRouting {
            pan: 0.5,
            ..BusRouting::new(a)
        }],
        vec![BusRouting {
            sends: vec![send(BusId::MASTER, 1.0, SendPosition::PreFader)],
            ..BusRouting::new(a)
        }],
    ] {
        assert_eq!(
            h.set_routing(&routes).unwrap_err().code,
            ErrorCode::InvalidArgument
        );
    }
    for gain in [-1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            h.set_routing(&[BusRouting {
                sends: vec![send(b, gain, SendPosition::PostFader)],
                ..BusRouting::new(a)
            }])
            .unwrap_err()
            .code,
            ErrorCode::InvalidArgument
        );
    }
    assert_eq!(
        h.set_routing(&[BusRouting {
            sends: vec![send(b, 1.0, SendPosition::PostFader); MAX_BUS_ROUTES + 1],
            ..BusRouting::new(a)
        }])
        .unwrap_err()
        .code,
        ErrorCode::LimitExceeded
    );
    play(&mut h, a, &[1.0], 8, false);
    assert_eq!(render(&mut m, 8, 3), vec![1.0; 8]);
}

#[test]
fn edit_queue_credit_retirement_and_dropped_handle_never_free_on_render() {
    let (mut m, mut h) = setup(1, 1);
    let a = id(&h, "a");
    play(&mut h, a, &[1.0], 32, true);
    assert_eq!(
        h.set_routing(&[BusRouting::new(a)]).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    render(&mut m, 1, 1);
    let first = h
        .set_routing(&[BusRouting {
            output: None,
            ..BusRouting::new(a)
        }])
        .unwrap();
    assert_eq!(first, RoutingId(0));
    assert_eq!(render(&mut m, 1, 1), [0.0]);
    for _ in 0..8 {
        assert_eq!(
            h.set_routing(&[]).unwrap_err().code,
            ErrorCode::LimitExceeded
        );
        assert_eq!(render(&mut m, 1, 1), [0.0]);
    }
    assert!(events(&mut h).contains(&MixerEvent::RoutingApplied(first, 1)));
    h.set_routing(&[]).unwrap();
    assert_eq!(render(&mut m, 1, 1), [1.0]);
    events(&mut h);
    let (mut m, mut h) = setup(1, 2);
    let a = id(&h, "a");
    h.set_routing(&[BusRouting::new(a)]).unwrap();
    h.at(1)
        .set_routing(&[BusRouting {
            output: None,
            ..BusRouting::new(a)
        }])
        .unwrap();
    drop(h);
    for _ in 0..16 {
        realtime(|| m.render(&mut [0.0; 3]));
    }
}

fn fingerprint(samples: &[f32]) -> u64 {
    samples
        .iter()
        .flat_map(|x| x.to_bits().to_le_bytes())
        .fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        })
}
fn golden(block: usize) -> Vec<f32> {
    let (mut m, mut h) = Mixer::new(MixerConfig {
        sample_rate: 8000,
        channels: 2,
        buses: vec!["a".into(), "b".into(), "group".into(), "return".into()],
        ..Default::default()
    })
    .unwrap();
    let a = id(&h, "a");
    let b = id(&h, "b");
    let g = id(&h, "group");
    let r = id(&h, "return");
    let mut routes = vec![
        BusRouting {
            output: Some(g),
            sends: vec![send(r, 0.25, SendPosition::PreFader)],
            pan: -0.25,
            ..BusRouting::new(a)
        },
        BusRouting {
            output: Some(g),
            sends: vec![send(r, 0.5, SendPosition::PostFader)],
            ..BusRouting::new(b)
        },
    ];
    h.set_routing(&routes).unwrap();
    h.set_effects(r, &[delay()]).unwrap();
    h.set_effects(g, &[Effect::LowPass { cutoff_hz: 2000.0 }])
        .unwrap();
    let mut seed = 7u32;
    let data: Vec<_> = (0..8000)
        .map(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 0.25 - 0.125
        })
        .collect();
    let audio = Arc::new(
        Audio::new(
            vec![data.clone(), data.into_iter().map(|x| -x * 0.5).collect()],
            8000,
        )
        .unwrap(),
    );
    h.play(
        audio.clone(),
        &Play {
            bus: a,
            ..Default::default()
        },
    )
    .unwrap();
    h.play(
        audio,
        &Play {
            bus: b,
            gain: 0.5,
            ..Default::default()
        },
    )
    .unwrap();
    h.at(501).set_bus_gain(g, 0.5, Fade::linear(0.02)).unwrap();
    routes.push(BusRouting {
        solo: true,
        ..BusRouting::new(g)
    });
    h.at(2003).set_routing(&routes).unwrap();
    routes.last_mut().unwrap().solo = false;
    routes[0].muted = true;
    h.at(3007).set_routing(&routes).unwrap();
    h.at(4009).set_routing(&[]).unwrap();
    render(&mut m, 8500, block)
}
#[test]
fn graph_edits_and_effect_state_are_block_invariant_and_cross_host_pinned() {
    let expected = golden(4096);
    for block in [1, 7, 64, 333, 1000] {
        assert_eq!(golden(block), expected);
    }
    assert_eq!(fingerprint(&expected), 8139826998022970413, "routed PCM");
}

#[test]
fn supported_layouts_keep_alignment_and_stereo_pan_is_explicit() {
    for ch in [1, 2, 4, 6, 8] {
        let (mut m, mut h) = setup(ch, 16);
        let a = id(&h, "a");
        let g = id(&h, "group");
        h.set_routing(&[BusRouting {
            output: Some(g),
            ..BusRouting::new(a)
        }])
        .unwrap();
        play(&mut h, a, &[1.0], 1, false);
        let out = render(&mut m, 16, 7);
        assert!(out[..ch].iter().any(|&s| s != 0.0));
        assert!(out[ch..].iter().all(|&s| s == 0.0));
    }
}

#[test]
fn pause_fades_gate_pre_sends_and_group_pause_keeps_child_clocks_running() {
    let (mut m, mut h) = setup(1, 16);
    let a = id(&h, "a");
    let b = id(&h, "b");
    h.set_routing(&[BusRouting {
        output: None,
        sends: vec![send(b, 1.0, SendPosition::PreFader)],
        ..BusRouting::new(a)
    }])
    .unwrap();
    h.set_bus_gain(a, 0.0, Fade::NONE).unwrap();
    let source = Audio::new(vec![(0..32).map(|i| i as f32 / 16.0).collect()], 1000).unwrap();
    h.play(
        source,
        &Play {
            bus: a,
            ..Default::default()
        },
    )
    .unwrap();
    h.at(4).pause_bus(a, Fade::linear(0.004)).unwrap();
    h.at(12).resume_bus(a, Fade::NONE).unwrap();
    let out = render(&mut m, 16, 7);
    assert_eq!(&out[4..8], &[0.25, 0.234375, 0.1875, 0.109375]);
    assert_eq!(&out[8..12], &[0.0; 4]);
    assert_eq!(out[12], 0.5);
    let (mut m, mut h) = setup(1, 16);
    let a = id(&h, "a");
    let b = id(&h, "b");
    h.set_routing(&[BusRouting {
        output: Some(b),
        ..BusRouting::new(a)
    }])
    .unwrap();
    h.play(
        Audio::new(vec![(0..32).map(|i| i as f32 / 16.0).collect()], 1000).unwrap(),
        &Play {
            bus: a,
            ..Default::default()
        },
    )
    .unwrap();
    h.at(4).pause_bus(b, Fade::NONE).unwrap();
    h.at(8).resume_bus(b, Fade::NONE).unwrap();
    let out = render(&mut m, 16, 7);
    assert_eq!(&out[4..8], &[0.0; 4]);
    assert_eq!(out[8], 0.5);
}

#[test]
fn maximum_bus_and_edge_counts_are_bounded_and_setting_order_independent() {
    let run = |reverse: bool| {
        let names: Vec<_> = (0..255).map(|i| format!("bus{i}")).collect();
        let (mut m, mut h) = Mixer::new(MixerConfig {
            sample_rate: 1000,
            channels: 1,
            max_voices: 1,
            buses: names.clone(),
            queue: 8,
            ..Default::default()
        })
        .unwrap();
        let ids: Vec<_> = names.iter().map(|n| h.bus(n).unwrap()).collect();
        let mut routes: Vec<_> = ids.iter().map(|&b| BusRouting::new(b)).collect();
        let mut edges = routes.len();
        for (from, route) in routes.iter_mut().enumerate() {
            for &to in ids.iter().skip(from + 1) {
                if edges == MAX_BUS_ROUTES {
                    break;
                }
                route.sends.push(send(to, 0.001, SendPosition::PostFader));
                edges += 1;
            }
        }
        assert_eq!(edges, MAX_BUS_ROUTES);
        if reverse {
            routes.reverse();
        }
        h.set_routing(&routes).unwrap();
        play(&mut h, ids[0], &[0.125], 32, false);
        let out = render(&mut m, 16, 7);
        assert!(out.iter().all(|s| s.is_finite() && *s >= 0.125));
        out
    };
    assert_eq!(run(false), run(true));
}
