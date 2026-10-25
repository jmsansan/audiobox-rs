//! Whole-file FLAC output growth (#82): amortized, exact when STREAMINFO declares
//! the length, and bounded by limits when it does not.
use audiobox::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default)]
struct Stats {
    allocations: usize,
    reallocations: usize,
    moved_bytes: usize,
    live: usize,
    peak: usize,
    requested_bytes: usize,
}
thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static STATS: Cell<Stats> = const { Cell::new(Stats { allocations: 0, reallocations: 0, moved_bytes: 0, live: 0, peak: 0, requested_bytes: 0 }) };
    static FAIL_ABOVE: Cell<usize> = const { Cell::new(usize::MAX) };
}
struct Tracking;
fn tracked() -> bool {
    TRACK.try_with(Cell::get).unwrap_or(false)
}
fn record(size: usize, old: usize, moved: bool) {
    STATS.with(|stats| {
        let mut s = stats.get();
        s.allocations += usize::from(old == 0);
        s.reallocations += usize::from(old != 0);
        s.moved_bytes += if moved { old } else { 0 };
        s.requested_bytes += size;
        s.live = s.live.saturating_sub(old) + size;
        s.peak = s.peak.max(s.live);
        stats.set(s);
    });
}
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if tracked() && layout.size() > FAIL_ABOVE.get() {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.alloc(layout) };
        if tracked() && !p.is_null() {
            record(layout.size(), 0, false);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if tracked() && layout.size() > FAIL_ABOVE.get() {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.alloc_zeroed(layout) };
        if tracked() && !p.is_null() {
            record(layout.size(), 0, false);
        }
        p
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if tracked() && size > FAIL_ABOVE.get() {
            return std::ptr::null_mut();
        }
        let next = unsafe { System.realloc(p, layout, size) };
        if tracked() && !next.is_null() {
            record(size, layout.size(), next != p);
        }
        next
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        if tracked() {
            STATS.with(|stats| {
                let mut s = stats.get();
                s.live = s.live.saturating_sub(layout.size());
                stats.set(s);
            });
        }
        unsafe { System.dealloc(p, layout) }
    }
}
#[global_allocator]
static GLOBAL: Tracking = Tracking;

fn measure(bytes: &[u8], options: &DecodeOptions, fail_above: usize) -> (Result<Audio>, Stats) {
    STATS.set(Stats::default());
    FAIL_ABOVE.set(fail_above);
    TRACK.set(true);
    let audio = decode_with_options(bytes, options);
    TRACK.set(false);
    FAIL_ABOVE.set(usize::MAX);
    (audio, STATS.get())
}

/// A deterministic stereo signal that FLAC cannot store verbatim.
fn flac(seconds: f64) -> (Vec<u8>, usize) {
    let frames = (seconds * 48_000.0) as usize;
    let data = (0..2)
        .map(|c| {
            (0..frames)
                .map(|i| (i as f32 * (0.013 + c as f32 * 0.004)).sin() * 0.5)
                .collect()
        })
        .collect();
    let audio = Audio::new(data, 48_000).unwrap();
    (encode(&audio, AudioFormat::Flac).unwrap(), frames)
}

/// Sets STREAMINFO's 36-bit total sample count.
fn declare(bytes: &mut [u8], total: u64) {
    bytes[21] = bytes[21] & 0xf0 | (total >> 32) as u8;
    bytes[22..26].copy_from_slice(&(total as u32).to_be_bytes());
}

fn incremental(bytes: &[u8]) -> Vec<Vec<u32>> {
    let mut decoder = stream::Decoder::new(bytes).unwrap();
    let mut buffer = [0.0f32; 8192];
    let mut out = vec![Vec::new(), Vec::new()];
    loop {
        let n = decoder.read(&mut buffer).unwrap();
        if n == 0 {
            return out;
        }
        for frame in buffer[..n * 2].chunks(2) {
            out[0].push(frame[0].to_bits());
            out[1].push(frame[1].to_bits());
        }
    }
}

#[test]
fn growth_is_amortized_and_a_declared_length_is_exact() {
    let (bytes, frames) = flac(30.0);
    let options = DecodeOptions::default();
    let (audio, stats) = measure(&bytes, &options, usize::MAX);
    let audio = audio.unwrap();
    assert_eq!(audio.frames(), frames);
    // Exact growth per 4096-frame block made 352 reallocations per channel here.
    assert!(stats.reallocations < 60, "{stats:?}");
    // Growth stops at the declared total, so the output is exactly sized.
    for c in audio.all_channels() {
        assert_eq!(c.capacity(), frames);
    }
    let bits: Vec<Vec<u32>> = audio
        .all_channels()
        .iter()
        .map(|c| c.iter().map(|s| s.to_bits()).collect())
        .collect();
    assert_eq!(bits, incremental(&bytes));
}

#[test]
fn an_absent_declared_length_still_grows_geometrically_within_limits() {
    let (mut bytes, frames) = flac(30.0);
    declare(&mut bytes, 0);
    let (audio, stats) = measure(&bytes, &DecodeOptions::default(), usize::MAX);
    let audio = audio.unwrap();
    assert_eq!(audio.frames(), frames);
    assert!(stats.reallocations < 60, "{stats:?}");
    for c in audio.all_channels() {
        assert!(
            c.capacity() < frames + frames / 2 + 8192,
            "{}",
            c.capacity()
        );
    }
    // Speculative growth stops at the frame limit, which still admits the stream.
    let options = DecodeOptions {
        limits: Limits {
            max_frames: frames,
            ..Limits::default()
        },
        ..DecodeOptions::default()
    };
    let audio = decode_with_options(&bytes, &options).unwrap();
    assert!(audio.all_channels().iter().all(|c| c.capacity() == frames));
    let options = DecodeOptions {
        limits: Limits {
            max_frames: frames - 1,
            ..Limits::default()
        },
        ..DecodeOptions::default()
    };
    let error = decode_with_options(&bytes, &options).unwrap_err();
    assert_eq!(error.code, ErrorCode::LimitExceeded);
}

#[test]
fn wrong_declared_lengths_keep_their_errors_without_trusting_them() {
    let (mut bytes, frames) = flac(5.0);
    // A declared total above the actual length is not an allocation hint.
    declare(&mut bytes, frames as u64 * 40);
    let (audio, stats) = measure(&bytes, &DecodeOptions::default(), usize::MAX);
    assert_eq!(audio.unwrap_err().code, ErrorCode::DecodeError);
    assert!(stats.peak < frames * 8 * 3, "{stats:?}");
    // Fewer declared samples than the stream holds is an error, as before.
    declare(&mut bytes, frames as u64 / 2);
    assert_eq!(decode(&bytes).unwrap_err().code, ErrorCode::DecodeError);
}

#[test]
fn a_failed_output_allocation_is_a_limit_error() {
    let (bytes, _) = flac(5.0);
    let (audio, _) = measure(&bytes, &DecodeOptions::default(), 256 * 1024);
    assert_eq!(audio.unwrap_err().code, ErrorCode::LimitExceeded);
}
