#![cfg(feature = "mixer")]
use audiobox::mixer::*;
use audiobox::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Count;
thread_local! {static ON:Cell<bool>=const{Cell::new(false)};static ALLOC:Cell<usize>=const{Cell::new(0)};static FREE:Cell<usize>=const{Cell::new(0)};}
fn bump(cell: &'static std::thread::LocalKey<Cell<usize>>) {
    if ON.try_with(Cell::get).unwrap_or(false) {
        cell.with(|v| v.set(v.get() + 1));
    }
}
unsafe impl GlobalAlloc for Count {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        bump(&ALLOC);
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        bump(&ALLOC);
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        bump(&FREE);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        bump(&ALLOC);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static GLOBAL: Count = Count;
fn realtime<T>(f: impl FnOnce() -> T) -> T {
    ALLOC.with(|x| x.set(0));
    FREE.with(|x| x.set(0));
    ON.with(|x| x.set(true));
    let value = f();
    ON.with(|x| x.set(false));
    assert_eq!(ALLOC.with(Cell::get), 0, "render allocated");
    assert_eq!(FREE.with(Cell::get), 0, "render freed");
    value
}
fn setup(physical: usize, capacity: usize) -> (Mixer, MixerHandle) {
    Mixer::new(MixerConfig {
        channels: 1,
        max_voices: physical,
        virtual_voices: Some(VirtualVoices {
            capacity,
            threshold: 0.0,
        }),
        ..Default::default()
    })
    .unwrap()
}
fn render(m: &mut Mixer, frames: usize, block: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames * m.channels()];
    for b in out.chunks_mut(block * m.channels()) {
        realtime(|| m.render(b));
    }
    out
}
fn pcm(values: Vec<f32>, rate: u32) -> Arc<Audio> {
    Arc::new(Audio::new(vec![values], rate).unwrap())
}
fn constant(v: f32) -> Arc<Audio> {
    pcm(vec![v; 48000], 48000)
}
fn events(h: &mut MixerHandle) -> Vec<MixerEvent> {
    std::iter::from_fn(|| h.poll_event()).collect()
}

