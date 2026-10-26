use audiobox::analyze::{self, MAX_MEASUREMENT_BOUNDARIES, MeasurementBoundary as Boundary};
use audiobox::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Count;
thread_local! {
    static ENABLED:Cell<bool>=const {Cell::new(false)};
    static ALLOCS:Cell<usize>=const {Cell::new(0)};
}
fn count() {
    if ENABLED.try_with(Cell::get).unwrap_or(false) {
        ALLOCS.with(|n| n.set(n.get() + 1));
    }
}
unsafe impl GlobalAlloc for Count {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static GLOBAL: Count = Count;
fn allocations<T>(f: impl FnOnce() -> T) -> (T, usize) {
    ALLOCS.with(|n| n.set(0));
    ENABLED.with(|n| n.set(true));
    let result = f();
    ENABLED.with(|n| n.set(false));
    (result, ALLOCS.with(Cell::get))
}
fn audio(channels: Vec<Vec<f32>>) -> Audio {
    Audio::new(channels, 48000).unwrap()
}
fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1e-12 * (1.0 + expected.abs()),
        "{actual} vs {expected}"
    );
}

#[test]
fn dc_means_preserve_channel_order_sign_headroom_and_cancellation() {
    let a = audio(vec![
        vec![1.5, -0.5, 1.5, -0.5],
        vec![-0.25; 4],
        vec![0.0; 4],
    ]);
    assert_eq!(
        a.dc_means().unwrap(),
        vec![Some(0.5), Some(-0.25), Some(0.0)]
    );
    assert_eq!(a.peak(), 1.5); // Existing peak exposes samples above full scale.
    assert_eq!(
        analyze::dc_mean(&[f32::MAX, 1.0, -f32::MAX]).unwrap(),
        Some(1.0 / 3.0)
    );
    assert_eq!(
        analyze::dc_mean(&[f32::from_bits(1); 3]).unwrap(),
        Some(f32::from_bits(1) as f64)
    );
    for frames in [0, 1, 16] {
        let a = audio(vec![vec![0.0; frames]; 3]);
        assert_eq!(
            a.dc_means().unwrap(),
            vec![if frames == 0 { None } else { Some(0.0) }; 3]
        );
    }
}

#[test]
fn silence_is_all_channel_frame_exact_and_threshold_equality_is_quiet() {
    let a = audio(vec![
        vec![0.0, 0.25, 0.5, 0.0, 0.25, 0.0, 0.0, 0.0],
        vec![0.0, 0.0, 0.0, 0.0, 0.0, -0.5, 0.25, 0.0],
    ]);
    let measured = a.threshold_silence(0.25).unwrap();
    assert_eq!(measured.frames, 8);
    assert_eq!(measured.leading_frames, 2);
    assert_eq!(measured.trailing_frames, 2);
    assert_eq!(measured.active_range, Some(2..6));
    let adjacent = a
        .threshold_silence(f64::from_bits(0.25f64.to_bits() - 1))
        .unwrap();
    assert_eq!(adjacent.active_range, Some(1..7));
    assert_eq!((adjacent.leading_frames, adjacent.trailing_frames), (1, 1));
    let quiet = a.threshold_silence(0.5).unwrap();
    assert_eq!(quiet.active_range, None);
    assert_eq!((quiet.leading_frames, quiet.trailing_frames), (8, 8));
    assert_eq!(a.threshold_silence(0.0).unwrap().active_range, Some(1..7));
    assert_eq!(a.threshold_silence(2.0).unwrap().active_range, None);
}

