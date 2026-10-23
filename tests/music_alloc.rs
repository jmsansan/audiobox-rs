//! Musical operation containers and rejected groups must be freed off audio.
#![cfg(feature = "music")]
use audiobox::Audio;
use audiobox::mixer::{BusId, Fade, Loop, Mixer, MixerConfig, MixerEvent};
use audiobox::music::{
    Layer, MAX_LAYERS, Music, Quantize, TimeSignature, Timing, Track, Transport,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
thread_local! { static COUNTING: Cell<bool> = const { Cell::new(false) }; }
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
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if counting() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn transitions_layers_cues_and_rejected_groups_allocate_and_free_nothing_in_render() {
    let rate = 1000;
    let (mut mixer, mut handle) = Mixer::new(MixerConfig {
        sample_rate: rate,
        channels: 1,
        max_voices: MAX_LAYERS,
        queue: 128,
        ..Default::default()
    })
    .unwrap();
    let transport = Transport::new(rate, 120.0, TimeSignature::default(), 0).unwrap();
    let music = Music::new(&mut handle, transport, BusId::MASTER).unwrap();
    let audio = Arc::new(Audio::new(vec![vec![0.01; 64]], rate).unwrap());
    let track = || Track {
        layers: (0..MAX_LAYERS).map(|_| Layer::new(audio.clone())).collect(),
        looping: Loop::Whole,
    };
    let at = |frame| Timing {
        not_before: frame,
        quantize: Quantize::Immediate,
    };
    for i in 0..10 {
        music
            .transition(&mut handle, track(), 0.0, at(i * 200), Fade::NONE)
            .unwrap();
        music
            .set_intensity(&mut handle, 0.5, at(i * 200 + 50), Fade::linear(0.01))
            .unwrap();
    }
    let rejected_cue = music.cue(&mut handle, audio.clone(), 1.0, at(10)).unwrap();
    let rejected_track = music
        .transition(&mut handle, track(), 0.0, at(100), Fade::linear(0.01))
        .unwrap();
    music
        .stop(&mut handle, at(2000), Fade::linear(0.01))
        .unwrap();
    let cue = music.cue(&mut handle, audio, 1.0, at(2100)).unwrap();
    let mut out = [0.0; 17];
    COUNTING.set(true);
    for _ in 0..150 {
        mixer.render(&mut out);
    }
    COUNTING.set(false);
    assert_eq!(ALLOCATIONS.load(Ordering::SeqCst), 0, "render allocated");
    assert_eq!(FREES.load(Ordering::SeqCst), 0, "render freed memory");
    assert_eq!(mixer.active_voices(), 0);
    let events: Vec<_> = std::iter::from_fn(|| handle.poll_event()).collect();
    assert!(events.contains(&MixerEvent::MusicRejected(
        rejected_cue,
        audiobox::ErrorCode::LimitExceeded
    )));
    assert!(events.contains(&MixerEvent::MusicRejected(
        rejected_track,
        audiobox::ErrorCode::LimitExceeded
    )));
    assert!(events.contains(&MixerEvent::MusicApplied(cue, 2100)));
}