#[test]
fn distant_voice_uses_no_physical_slot_and_returns_at_the_correct_sample() {
    let (mut m, mut h) = setup(1, 4);
    let near = h
        .play(
            constant(0.25),
            &Play {
                priority: 100,
                ..Default::default()
            },
        )
        .unwrap();
    let far = h
        .play(
            pcm((0..256).map(|i| i as f32 / 512.0).collect(), 48000),
            &Play {
                priority: 200,
                position: Some([100.0, 0.0]),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(render(&mut m, 20, 7), vec![0.25; 20]);
    assert_eq!(h.voice_status(far).unwrap().state, VoiceState::Virtual);
    assert_eq!(h.voice_status(far).unwrap().position, 20);
    assert_eq!(
        (
            m.active_voices(),
            m.physical_voices(),
            m.virtual_voice_count()
        ),
        (2, 1, 1)
    );
    h.set_position(far, [0.0, 0.0], Fade::NONE).unwrap();
    assert_eq!(
        render(&mut m, 20, 3),
        (20..40).map(|i| i as f32 / 512.0).collect::<Vec<_>>()
    );
    assert_eq!(h.voice_status(near).unwrap().state, VoiceState::Virtual);
    assert_eq!(h.voice_status(far).unwrap().position, 40);
    assert!(
        !events(&mut h)
            .iter()
            .any(|e| matches!(e, MixerEvent::Stolen(_) | MixerEvent::Rejected(_)))
    );
}

#[test]
fn oversubscribed_priority_gain_and_stop_promote_retained_sources() {
    let (mut m, mut h) = setup(1, 3);
    let mut ids = Vec::new();
    for priority in [10, 20, 30] {
        ids.push(
            h.play(
                pcm(
                    (0..256)
                        .map(|i| i as f32 / 1024.0 + priority as f32 / 128.0)
                        .collect(),
                    48000,
                ),
                &Play {
                    priority,
                    ..Default::default()
                },
            )
            .unwrap(),
        );
    }
    h.at(13).set_gain(ids[2], 0.0, Fade::NONE).unwrap();
    h.at(21).stop(ids[1], Fade::NONE).unwrap();
    let out = render(&mut m, 32, 7);
    for (i, &sample) in out.iter().enumerate() {
        let priority = if i < 13 {
            30
        } else if i < 21 {
            20
        } else {
            10
        };
        assert_eq!(sample, i as f32 / 1024.0 + priority as f32 / 128.0);
    }
    assert_eq!(h.voice_status(ids[0]).unwrap().state, VoiceState::Playing);
    assert_eq!(h.voice_status(ids[2]).unwrap().state, VoiceState::Virtual);
    assert!(events(&mut h).contains(&MixerEvent::Ended(ids[1])));
}

fn complex(virtualized: bool, quality: ResamplingQuality, block: usize) -> Vec<f32> {
    let (mut m, mut h) = if virtualized {
        setup(1, 4)
    } else {
        Mixer::new(MixerConfig {
            channels: 1,
            max_voices: 4,
            ..Default::default()
        })
        .unwrap()
    };
    let blocker = if virtualized {
        Some(
            h.play(
                constant(0.5),
                &Play {
                    priority: 255,
                    ..Default::default()
                },
            )
            .unwrap(),
        )
    } else {
        None
    };
    let mut seed = 7u32;
    let source = pcm(
        (0..1024)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                ((seed >> 16) as i32 - 32768) as f32 / 131072.0
            })
            .collect(),
        32000,
    );
    let id = h
        .play(
            source,
            &Play {
                resampling: quality,
                start: TimePosition::Frames(79),
                looping: Loop::Region {
                    start: 31,
                    end: 279,
                },
                ..Default::default()
            },
        )
        .unwrap();
    h.at(503).set_pitch(id, 12.0, Fade::linear(0.005)).unwrap();
    h.at(1107).set_rate(id, 0.5, Fade::linear(0.002)).unwrap();
    h.at(1303).pause(id, Fade::linear(0.0002)).unwrap();
    h.at(1451).resume(id, Fade::NONE).unwrap();
    if let Some(blocker) = blocker {
        h.at(2003).set_gain(blocker, 0.0, Fade::NONE).unwrap();
    }
    render(&mut m, 4096, block)
}
#[test]
fn memory_promotion_preserves_filter_history_loops_rate_pitch_and_pause() {
    for q in [ResamplingQuality::Sinc, ResamplingQuality::Linear] {
        let expected = complex(false, q, 4096);
        let actual = complex(true, q, 4096);
        let hash = actual
            .iter()
            .flat_map(|x| x.to_bits().to_le_bytes())
            .fold(0xcbf29ce484222325u64, |h, b| {
                (h ^ b as u64).wrapping_mul(0x100000001b3)
            });
        assert_eq!(
            hash,
            match q {
                ResamplingQuality::Sinc => 0x6cf537aff64604ba,
                ResamplingQuality::Linear => 0xc4f00d31d61df416,
            },
            "{q:?} native/WASI fixture"
        );
        assert!(actual[..2003].iter().all(|&v| v == 0.5));
        assert_eq!(&actual[2003..], &expected[2003..], "{q:?} warm history");
        for block in [1, 7, 32, 64, 333] {
            assert_eq!(complex(true, q, block), actual, "{q:?} block {block}");
        }
    }
}

