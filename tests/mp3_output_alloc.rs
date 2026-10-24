//! Whole-file MP3 allocation regression and opt-in release profiling.
use audiobox::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

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

// MPEG-1 Layer III, 48 kHz, 128 kbit/s stereo, zero side information and
// main data: valid synthetic silence. Each 384-byte frame emits 1152 samples.
fn silence(seconds: usize, tagged: bool) -> Vec<u8> {
    let count = (seconds * 48_000).div_ceil(1152);
    let mut frame = [0u8; 384];
    frame[..4].copy_from_slice(&[0xff, 0xfb, 0x94, 0]);
    let mut bytes = Vec::with_capacity((count + usize::from(tagged)) * frame.len());
    if tagged {
        let mut tag = frame;
        tag[36..40].copy_from_slice(b"Xing");
        tag[40..44].copy_from_slice(&1u32.to_be_bytes());
        tag[44..48].copy_from_slice(&(count as u32).to_be_bytes());
        tag[48..57].copy_from_slice(b"LAME3.100");
        // Delay 576, padding 1000: exercises both ends of gapless trimming.
        tag[69..72].copy_from_slice(&[36, 3, 232]);
        bytes.extend_from_slice(&tag);
    }
    for _ in 0..count {
        bytes.extend_from_slice(&frame);
    }
    bytes
}

// Repeat the repository's MIT synthetic 3 s LAME sine at complete frame
// boundaries, excluding ID3/Xing on repeats. Reservoir state remains valid.
fn sine(seconds: usize, tagged: bool) -> Vec<u8> {
    let source = include_bytes!("fixtures/lame-stereo.mp3");
    let at = 10
        + source[6..10]
            .iter()
            .fold(0usize, |n, &b| (n << 7) | b as usize);
    fn length(b: &[u8]) -> usize {
        const RATES: [usize; 16] = [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
        ];
        144_000 * RATES[(b[2] >> 4) as usize] / 44_100 + ((b[2] >> 1) & 1) as usize
    }
    let first_end = at + length(&source[at..]);
    let mut frames = Vec::new();
    let mut pos = first_end;
    while pos + 4 <= source.len() && source[pos] == 0xff && source[pos + 1] & 0xe0 == 0xe0 {
        let end = pos + length(&source[pos..]);
        frames.push(&source[pos..end]);
        pos = end;
    }
    let count = (seconds * 44_100).div_ceil(1152);
    let mut bytes = if tagged {
        source[at..first_end].to_vec()
    } else {
        Vec::new()
    };
    if tagged {
        bytes[44..48].copy_from_slice(&(count as u32).to_be_bytes());
    }
    for i in 0..count {
        bytes.extend_from_slice(frames[i % frames.len()]);
    }
    bytes
}
fn measure(bytes: &[u8], fail_above: usize) -> (Result<Audio>, Stats, std::time::Duration) {
    STATS.set(Stats::default());
    FAIL_ABOVE.set(fail_above);
    TRACK.set(std::env::var_os("AUDIOBOX_PROFILE_UNTRACKED").is_none());
    let start = Instant::now();
    let audio = decode(bytes);
    let elapsed = start.elapsed();
    TRACK.set(false);
    FAIL_ABOVE.set(usize::MAX);
    (audio, STATS.get(), elapsed)
}

#[test]
fn whole_file_growth_is_amortized_and_trimming_reuses_pcm() {
    decode(include_bytes!("fixtures/lame-stereo.mp3")).unwrap(); // warm tables
    for make in [silence, sine] {
        for tagged in [false, true] {
            let bytes = make(3, tagged);
            let (audio, stats, _) = measure(&bytes, usize::MAX);
            let audio = audio.unwrap();
            assert!(stats.reallocations < 40, "{tagged}: {stats:?}");
            let capacity: usize = audio.all_channels().iter().map(|c| c.capacity() * 4).sum();
            assert!(
                stats.peak < capacity + 64 * 1024,
                "{stats:?}, output capacity {capacity}"
            );
            // Every sample and both gapless boundaries also match the incremental path.
            let mut decoder = audiobox::stream::Decoder::new(&bytes).unwrap();
            let mut buffer = [0.0; 2304];
            let mut position = 0;
            loop {
                let n = decoder.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }
                for i in 0..n {
                    for c in 0..2 {
                        assert_eq!(
                            buffer[2 * i + c].to_bits(),
                            audio.all_channels()[c][position + i].to_bits()
                        );
                    }
                }
                position += n;
            }
            assert_eq!(position, audio.frames());
        }
    }
    let bytes = silence(2, false);
    let (audio, _, _) = measure(&bytes, 64 * 1024);
    assert_eq!(
        audio.expect_err("expected decode failure").code,
        ErrorCode::LimitExceeded
    );
}

