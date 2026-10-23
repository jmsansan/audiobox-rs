//! Every callback path, including underrun/seek/EOF, allocates and frees nothing.
#![cfg(all(feature = "file-stream", not(target_family = "wasm")))]
use audiobox::stream::FileStream;
#[cfg(feature = "mixer")]
use audiobox::stream::FileStreamOptions;
use audiobox::{Audio, AudioFormat, encode};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
thread_local! {static ACTIVE:Cell<bool>=const {Cell::new(false)};}
fn active() -> bool {
    ACTIVE.try_with(Cell::get).unwrap_or(false)
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if active() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if active() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if active() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, n) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if active() {
            FREES.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;
#[test]
fn reading_seeking_reporting_and_mixing_use_only_retained_memory() {
    let path = std::env::temp_dir().join(format!("audiobox-disk-alloc-{}.wav", std::process::id()));
    let bytes = encode(
        &Audio::new(vec![vec![0.25; 301]], 8000).unwrap(),
        AudioFormat::Wav,
    )
    .unwrap();
    std::fs::write(&path, bytes).unwrap();
    let (mut source, handle) = FileStream::open(&path).unwrap();
    let mut buffer = [0.0; 512];
    #[cfg(feature = "mixer")]
    let (mut mixer, mut control, reader) = {
        use audiobox::mixer::*;
        let (source, reader) = FileStream::with_options(
            &path,
            &FileStreamOptions {
                buffer_frames: 17,
                ..Default::default()
            },
        )
        .unwrap();
        let (mixer, mut control) = Mixer::new(MixerConfig {
            sample_rate: 8000,
            channels: 1,
            ..Default::default()
        })
        .unwrap();
        control
            .play(
                Sound::stream(source),
                &Play {
                    looping: Loop::Whole,
                    ..Default::default()
                },
            )
            .unwrap();
        (mixer, control, reader)
    };
    ACTIVE.set(true);
    let first = source.read(&mut buffer).unwrap();
    let end = source.read(&mut buffer).unwrap();
    for i in 0..2000 {
        if i % 17 == 0 {
            source.seek((i % 300) as u64).unwrap();
        }
        let _ = source.read(&mut buffer);
        let _ = handle.status();
        #[cfg(feature = "mixer")]
        mixer.render(&mut buffer);
    }
    handle.cancel();
    let _ = source.read(&mut buffer);
    ACTIVE.set(false);
    assert_eq!((first, end), (301, 0));
    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0);
    assert_eq!(FREES.load(Ordering::Relaxed), 0);
    #[cfg(feature = "mixer")]
    {
        drop(mixer);
        while control.poll_event().is_some() {}
        drop(control);
        drop(reader);
    }
    drop(source);
    drop(handle);
    std::fs::remove_file(path).unwrap();
}
