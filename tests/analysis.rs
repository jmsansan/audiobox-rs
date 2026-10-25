//! Analysis contracts kept through the #79 optimizations: every spectrogram column
//! equals `spectrum` of the samples from its start, with the input checked and the
//! workspace built once, and true peak returns the reference result bit for bit.
use audiobox::{Audio, ErrorCode, analyze};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;
thread_local! {
    // Only the measuring thread counts: the test harness allocates on its own threads.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}
fn count() {
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        ALLOCATIONS.with(|a| a.set(a.get() + 1));
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
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocations<T>(f: impl FnOnce() -> T) -> (T, usize) {
    ALLOCATIONS.with(|a| a.set(0));
    COUNTING.with(|c| c.set(true));
    let value = f();
    COUNTING.with(|c| c.set(false));
    (value, ALLOCATIONS.with(Cell::get))
}

fn signal(len: usize) -> Vec<f32> {
    let mut seed = 0x9e37_79b9u32;
    (0..len)
        .map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            (i as f32 * 0.031).sin() * 0.6 + noise * 0.2
        })
        .collect()
}

#[test]
fn columns_equal_the_spectrum_of_each_suffix() {
    let inputs = [
        Vec::new(),
        vec![0.0; 3000],
        signal(1),
        signal(37),
        signal(5000),
    ];
    for samples in &inputs {
        for fft in [2, 64, 1024] {
            for hop in [1, 3, 256, 1024, 6000] {
                if samples.len() / hop > 2000 {
                    continue;
                }
                let s = analyze::spectrogram(samples, 44_100, fft, hop).unwrap();
                let first = analyze::spectrum(&[], 44_100, fft).unwrap();
                assert_eq!(bits(&s.frequencies), bits(&first.frequencies));
                let starts: Vec<usize> = (0..samples.len()).step_by(hop).collect();
                assert_eq!(s.times.len(), starts.len());
                assert_eq!(s.magnitudes.len(), starts.len());
                for ((&start, &time), column) in starts.iter().zip(&s.times).zip(&s.magnitudes) {
                    assert_eq!(time.to_bits(), (start as f64 / 44_100.0).to_bits());
                    let reference = analyze::spectrum(&samples[start..], 44_100, fft).unwrap();
                    assert_eq!(
                        bits(column),
                        bits(&reference.magnitudes),
                        "{} samples, fft {fft}, hop {hop}, column at {start}",
                        samples.len()
                    );
                }
            }
        }
    }
}

fn bits(values: &[f64]) -> Vec<u64> {
    values.iter().map(|v| v.to_bits()).collect()
}

#[test]
fn errors_keep_their_order_and_reach_past_the_first_window() {
    let invalid = |r: audiobox::Result<analyze::Spectrogram>| r.unwrap_err().code;
    let mut samples = signal(10_000);
    // Hop first, then the FFT size, then the output limit, then the samples.
    samples[9_999] = f32::NAN;
    assert_eq!(
        invalid(analyze::spectrogram(&samples, 48_000, 7, 0)),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        invalid(analyze::spectrogram(&samples, 0, 64, 1)),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        invalid(analyze::spectrogram(&samples, 48_000, 3, 1)),
        ErrorCode::InvalidArgument
    );
    let long = vec![f32::NAN; 1 << 20];
    assert_eq!(
        invalid(analyze::spectrogram(&long, 48_000, 1 << 10, 1)),
        ErrorCode::LimitExceeded
    );
    // A non-finite sample far beyond the first window still fails, as with spectrum.
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        samples[9_999] = bad;
        assert_eq!(
            invalid(analyze::spectrogram(&samples, 48_000, 64, 4096)),
            ErrorCode::InvalidArgument
        );
        assert!(analyze::spectrum(&samples, 48_000, 64).is_err());
    }
    // Empty input has no columns and no samples to check.
    let empty = analyze::spectrogram(&[], 48_000, 64, 1).unwrap();
    assert!(empty.times.is_empty() && empty.magnitudes.is_empty());
    assert_eq!(empty.frequencies.len(), 33);
}

#[test]
fn only_the_output_grows_with_the_column_count() {
    let samples = signal(20_000);
    for hop in [100, 10] {
        let (s, count) = allocations(|| analyze::spectrogram(&samples, 48_000, 256, hop).unwrap());
        let columns = s.magnitudes.len();
        // One vector per column, plus the window, transform buffers, frequencies and
        // the column lists, made once. Building a workspace per column took four.
        assert!(
            count <= columns + 8,
            "{columns} columns made {count} allocations"
        );
    }
}

#[test]
fn many_columns_scale_with_the_input_not_its_square() {
    // Two-point transforms every sample over a million samples: a million columns.
    // Checking each column's suffix again visited about 5e11 samples; checking the
    // input once visits a million, so this finishes in moments.
    let samples = signal(1 << 20);
    let s = analyze::spectrogram(&samples, 48_000, 2, 1).unwrap();
    assert_eq!(s.magnitudes.len(), 1 << 20);
    let last = analyze::spectrum(&samples[(1 << 20) - 1..], 48_000, 2).unwrap();
    assert_eq!(bits(&s.magnitudes[(1 << 20) - 1]), bits(&last.magnitudes));
}

/// The true-peak scan as 0.4.0 wrote it: four-times oversampling with a 32-tap
/// Blackman-windowed sinc per phase, skipping taps outside the signal.
fn reference_true_peak(audio: &Audio) -> f64 {
    use std::f64::consts::PI;
    let mut peak = audio.peak();
    let mut kernel = [0.0; 128];
    for (i, k) in kernel.iter_mut().enumerate() {
        let t = i as f64 / 4.0 - 16.0;
        let sinc = if t == 0.0 {
            1.0
        } else {
            (PI * t).sin() / (PI * t)
        };
        let w = i as f64 / 127.0;
        *k = sinc * (0.42 - 0.5 * (2.0 * PI * w).cos() + 0.08 * (4.0 * PI * w).cos());
    }
    for c in audio.all_channels() {
        for i in 0..c.len() {
            for phase in 0..4 {
                let mut sum = 0.0;
                for k in 0..32 {
                    let at = i as i64 + k as i64 - 16;
                    if at >= 0 && (at as usize) < c.len() {
                        sum += c[at as usize] as f64 * kernel[k * 4 + phase];
                    }
                }
                peak = peak.max(sum.abs());
            }
        }
    }
    analyze::to_db(peak)
}

#[test]
fn true_peak_matches_the_reference_bit_for_bit() {
    for len in [0, 1, 15, 16, 17, 31, 32, 33, 48, 1000, 4801] {
        let left = signal(len);
        let right: Vec<f32> = left.iter().rev().map(|s| -s * 0.7).collect();
        for data in [vec![left.clone()], vec![left.clone(), right]] {
            let audio = Audio::new(data, 48_000).unwrap();
            assert_eq!(
                audio.true_peak_db().to_bits(),
                reference_true_peak(&audio).to_bits(),
                "{len} frames, {} channels",
                audio.channels()
            );
        }
    }
    // Inter-sample overs: alternating full-scale samples peak between them.
    let square: Vec<f32> = (0..256)
        .map(|i| if i % 4 < 2 { 1.0 } else { -1.0 })
        .collect();
    let audio = Audio::new(vec![square], 48_000).unwrap();
    assert_eq!(
        audio.true_peak_db().to_bits(),
        reference_true_peak(&audio).to_bits()
    );
    assert!(audio.true_peak_db() > 0.0);
}
