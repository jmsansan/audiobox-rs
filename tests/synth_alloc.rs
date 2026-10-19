//! Checks that incremental synthesis allocates nothing, so it can run on an audio thread.
#![cfg(feature = "synth")]
use audiobox::synth::{RenderOptions, Score, Synth};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Counting;
static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::SeqCst) {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::SeqCst) {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn fill_does_not_allocate() {
    let score = Score::parse(
        "tempo 180
         instrument lead wave=pulse duty=12.5,25,50 duty-step=20 adsr=3,40,60,30 vibrato=30,120,50
         instrument arp wave=saw adsr=0,0,100,10 arpeggio=0,4,7 slide=20
         instrument hat wave=noise noise=short adsr=0,20,0,0
         instrument hiss wave=noise adsr=0,60,0,0
         channel a = @lead o5 l16 L [c d e g]4
         channel b = @arp o3 l8 L [c g]4
         channel c = l16 L [@hat o7 c @hiss o6 c]8",
    )
    .unwrap();
    let mut synth = Synth::new(
        &score,
        48_000,
        &RenderOptions {
            loops: None,
            ..Default::default()
        },
    )
    .unwrap();
    let mut buffer = vec![0.0f32; 480];
    COUNTING.store(true, Ordering::SeqCst);
    for _ in 0..500 {
        synth.fill(&mut buffer);
    }
    synth.reset();
    synth.fill(&mut buffer);
    COUNTING.store(false, Ordering::SeqCst);
    assert_eq!(ALLOCATIONS.load(Ordering::SeqCst), 0);
    assert!(buffer.iter().any(|&s| s != 0.0));
}