#[test]
fn empty_silent_and_short_stereo_have_explicit_no_result_states() {
    for frames in [0, 1, 8] {
        let a = audio(vec![vec![0.0; frames]; 2]);
        let silence = a.threshold_silence(0.0).unwrap();
        assert_eq!(silence.active_range, None);
        assert_eq!(
            (silence.leading_frames, silence.trailing_frames),
            (frames, frames)
        );
        let stereo = a.stereo_measurements().unwrap();
        assert_eq!(stereo.correlation, None);
        assert_eq!(
            stereo.mid_mean_square,
            if frames == 0 { None } else { Some(0.0) }
        );
        assert_eq!(stereo.side_mean_square, stereo.mid_mean_square);
        let boundaries = a.measure_boundaries(&[Boundary::LoopSeam]).unwrap();
        if frames < 2 {
            assert_eq!(boundaries[0].samples, None)
        } else {
            assert_eq!(boundaries[0].samples.as_ref().unwrap().max_abs_step, 0.0);
        }
        assert!(a.measure_boundaries(&[]).unwrap().is_empty());
    }
    let s = analyze::stereo_measurements(&[1.0], &[0.5]).unwrap();
    assert_eq!(s.correlation, None);
    assert_eq!(s.left_mean_square, Some(1.0));
    assert_eq!(s.right_mean_square, Some(0.25));
    assert_eq!(s.mid_mean_square, Some(0.5625));
    assert_eq!(s.side_mean_square, Some(0.0625));
}

#[test]
fn identical_inverted_and_orthogonal_stereo_obey_mid_side_scaling() {
    let l = [-1.0, 1.0, -1.0, 1.0];
    let identical = analyze::stereo_measurements(&l, &l).unwrap();
    close(identical.correlation.unwrap(), 1.0);
    assert_eq!(identical.mid_mean_square, Some(1.0));
    assert_eq!(identical.side_mean_square, Some(0.0));
    let r = l.map(|x| -x);
    let inverted = analyze::stereo_measurements(&l, &r).unwrap();
    close(inverted.correlation.unwrap(), -1.0);
    assert_eq!(inverted.mid_mean_square, Some(0.0));
    assert_eq!(inverted.side_mean_square, Some(1.0));
    let orthogonal = analyze::stereo_measurements(&l, &[-1.0, -1.0, 1.0, 1.0]).unwrap();
    assert_eq!(orthogonal.correlation, Some(0.0));
    assert_eq!(orthogonal.mid_mean_square, Some(0.5));
    assert_eq!(orthogonal.side_mean_square, Some(0.5));
    for s in [identical, inverted, orthogonal] {
        close(
            s.left_mean_square.unwrap() + s.right_mean_square.unwrap(),
            2.0 * (s.mid_mean_square.unwrap() + s.side_mean_square.unwrap()),
        );
    }
}

#[test]
fn pearson_removes_dc_only_for_correlation_and_handles_constant_channels() {
    let l = [0.125, -0.125, 0.125, -0.125].map(|offset| 1_000_000.0 + offset);
    let r = [99.75, 100.25, 99.75, 100.25];
    let s = analyze::stereo_measurements(&l, &r).unwrap();
    close(s.correlation.unwrap(), -1.0);
    assert!(s.left_mean_square.unwrap() > 1e11);
    let a = vec![0.1; 10001];
    let b: Vec<_> = (0..10001).map(|i| (i % 7) as f32).collect();
    let s = analyze::stereo_measurements(&a, &b).unwrap();
    assert_eq!(s.correlation, None);
    close(s.left_mean_square.unwrap(), (0.1f32 as f64).powi(2));
}

#[test]
fn finite_extremes_and_subnormals_keep_finite_powers_and_correlation() {
    for v in [f32::MAX, f32::MIN_POSITIVE, f32::from_bits(1)] {
        let l = [v, -v, v, -v];
        let s = analyze::stereo_measurements(&l, &l).unwrap();
        close(s.correlation.unwrap(), 1.0);
        for power in [s.left_mean_square, s.right_mean_square, s.mid_mean_square] {
            let power = power.unwrap();
            assert!(power.is_finite() && power > 0.0);
            assert!((power / ((v as f64) * (v as f64)) - 1.0).abs() < 1e-12);
        }
        assert_eq!(s.side_mean_square, Some(0.0));
    }
}