#[test]
fn logical_capacity_and_instance_limits_stay_bounded_and_explicit() {
    let (mut m, mut h) = setup(1, 2);
    let first = h
        .play(
            constant(0.25),
            &Play {
                gain: 0.0,
                instances: Some(Instances::new(7).max(1)),
                ..Default::default()
            },
        )
        .unwrap();
    let limited = h
        .play(
            constant(0.5),
            &Play {
                instances: Some(Instances::new(7).max(1)),
                ..Default::default()
            },
        )
        .unwrap();
    h.play(
        constant(0.0),
        &Play {
            gain: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    let rejected = h
        .play(
            constant(0.5),
            &Play {
                priority: 0,
                ..Default::default()
            },
        )
        .unwrap();
    render(&mut m, 100, 7);
    assert_eq!(m.active_voices(), 2);
    assert_eq!(m.physical_voices(), 0);
    assert_eq!(h.voice_status(first).unwrap().position, 100);
    let e = events(&mut h);
    assert!(e.contains(&MixerEvent::Limited(limited)));
    assert!(e.contains(&MixerEvent::Rejected(rejected)));
}

#[test]
fn pauses_freeze_virtual_clocks_and_stop_finishes_without_a_source_restart() {
    let (mut m, mut h) = setup(1, 3);
    h.play(
        constant(0.5),
        &Play {
            priority: 255,
            ..Default::default()
        },
    )
    .unwrap();
    let v = h.play(constant(0.25), &Play::default()).unwrap();
    h.at(10).pause(v, Fade::NONE).unwrap();
    h.at(20).resume(v, Fade::NONE).unwrap();
    render(&mut m, 30, 7);
    assert_eq!(h.voice_status(v).unwrap().position, 20);
    h.stop(v, Fade::linear(4.0 / 48000.0)).unwrap();
    render(&mut m, 4, 3);
    assert!(events(&mut h).contains(&MixerEvent::Ended(v)));
}

#[test]
fn inverse_distance_culls_outside_max_and_listener_motion_reactivates() {
    let (mut m, mut h) = setup(1, 4);
    let near = h
        .play(
            constant(0.25),
            &Play {
                priority: 100,
                ..Default::default()
            },
        )
        .unwrap();
    let emitter = Emitter {
        position: [0.0, 0.0, -20.0],
        attenuation: Attenuation {
            max_distance: 10.0,
            model: DistanceModel::Inverse,
            ..Default::default()
        },
        ..Default::default()
    };
    let far = h
        .play(
            constant(0.5),
            &Play {
                emitter: Some(emitter),
                priority: 200,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(render(&mut m, 16, 7), vec![0.25; 16]);
    assert_eq!(h.voice_status(far).unwrap().state, VoiceState::Virtual);
    h.set_listener(
        Listener {
            position: [0.0, 0.0, -20.0],
            ..Default::default()
        },
        Fade::NONE,
    )
    .unwrap();
    assert_eq!(render(&mut m, 16, 7), vec![0.5; 16]);
    assert_eq!(h.voice_status(near).unwrap().state, VoiceState::Virtual);
    assert_eq!(h.voice_status(far).unwrap().position, 32);
}

struct Counter {
    at: usize,
    reads: Arc<AtomicUsize>,
}
impl Stream for Counter {
    fn sample_rate(&self) -> u32 {
        48000
    }
    fn channels(&self) -> usize {
        1
    }
    fn frames(&self) -> Option<u64> {
        Some(10000)
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let n = out.len().min(10000 - self.at);
        for (i, x) in out[..n].iter_mut().enumerate() {
            *x = (self.at + i) as f32 / 65536.0;
        }
        self.at += n;
        Ok(n)
    }
    fn seek(&mut self, frame: u64) -> Result<()> {
        self.at = frame.min(10000) as usize;
        Ok(())
    }
}
#[test]
fn stateful_streams_consume_and_discard_then_resume_the_same_sample() {
    let (mut m, mut h) = setup(1, 3);
    let blocker = h
        .play(
            constant(0.5),
            &Play {
                priority: 255,
                ..Default::default()
            },
        )
        .unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let v = h
        .play(
            Sound::stream(Counter {
                at: 0,
                reads: reads.clone(),
            }),
            &Play::default(),
        )
        .unwrap();
    h.at(300).set_gain(blocker, 0.0, Fade::NONE).unwrap();
    let out = render(&mut m, 400, 7);
    assert!(out[..300].iter().all(|&v| v == 0.5));
    for (i, &x) in out[300..].iter().enumerate() {
        assert_eq!(x, (i + 300) as f32 / 65536.0);
    }
    assert_eq!(h.voice_status(v).unwrap().position, 400);
    assert!(reads.load(Ordering::Relaxed) > 1);
}

#[test]
fn pre_fader_audio_routes_are_not_culled_by_a_bus_fader() {
    let (mut m, mut h) = Mixer::new(MixerConfig {
        channels: 1,
        max_voices: 1,
        virtual_voices: Some(VirtualVoices {
            capacity: 4,
            threshold: 0.0,
        }),
        buses: vec!["source".into(), "return".into()],
        ..Default::default()
    })
    .unwrap();
    let source = h.bus("source").unwrap();
    let ret = h.bus("return").unwrap();
    h.set_routing(&[BusRouting {
        output: None,
        sends: vec![BusSend {
            destination: ret,
            gain: 1.0,
            position: SendPosition::PreFader,
        }],
        ..BusRouting::new(source)
    }])
    .unwrap();
    h.set_bus_gain(source, 0.0, Fade::NONE).unwrap();
    h.play(
        constant(0.5),
        &Play {
            bus: source,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(render(&mut m, 40, 7), vec![0.5; 40]);
    assert_eq!(m.physical_voices(), 1);
}

#[test]
fn stop_fade_render_cost_is_bounded_by_physical_budget_plus_legacy_tail_slots() {
    let (mut m, mut h) = setup(2, 32);
    let mut voices = Vec::new();
    for priority in 0..20 {
        voices.push(
            h.play(
                constant(0.01),
                &Play {
                    priority,
                    ..Default::default()
                },
            )
            .unwrap(),
        );
    }
    render(&mut m, 1, 1);
    for v in voices {
        h.stop(v, Fade::linear(0.1)).unwrap();
    }
    for _ in 0..20 {
        h.play(constant(0.01), &Play::default()).unwrap();
    }
    render(&mut m, 100, 7);
    assert!(m.physical_voices() <= 2 + 8);
    assert!(m.active_voices() <= 32 + 8);
}

#[test]
fn maximum_logical_pool_retires_without_allocations_or_frees_during_render() {
    let (mut m, mut h) = Mixer::new(MixerConfig {
        channels: 1,
        max_voices: 1,
        queue: 4096,
        virtual_voices: Some(VirtualVoices {
            capacity: MAX_LOGICAL_VOICES,
            threshold: 0.0,
        }),
        ..Default::default()
    })
    .unwrap();
    let a = pcm(vec![1.0], 48000);
    for _ in 0..MAX_LOGICAL_VOICES {
        h.play(
            a.clone(),
            &Play {
                gain: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
    }
    let mut out = [0.0; 64];
    realtime(|| m.render(&mut out));
    assert!(out.iter().all(|&v| v == 0.0));
    assert_eq!(m.active_voices(), 0);
    assert_eq!(
        events(&mut h)
            .iter()
            .filter(|e| matches!(e, MixerEvent::Ended(_)))
            .count(),
        MAX_LOGICAL_VOICES
    );
}

#[test]
fn virtualization_validation_rejects_unbounded_or_nonfinite_settings() {
    for (capacity, threshold) in [
        (0, 0.0),
        (1, 0.0),
        (MAX_LOGICAL_VOICES + 1, 0.0),
        (4, f32::NAN),
        (4, f32::INFINITY),
        (4, -0.01),
        (4, 1.1),
    ] {
        assert!(
            Mixer::new(MixerConfig {
                max_voices: 2,
                virtual_voices: Some(VirtualVoices {
                    capacity,
                    threshold
                }),
                ..Default::default()
            })
            .is_err()
        );
    }
}

#[test]
fn prepared_sample_peaks_cull_silence_and_quiet_pcm_without_retaining_sources() {
    let (mut m, mut h) = Mixer::new(MixerConfig {
        channels: 1,
        max_voices: 1,
        virtual_voices: Some(VirtualVoices {
            capacity: 3,
            threshold: 0.001,
        }),
        ..Default::default()
    })
    .unwrap();
    let audible = h
        .play(
            constant(0.25),
            &Play {
                priority: 0,
                ..Default::default()
            },
        )
        .unwrap();
    let quiet_audio = constant(0.0001);
    let quiet = h
        .play(
            quiet_audio.clone(),
            &Play {
                priority: 255,
                ..Default::default()
            },
        )
        .unwrap();
    let silent = h
        .play(
            constant(0.0),
            &Play {
                priority: 255,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(render(&mut m, 19, 7), vec![0.25; 19]);
    assert_eq!(h.voice_status(silent).unwrap().state, VoiceState::Virtual);
    assert_eq!(h.voice_status(quiet).unwrap().position, 19);
    h.set_gain(quiet, 20.0, Fade::NONE).unwrap();
    assert_eq!(render(&mut m, 1, 1), vec![0.0001f32 * 20.0]);
    assert_eq!(h.voice_status(audible).unwrap().state, VoiceState::Virtual);
    h.stop_all(Fade::NONE).unwrap();
    realtime(|| m.render(&mut []));
    events(&mut h);
    assert_eq!(
        Arc::strong_count(&quiet_audio),
        1,
        "peak cache retains metadata only"
    );
    // Exceed the metadata-cache capacity with distinct sources, then reuse one.
    for _ in 0..8 {
        h.play(pcm(vec![0.0001], 48000), &Play::default()).unwrap();
        assert_eq!(render(&mut m, 1, 1), vec![0.0]);
        events(&mut h);
    }
    h.play(quiet_audio, &Play::default()).unwrap();
    assert_eq!(render(&mut m, 1, 1), vec![0.0]);
}

#[test]
fn gain_ramps_use_a_conservative_quantum_bound_and_stable_start_order() {
    let run = |block| {
        let (mut m, mut h) = Mixer::new(MixerConfig {
            channels: 1,
            max_voices: 1,
            virtual_voices: Some(VirtualVoices {
                capacity: 4,
                threshold: 0.1,
            }),
            ..Default::default()
        })
        .unwrap();
        h.play(
            constant(0.5),
            &Play {
                priority: 0,
                ..Default::default()
            },
        )
        .unwrap();
        let rising = h
            .play(
                constant(0.5),
                &Play {
                    gain: 0.0,
                    priority: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        h.at(5)
            .set_gain(rising, 1.0, Fade::linear(40.0 / 48000.0))
            .unwrap();
        let out = render(&mut m, 80, block);
        assert!(out[..5].iter().all(|&v| v == 0.5));
        assert_eq!(
            out[5], 0.0,
            "promotion includes the future ramp within this quantum"
        );
        assert_eq!(out[25], 0.25);
        out
    };
    let expected = run(80);
    for block in [1, 7, 32] {
        assert_eq!(run(block), expected);
    }
    let (mut m, mut h) = setup(1, 3);
    let first = h.play(constant(0.5), &Play::default()).unwrap();
    let second = h.play(constant(-0.5), &Play::default()).unwrap();
    assert_eq!(render(&mut m, 100, 7), vec![0.5; 100]);
    assert_eq!(h.voice_status(first).unwrap().state, VoiceState::Playing);
    assert_eq!(h.voice_status(second).unwrap().state, VoiceState::Virtual);
}

#[test]
fn virtual_retirement_after_control_disconnect_stays_off_the_allocator() {
    let (mut m, mut h) = setup(1, 4);
    for _ in 0..4 {
        h.play(pcm(vec![0.0; 8], 48000), &Play::default()).unwrap();
    }
    drop(h);
    let mut out = [0.0; 64];
    realtime(|| m.render(&mut out));
    assert_eq!(m.active_voices(), 0);
    assert_eq!(out, [0.0; 64]);
}

#[cfg(feature = "music")]
#[test]
fn atomic_music_admission_uses_logical_capacity_and_rejects_an_oversized_track() {
    use audiobox::music::{Layer, Music, TimeSignature, Timing, Track, Transport};
    let (mut m, mut h) = setup(1, 3);
    let transport = Transport::new(48000, 120.0, TimeSignature::default(), 0).unwrap();
    let music = Music::new(&mut h, transport, BusId::MASTER).unwrap();
    let track = Track {
        layers: [0.25, 0.5, 0.125]
            .into_iter()
            .map(|x| Layer::new(constant(x)))
            .collect(),
        looping: Loop::Whole,
    };
    let applied = music
        .transition(&mut h, track, 0.0, Timing::NOW, Fade::NONE)
        .unwrap();
    assert_eq!(render(&mut m, 64, 7), vec![0.5; 64]);
    assert_eq!(
        (
            m.active_voices(),
            m.physical_voices(),
            m.virtual_voice_count()
        ),
        (3, 1, 2)
    );
    assert!(events(&mut h).contains(&MixerEvent::MusicApplied(applied, 0)));
    let track = Track {
        layers: (0..4).map(|_| Layer::new(constant(0.75))).collect(),
        looping: Loop::Whole,
    };
    let rejected = music
        .transition(&mut h, track, 0.0, Timing::NOW, Fade::NONE)
        .unwrap();
    assert_eq!(render(&mut m, 64, 7), vec![0.5; 64]);
    assert_eq!(m.active_voices(), 3);
    assert!(events(&mut h).contains(&MixerEvent::MusicRejected(
        rejected,
        ErrorCode::LimitExceeded
    )));
}

fn resampled(quality: ResamplingQuality, block: usize) -> Vec<f32> {
    let (mut m, mut h) = Mixer::new(MixerConfig {
        channels: 1,
        sample_rate: 48000,
        ..Default::default()
    })
    .unwrap();
    h.play(
        pcm((0..32).map(|n| n as f32 / 64.0).collect(), 24000),
        &Play {
            resampling: quality,
            ..Default::default()
        },
    )
    .unwrap();
    render(&mut m, 80, block)
}
#[test]
fn linear_resampling_interpolates_two_samples_and_ends_at_the_source_boundary() {
    let expected = resampled(ResamplingQuality::Linear, 80);
    for (i, &x) in expected[..63].iter().enumerate() {
        assert_eq!(x, i as f32 / 128.0);
    }
    assert_eq!(expected[63], 31.0 / 128.0);
    assert!(expected[64..].iter().all(|&x| x == 0.0));
    for block in [1, 7, 32] {
        assert_eq!(resampled(ResamplingQuality::Linear, block), expected);
    }
    assert_ne!(resampled(ResamplingQuality::Sinc, 80), expected);
    let (mut m, mut h) = setup(1, 2);
    let id = h
        .play(
            pcm(vec![0.25; 32], 24000),
            &Play {
                resampling: ResamplingQuality::Linear,
                ..Default::default()
            },
        )
        .unwrap();
    render(&mut m, 64, 7);
    assert_eq!(m.active_voices(), 0);
    assert!(events(&mut h).contains(&MixerEvent::Ended(id)));
}

#[test]
fn linear_quality_exposes_its_downsampling_alias_tradeoff() {
    let frames = 9600;
    let input: Vec<_> = (0..frames)
        .map(|i| match i % 8 {
            0 | 4 => 0.0,
            1 | 3 => std::f32::consts::FRAC_1_SQRT_2,
            2 => -1.0,
            5 | 7 => -std::f32::consts::FRAC_1_SQRT_2,
            _ => 1.0,
        })
        .collect();
    // A rationally specified 3/8-cycle waveform: 36 kHz at 96 kHz aliases to 12 kHz.
    let run = |q| {
        let (mut m, mut h) = Mixer::new(MixerConfig {
            channels: 1,
            ..Default::default()
        })
        .unwrap();
        h.play(
            pcm(input.clone(), 96000),
            &Play {
                resampling: q,
                ..Default::default()
            },
        )
        .unwrap();
        render(&mut m, frames / 2, 256)
    };
    let linear = run(ResamplingQuality::Linear);
    let sinc = run(ResamplingQuality::Sinc);
    let power = |v: &[f32]| {
        v[32..v.len() - 32]
            .iter()
            .map(|&x| (x as f64).powi(2))
            .sum::<f64>()
            / (v.len() - 64) as f64
    };
    assert!(power(&linear) > 0.4);
    assert!(power(&sinc) < power(&linear) * 0.001);
}