#[test]
fn limits_and_hostile_lengths_preserve_decode_contracts() {
    let raw = silence(1, false);
    let total = raw.len() / 384 * 1152 + 64;
    for tagged in [false, true] {
        let bytes = silence(1, tagged);
        let expected = decode(&bytes).unwrap();
        for limits in [
            Limits {
                max_decoded_bytes: total * 8,
                ..Default::default()
            },
            Limits {
                max_frames: total,
                ..Default::default()
            },
            Limits {
                max_duration_seconds: total as f64 / 48_000.0,
                ..Default::default()
            },
        ] {
            let options = DecodeOptions {
                limits,
                ..Default::default()
            };
            let audio = decode_with_options(&bytes, &options).unwrap();
            assert_eq!(audio.frames(), expected.frames());
            assert!(audio.all_channels().iter().all(|c| c.capacity() <= total));
            let mut too_small = options.clone();
            too_small.limits.max_decoded_bytes = (total - 1) * 8;
            assert_eq!(
                decode_with_options(&bytes, &too_small)
                    .expect_err("expected decode failure")
                    .code,
                ErrorCode::LimitExceeded
            );
            too_small = options.clone();
            too_small.limits.max_frames = total - 1;
            assert_eq!(
                decode_with_options(&bytes, &too_small)
                    .expect_err("expected decode failure")
                    .code,
                ErrorCode::LimitExceeded
            );
            too_small = options.clone();
            too_small.limits.max_duration_seconds = (total - 1) as f64 / 48_000.0;
            assert_eq!(
                decode_with_options(&bytes, &too_small)
                    .expect_err("expected decode failure")
                    .code,
                ErrorCode::LimitExceeded
            );
        }
    }
    let mut bytes = silence(1, true);
    for declared in [0u32, 1, 40_000] {
        bytes[44..48].copy_from_slice(&declared.to_be_bytes());
        let (audio, stats, _) = measure(&bytes, usize::MAX);
        assert_eq!(audio.unwrap().frames(), total - 64 - 576 - 1000);
        // Even a length tag within default limits is not trusted as an allocation hint.
        assert!(stats.peak < 1024 * 1024, "{stats:?}");
    }
    bytes[44..48].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(
        decode(&bytes).expect_err("expected decode failure").code,
        ErrorCode::LimitExceeded
    );
    bytes[44..48].copy_from_slice(&1u32.to_be_bytes());
    bytes[8] = 0xff; // Invalid big_values pair count (511 > 288).
    bytes[9] = 0x80;
    assert_eq!(
        decode(&bytes).expect_err("expected decode failure").code,
        ErrorCode::DecodeError
    );
    let mut truncated = raw.clone();
    truncated.pop();
    assert_eq!(
        decode(&truncated)
            .expect_err("expected decode failure")
            .code,
        ErrorCode::DecodeError
    );
    let mut changed = raw;
    changed[768 + 2] = 0x90;
    assert_eq!(
        decode(&changed).expect_err("expected decode failure").code,
        ErrorCode::DecodeError
    );
}

#[test]
#[ignore = "manual release benchmark; no absolute-time assertions"]
fn profile_whole_mp3() {
    decode(include_bytes!("fixtures/lame-stereo.mp3")).unwrap();
    let runs = std::env::var("AUDIOBOX_PROFILE_RUNS")
        .ok()
        .map(|v| v.parse::<usize>().expect("positive run count"))
        .unwrap_or(3);
    let selected = std::env::var("AUDIOBOX_PROFILE_KIND").unwrap_or_default();
    let seconds_filter = std::env::var("AUDIOBOX_PROFILE_SECONDS").unwrap_or_default();
    let tag_filter = std::env::var("AUDIOBOX_PROFILE_TAGGED").unwrap_or_default();
    println!(
        "kind,tagged,seconds,run,input_bytes,frames,ms,allocs,reallocs,moved_bytes,requested_bytes,peak_bytes,capacity_bytes,checksum"
    );
    for (kind, make) in [
        ("silence48k128", silence as fn(usize, bool) -> Vec<u8>),
        ("lame_sine44k_vbr", sine),
    ] {
        if !selected.is_empty() && selected != kind {
            continue;
        }
        for tagged in [false, true] {
            if !tag_filter.is_empty() && tag_filter != tagged.to_string() {
                continue;
            }
            for seconds in [15, 60, 120, 257] {
                if !seconds_filter.is_empty() && seconds_filter != seconds.to_string() {
                    continue;
                }
                let bytes = make(seconds, tagged); // outside timing/tracking
                for run in 1..=runs {
                    let (audio, s, elapsed) = measure(&bytes, usize::MAX);
                    let audio = audio.unwrap();
                    let capacity: usize =
                        audio.all_channels().iter().map(|c| c.capacity() * 4).sum();
                    let checksum = audio.all_channels().iter().flatten().fold(0u64, |hash, x| {
                        hash.wrapping_mul(31).wrapping_add(x.to_bits() as u64)
                    });
                    println!(
                        "{kind},{tagged},{seconds},{run},{},{},{:.3},{},{},{},{},{},{},{checksum:016x}",
                        bytes.len(),
                        audio.frames(),
                        elapsed.as_secs_f64() * 1000.0,
                        s.allocations,
                        s.reallocations,
                        s.moved_bytes,
                        s.requested_bytes,
                        s.peak,
                        capacity
                    );
                }
            }
        }
    }
}