#[test]
fn boundaries_preserve_explicit_order_frames_and_signed_per_channel_steps() {
    let a = audio(vec![
        vec![0.25, 0.25, -0.5, -0.5],
        vec![-0.125, -0.125, 0.125, 0.125],
    ]);
    let requests = [
        Boundary::Frame(2),
        Boundary::LoopSeam,
        Boundary::Frame(2),
        Boundary::Frame(1),
    ];
    let steps = a.measure_boundaries(&requests).unwrap();
    assert_eq!(steps.len(), 4);
    assert_eq!(
        steps.iter().map(|s| s.boundary).collect::<Vec<_>>(),
        requests
    );
    assert_eq!(steps[0], steps[2]);
    let join = steps[0].samples.as_ref().unwrap();
    assert_eq!((join.before_frame, join.after_frame), (1, 2));
    assert_eq!(
        (
            join.channels[0].before,
            join.channels[0].after,
            join.channels[0].difference
        ),
        (0.25, -0.5, -0.75)
    );
    assert_eq!(join.channels[1].difference, 0.25);
    assert_eq!(join.max_abs_step, 0.75);
    let seam = steps[1].samples.as_ref().unwrap();
    assert_eq!((seam.before_frame, seam.after_frame), (3, 0));
    assert_eq!(seam.channels[0].difference, 0.75);
    assert_eq!(seam.channels[1].difference, -0.25);
    assert_eq!(steps[3].samples.as_ref().unwrap().max_abs_step, 0.0);
}

#[test]
fn intentional_drum_attack_is_a_numeric_step_without_a_defect_classification() {
    let a = audio(vec![vec![0.0, 0.0, 0.75, -0.25, 0.0]]);
    let measured = a.measure_boundaries(&[Boundary::Frame(2)]).unwrap();
    assert_eq!(measured[0].samples.as_ref().unwrap().max_abs_step, 0.75);
    assert_eq!(a.peak(), 0.75);
}

