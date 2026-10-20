//! Checks that the incremental decoder's memory is bounded and that reading allocates nothing.
use audiobox::stream::Decoder;
use audiobox::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Tracking;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    // Only this test's thread is tracked: the harness allocates on its own threads.
    static TRACKING: Cell<bool> = const { Cell::new(false) };
}
fn tracking() -> bool {
    TRACKING.try_with(Cell::get).unwrap_or(false)
}
fn grow(size: usize) {
    ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
    let live = LIVE.fetch_add(size, Ordering::SeqCst) + size;
    PEAK.fetch_max(live, Ordering::SeqCst);
}
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if tracking() {
            grow(layout.size());
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if tracking() {
            grow(layout.size());
        }
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if tracking() {
            LIVE.fetch_sub(
                layout.size().min(LIVE.load(Ordering::SeqCst)),
                Ordering::SeqCst,
            );
        }
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if tracking() {
            LIVE.fetch_sub(
                layout.size().min(LIVE.load(Ordering::SeqCst)),
                Ordering::SeqCst,
            );
            grow(size);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static GLOBAL: Tracking = Tracking;

fn start() {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    LIVE.store(0, Ordering::SeqCst);
    PEAK.store(0, Ordering::SeqCst);
    TRACKING.set(true);
}
fn stop() -> (usize, usize) {
    TRACKING.set(false);
    (
        PEAK.load(Ordering::SeqCst),
        ALLOCATIONS.load(Ordering::SeqCst),
    )
}

#[test]
fn memory_is_bounded_and_reads_do_not_allocate() {
    // 8 seconds of stereo: 2.8 MB as f32, far more than the decoder may hold.
    let rate = 44_100;
    let frames = 8 * rate as usize;
    let wave = |hz: f32| -> Vec<f32> {
        (0..frames)
            .map(|i| (i as f32 * hz / rate as f32 * std::f32::consts::TAU).sin() * 0.4)
            .collect()
    };
    let audio = Audio::new(vec![wave(440.0), wave(554.0)], rate).unwrap();
    let streams = [
        ("wav", encode(&audio, AudioFormat::Wav).unwrap()),
        ("flac", encode(&audio, AudioFormat::Flac).unwrap()),
        ("mp3", encode(&audio, AudioFormat::Mp3).unwrap()),
        (
            "lame mp3",
            include_bytes!("fixtures/lame-stereo.mp3").to_vec(),
        ),
        (
            "ima wav",
            include_bytes!("fixtures/ima-stereo.wav").to_vec(),
        ),
    ];
    let mut buffer = vec![0.0f32; 2 * 1024];
    for (name, bytes) in &streams {
        start();
        let mut decoder = Decoder::new(bytes.as_slice()).unwrap();
        let (opened, _) = stop();
        // The first read and a seek may finish warming up; later reads must not allocate.
        decoder.read(&mut buffer).unwrap();
        decoder.seek(decoder.frames().unwrap() / 2).unwrap();
        decoder.rewind().unwrap();
        start();
        let mut total = 0;
        loop {
            let n = decoder.read(&mut buffer).unwrap();
            if n == 0 {
                break;
            }
            total += n;
        }
        decoder.rewind().unwrap();
        decoder.read(&mut buffer).unwrap();
        let (peak, allocations) = stop();
        assert_eq!(total as u64, decoder.frames().unwrap(), "{name}");
        assert_eq!(allocations, 0, "{name}: reading allocated");
        assert_eq!(peak, 0, "{name}");
        assert!(
            opened < 256 * 1024,
            "{name}: the decoder holds {opened} bytes"
        );
        println!(
            "{name}: {} input bytes, {opened} decoder bytes",
            bytes.len()
        );
    }
}
