//! Checks that rendering allocates nothing, so it can run on an audio thread.
#![cfg(feature = "mixer")]
use audiobox::mixer::*;
use audiobox::stream::Decoder;
use audiobox::{Audio, AudioFormat, FadeCurve, encode};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    // Only the rendering thread counts: the test harness allocates on its own threads.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}
fn counting() -> bool {
    COUNTING.try_with(Cell::get).unwrap_or(false)
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if counting() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if counting() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if counting() {
            FREES.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if counting() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;

fn tone(frames: usize, rate: u32, hz: f32) -> Vec<f32> {
    (0..frames)
        .map(|i| (i as f32 * hz / rate as f32 * std::f32::consts::TAU).sin() * 0.3)
        .collect()
}

#[test]
fn rendering_allocates_and_frees_nothing() {
    let (mut mixer, mut handle) = Mixer::new(MixerConfig {
        max_voices: 3,
        buses: vec!["music".into(), "effects".into()],
        ..MixerConfig::default()
    })
    .unwrap();
    let music = handle.bus("music").unwrap();
    let effects = handle.bus("effects").unwrap();
    let flac = encode(
        &Audio::new(vec![tone(20_000, 44_100, 330.0)], 44_100).unwrap(),
        AudioFormat::Flac,
    )
    .unwrap();
    let mp3: &'static [u8] = include_bytes!("fixtures/lame-stereo.mp3");
    let blip = Arc::new(Audio::new(vec![tone(3000, 22_050, 880.0)], 22_050).unwrap());
    let same_rate = Arc::new(
        Audio::new(
            vec![tone(2000, 48_000, 440.0), tone(2000, 48_000, 660.0)],
            48_000,
        )
        .unwrap(),
    );

    let song = handle
        .play(
            Decoder::new(mp3).unwrap(),
            &Play {
                bus: music,
                looping: Loop::Whole,
                priority: 255,
                ..Play::default()
            },
        )
        .unwrap();
    handle
        .play(
            Decoder::new(flac).unwrap(),
            &Play {
                bus: music,
                looping: Loop::Region {
                    start: 1000,
                    end: 15_000,
                },
                ..Play::default()
            },
        )
        .unwrap();
    #[cfg(feature = "synth")]
    {
        use audiobox::synth::{RenderOptions, Score, Synth};
        let score: Score = "instrument p wave=pulse vibrato=20,100\nchannel a = @p o5 L [c e g]4"
            .parse()
            .unwrap();
        let synth = Synth::new(
            &score,
            48_000,
            &RenderOptions {
                loops: None,
                ..Default::default()
            },
        )
        .unwrap();
        handle
            .play(
                synth,
                &Play {
                    bus: music,
                    ..Play::default()
                },
            )
            .unwrap();
    }
    // Effects that start, steal, fade, pan and end while counting.
    for i in 0..12u64 {
        let sound = if i % 2 == 0 {
            blip.clone()
        } else {
            same_rate.clone()
        };
        let id = handle
            .at(i * 3000)
            .play(
                sound,
                &Play {
                    bus: effects,
                    pan: -0.8,
                    fade_in: Fade::linear(0.01),
                    ..Play::default()
                },
            )
            .unwrap();
        handle
            .at(i * 3000 + 500)
            .set_pan(id, 0.8, Fade::new(0.02, FadeCurve::EqualPower))
            .unwrap();
        handle
            .at(i * 3000 + 700)
            .set_gain(id, 0.5, Fade::linear(0.01))
            .unwrap();
        if i % 3 == 0 {
            handle
                .at(i * 3000 + 900)
                .stop(id, Fade::linear(0.005))
                .unwrap();
        }
    }
    handle
        .at(20_000)
        .set_bus_gain(music, 0.4, Fade::linear(0.5))
        .unwrap();
    handle
        .at(30_000)
        .set_bus_gain(BusId::MASTER, 0.8, Fade::linear(0.1))
        .unwrap();
    handle.at(60_000).stop(song, Fade::linear(0.2)).unwrap();

    let mut buffer = vec![0.0f32; 2 * 480];
    let mut loudest = 0.0f32;
    COUNTING.set(true);
    for _ in 0..200 {
        mixer.render(&mut buffer);
        loudest = buffer.iter().fold(loudest, |m, s| m.max(s.abs()));
    }
    COUNTING.set(false);
    assert_eq!(ALLOCATIONS.load(Ordering::SeqCst), 0, "rendering allocated");
    assert_eq!(FREES.load(Ordering::SeqCst), 0, "rendering freed memory");
    // Finished voices came back to the handle, which frees them here.
    let events: Vec<_> = std::iter::from_fn(|| handle.poll_event()).collect();
    assert!(
        events.iter().any(|e| matches!(e, MixerEvent::Stolen(_))),
        "{events:?}"
    );
    assert!(events.contains(&MixerEvent::Ended(song)));
    assert!(loudest > 0.1);
}
