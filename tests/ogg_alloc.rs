use audiobox::stream::Decoder;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
thread_local! { static ON: Cell<bool> = const { Cell::new(false) }; static ALLOC: Cell<usize> = const { Cell::new(0) }; static FREE: Cell<usize> = const { Cell::new(0) }; }
struct Count;
fn mark(key: &'static std::thread::LocalKey<Cell<usize>>) {
    if ON.try_with(Cell::get).unwrap_or(false) {
        key.with(|n| n.set(n.get() + 1));
    }
}
unsafe impl GlobalAlloc for Count {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        mark(&ALLOC);
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        mark(&ALLOC);
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        mark(&ALLOC);
        unsafe { System.realloc(p, l, n) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        mark(&FREE);
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static GLOBAL: Count = Count;
#[test]
fn vorbis_read_seek_and_rewind_reuse_all_prepared_memory() {
    for bytes in [
        include_bytes!("fixtures/ogg/vorbis-floor0.ogg").as_slice(),
        include_bytes!("fixtures/ogg/vorbis-floor0-even.ogg").as_slice(),
        include_bytes!("fixtures/ogg/vorbis-floor0-vector.ogg").as_slice(),
        include_bytes!("fixtures/ogg/vorbis-stereo-q4.ogg").as_slice(),
        include_bytes!("fixtures/ogg/vorbis-8ch.ogg").as_slice(),
    ] {
        let mut d = Decoder::new(bytes).unwrap();
        let channels = d.channels();
        let mut out = vec![0.0; 37 * channels];
        ALLOC.with(|n| n.set(0));
        FREE.with(|n| n.set(0));
        ON.with(|n| n.set(true));
        while d.read(&mut out).unwrap() != 0 {}
        for at in [0, 1, 128, 573, 8850, 9637, 99] {
            d.seek(at).unwrap();
            d.read(&mut out).unwrap();
        }
        d.rewind().unwrap();
        d.read(&mut out).unwrap();
        ON.with(|n| n.set(false));
        assert_eq!(ALLOC.with(Cell::get), 0, "read/seek allocated");
        assert_eq!(FREE.with(Cell::get), 0, "read/seek freed");
    }
}