#[test]
fn invalid_thresholds_samples_layouts_and_boundaries_have_stable_errors() {
    let a = audio(vec![vec![0.0; 4]; 2]);
    for t in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            a.threshold_silence(t).unwrap_err().code,
            ErrorCode::InvalidArgument
        );
    }
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            analyze::dc_mean(&[0.0, value]).unwrap_err().code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            analyze::stereo_measurements(&[0.0, value], &[0.0, 0.0])
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            analyze::stereo_measurements(&[0.0, 0.0], &[value, 0.0])
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
    }
    assert_eq!(
        analyze::stereo_measurements(&[0.0], &[]).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    for channels in [1, 3, 4, 6, 8] {
        assert_eq!(
            audio(vec![vec![0.0; 4]; channels])
                .stereo_measurements()
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
    }
    for at in [0, 4, usize::MAX] {
        assert_eq!(
            a.measure_boundaries(&[Boundary::Frame(at)])
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
    }
    assert_eq!(
        a.measure_boundaries(&vec![Boundary::LoopSeam; MAX_MEASUREMENT_BOUNDARIES + 1])
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
}

#[test]
fn measurement_output_has_bounded_allocations_independent_of_pcm_length() {
    for frames in [16, 65536] {
        let a = audio(vec![vec![0.125; frames]; 2]);
        let ((dc, silence, stereo), allocs) = allocations(|| {
            (
                analyze::dc_mean(a.channel_data(0).unwrap()).unwrap(),
                a.threshold_silence(0.0).unwrap(),
                a.stereo_measurements().unwrap(),
            )
        });
        assert_eq!(allocs, 0);
        assert_eq!(dc, Some(0.125));
        assert_eq!(silence.leading_frames, 0);
        assert_eq!(stereo.correlation, None);
        let (means, allocs) = allocations(|| a.dc_means().unwrap());
        assert_eq!(allocs, 1);
        assert_eq!(means.len(), 2);
        let (steps, allocs) = allocations(|| {
            a.measure_boundaries(&[Boundary::Frame(1), Boundary::LoopSeam])
                .unwrap()
        });
        assert_eq!(allocs, 3);
        assert_eq!(steps.len(), 2);
    }
    let a = audio(vec![vec![0.0; 2]; 64]);
    let steps = a
        .measure_boundaries(&vec![Boundary::LoopSeam; MAX_MEASUREMENT_BOUNDARIES])
        .unwrap();
    assert_eq!(steps.len(), MAX_MEASUREMENT_BOUNDARIES);
    assert!(
        steps
            .iter()
            .all(|s| s.samples.as_ref().unwrap().channels.len() == 64)
    );
}

#[test]
fn decoded_custom_limits_cannot_bypass_measurement_shape_budgets() {
    let mut bytes = encode(&audio(vec![vec![]]), AudioFormat::Wav).unwrap();
    assert_eq!(&bytes[12..16], b"fmt ");
    bytes[22..24].copy_from_slice(&65u16.to_le_bytes());
    bytes[28..32].copy_from_slice(&(48000u32 * 130).to_le_bytes());
    bytes[32..34].copy_from_slice(&130u16.to_le_bytes());
    let a = decode_with_options(
        &bytes,
        &DecodeOptions {
            limits: Limits {
                max_channels: 65,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(a.channels(), 65);
    assert_eq!(a.dc_means().unwrap_err().code, ErrorCode::LimitExceeded);
    assert_eq!(
        a.threshold_silence(0.0).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        a.measure_boundaries(&[Boundary::LoopSeam])
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
}

#[test]
fn integer_reference_and_repeatability_cover_report_accuracy_without_a_byte_promise() {
    let mut state = 7u32;
    let mut xy = Vec::new();
    for _ in 0..8192 {
        let mut pair = [0i64; 2];
        for x in &mut pair {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *x = ((state >> 16) & 1023) as i64 - 512;
        }
        xy.push(pair);
    }
    let a = audio(
        (0..2)
            .map(|c| xy.iter().map(|p| p[c] as f32 / 1024.0).collect())
            .collect(),
    );
    let original = a.all_channels().to_vec();
    let [sx, sy, xx, yy, cross] = xy.iter().fold([0i128; 5], |mut sums, p| {
        let x = p[0] as i128;
        let y = p[1] as i128;
        sums[0] += x;
        sums[1] += y;
        sums[2] += x * x;
        sums[3] += y * y;
        sums[4] += x * y;
        sums
    });
    let n = xy.len() as i128;
    let correlation = (n * cross - sx * sy) as f64
        / (((n * xx - sx * sx) as f64).sqrt() * ((n * yy - sy * sy) as f64).sqrt());
    let denominator = n as f64 * 1024.0 * 1024.0;
    let expected = a.stereo_measurements().unwrap();
    close(expected.correlation.unwrap(), correlation);
    close(expected.left_mean_square.unwrap(), xx as f64 / denominator);
    close(expected.right_mean_square.unwrap(), yy as f64 / denominator);
    close(
        expected.mid_mean_square.unwrap(),
        (xx + 2 * cross + yy) as f64 / (4.0 * denominator),
    );
    close(
        expected.side_mean_square.unwrap(),
        (xx - 2 * cross + yy) as f64 / (4.0 * denominator),
    );
    close(a.mean_square(), (xx + yy) as f64 / (2.0 * denominator));
    let means = a.dc_means().unwrap();
    close(means[0].unwrap(), sx as f64 / (n as f64 * 1024.0));
    close(means[1].unwrap(), sy as f64 / (n as f64 * 1024.0));
    for _ in 0..3 {
        assert_eq!(a.stereo_measurements().unwrap(), expected);
        assert_eq!(a.dc_means().unwrap(), means);
    }
    assert_eq!(a.all_channels(), original);
}
