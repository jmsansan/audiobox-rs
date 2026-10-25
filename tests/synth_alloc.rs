//! Checks that incremental synthesis allocates nothing, so it can run on an audio thread.
#![cfg(feature = "synth")]
use audiobox::synth::{RenderOptions, Score, Synth};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    // Only the rendering thread counts: the test harness allocates on its own threads.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}
fn count() {
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            FREES.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
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
         instrument kick drum=kick kit=electronic
         instrument snare drum=snare kit=chiptune
         instrument open drum=open-hat
         instrument closed drum=closed-hat kit=chiptune
         instrument tom drum=tom
         instrument clap drum=clap kit=chiptune
         instrument crash drum=crash
         channel c = l16 L [@hat o7 c @hiss o6 c]8
         channel d = @kick l8 L [c!127 c!80]8
         channel e = @snare l8 L [r c!96]8
         channel f = l16 L [@open c!100 @closed c!40]16
         channel g = l8 L [@tom c @clap c!64]8
         channel h = @crash L c1",
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
    let chip_score = Score::parse(
        "tempo 180
         instrument p quantize=ntsc-pulse macro-duty=0,1,2,3 macro-duty-loop=0 macro-arpeggio=0,7,12 macro-arpeggio-loop=0 macro-pitch=0,-10,10 macro-pitch-loop=0 macro-volume=0,15,7 macro-volume-loop=0
         instrument w wavetable=0123456789abcdef macro-volume=15,7,0 macro-volume-loop=0
         instrument n wave=noise noise=short noise-period=0 macro-volume=15,0 macro-volume-loop=0
         instrument s sample=00040f08 sample-rate=1000 sample-loop=on
         instrument one sample=0f070300 sample-rate=1000 adsr=2,0,100,10
         instrument wv wavetable=0123456789abcdef voices=3 adsr=2,40,50,200
         channel wv=@wv l16 L [{c e} g c {d f}]12
         channel p=@p l16 L [c e g]16
         channel w=@w l8 L [c g]12
         channel n=@n l16 L [c r]24
         channel s=@s l8 L [c g]12
         channel one=@one l16 L [c r]24",
    ).unwrap();
    let mut chip_synth = Synth::new(
        &chip_score,
        48_000,
        &RenderOptions {
            loops: None,
            ..Default::default()
        },
    )
    .unwrap();
    let wide_score=Score::parse("tempo 240\ninstrument pad wave=saw unison=3 detune=15 spread=100 voices=3 filter=lowpass cutoff=1500 resonance=200 filter-env=2400 filter-adsr=10,30,50,20 lfo-cutoff=beats,1,2,600 adsr=2,30,70,600\ninstrument lead voices=1 legato=on glide=80 lfo-pan=2000,50\nchannel p=@pad l16 {c e g} L [{c f a} {d f a} {e g b}]2\nchannel l=@lead l16 c L l8 [e g c]2").unwrap();
    let mut wide_synth = Synth::new_stereo(
        &wide_score,
        48000,
        &RenderOptions {
            loops: None,
            ..Default::default()
        },
    )
    .unwrap();
    let mut wide_mono = Synth::new(
        &wide_score,
        48000,
        &RenderOptions {
            loops: None,
            ..Default::default()
        },
    )
    .unwrap();
    let mut buffer = vec![0.0f32; 480];
    COUNTING.set(true);
    for _ in 0..500 {
        synth.fill(&mut buffer);
        chip_synth.fill(&mut buffer);
        wide_synth.fill(&mut buffer);
        wide_mono.fill(&mut buffer);
    }
    synth.reset();
    synth.fill(&mut buffer);
    chip_synth.reset();
    chip_synth.fill(&mut buffer);
    wide_synth.reset();
    wide_synth.fill(&mut buffer);
    wide_mono.reset();
    wide_mono.fill(&mut buffer);
    COUNTING.set(false);
    assert_eq!(ALLOCATIONS.load(Ordering::SeqCst), 0);
    assert_eq!(FREES.load(Ordering::SeqCst), 0);
    assert!(buffer.iter().any(|&s| s != 0.0));
}
