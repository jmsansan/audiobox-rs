// Adapted from TPT Cadence and the BSD-licensed Opus reference; see ../NOTICE.md.
//! LPC analysis/filtering for packet-loss concealment (`celt/celt_lpc.c`).
//!
//! SOURCE: Xiph.Org libopus 1.5.2, `celt/celt_lpc.c` (BSD-3-Clause).
//! Float build. `celt_iir` reproduces the reference's 4x-unrolled
//! structure exactly: the per-sample accumulation order (including the
//! patch terms) affects f32 rounding, so the "obvious" naive loop is NOT
//! bit-compatible.

// Lints: this file ports libopus verbatim - verbatim float literals and
// reference loop shapes trigger these lints (see tables.rs).
#![allow(
    clippy::excessive_precision,
    clippy::approx_constant,
    clippy::precedence,
    clippy::needless_range_loop,
    clippy::manual_memcpy,
    clippy::too_many_arguments
)]

/// `CELT_LPC_ORDER`.
pub(crate) const CELT_LPC_ORDER: usize = 24;

/// `_celt_lpc`: Levinson-Durbin from autocorrelation values.
pub(crate) fn _celt_lpc(lpc: &mut [f32], ac: &[f32], p: usize) {
    let mut error = ac[0];
    for v in lpc.iter_mut().take(p) {
        *v = 0.0;
    }
    if ac[0] > 1e-10 {
        for i in 0..p {
            // Sum up this iteration's reflection coefficient.
            let mut rr = 0f32;
            for j in 0..i {
                rr += lpc[j] * ac[i - j];
            }
            rr += ac[i + 1];
            let r = -(rr / error);
            // Update LPC coefficients and total error.
            lpc[i] = r;
            for j in 0..(i + 1) >> 1 {
                let tmp1 = lpc[j];
                let tmp2 = lpc[i - 1 - j];
                lpc[j] = tmp1 + r * tmp2;
                lpc[i - 1 - j] = tmp2 + r * tmp1;
            }
            error -= r * r * error;
            // Bail out once we get 30 dB gain.
            if error <= 0.001 * ac[0] {
                break;
            }
        }
    }
}

/// `xcorr_kernel_c` (float): `sum[j] += x[k] * y[j + k]` for j in 0..4.
/// The interleaved accumulation order per `sum[j]` is ascending in `k`,
/// identical to a naive loop, so a plain implementation is bit-equal.
fn xcorr_kernel(x: &[f32], y: &[f32], sum: &mut [f32; 4], len: usize) {
    for k in 0..len {
        let xv = x[k];
        sum[0] += xv * y[k];
        sum[1] += xv * y[k + 1];
        sum[2] += xv * y[k + 2];
        sum[3] += xv * y[k + 3];
    }
}

/// `celt_fir_c`: FIR filter. The source history lives at
/// `x[x_start - ord .. x_start]` (the reference relies on negative
/// indices into the caller's buffer); `x_start >= ord` is required.
pub(crate) fn celt_fir(
    x: &[f32],
    x_start: usize,
    num: &[f32],
    y: &mut [f32],
    n: usize,
    ord: usize,
) {
    let mut rnum = [0f32; CELT_LPC_ORDER];
    for i in 0..ord {
        rnum[i] = num[ord - i - 1];
    }
    for i in 0..n {
        let mut sum = x[x_start + i];
        for j in 0..ord {
            sum += rnum[j] * x[x_start + i + j - ord];
        }
        y[i] = sum;
    }
}

/// `celt_iir`: IIR filter (as a transposed FIR with negated history).
///
/// `x`/`y` may alias in the reference caller only through disjoint
/// buffers; here they are separate slices. `mem` holds `ord` past outputs.
pub(crate) fn celt_iir(
    x: &[f32],
    den: &[f32],
    y: &mut [f32],
    n: usize,
    ord: usize,
    mem: &mut [f32],
    scratch: &mut [f32],
) {
    debug_assert_eq!(ord & 3, 0);
    let mut rden = [0f32; CELT_LPC_ORDER];
    for i in 0..ord {
        rden[i] = den[ord - i - 1];
    }
    // y[0..ord) = -mem[ord-1-i]; y[ord..N+ord) = 0.
    for i in 0..ord {
        scratch[i] = -mem[ord - i - 1];
    }
    for v in scratch[ord..n + ord].iter_mut() {
        *v = 0.0;
    }
    let mut sum = [0f32; 4];
    let mut i = 0usize;
    while i + 3 < n {
        // Unroll by 4 as if it were an FIR filter.
        sum[0] = x[i];
        sum[1] = x[i + 1];
        sum[2] = x[i + 2];
        sum[3] = x[i + 3];
        xcorr_kernel(&rden[..ord], &scratch[i..], &mut sum, ord);
        // Patch up the result to compensate for the fact that this is an
        // IIR.
        scratch[i + ord] = -sum[0];
        y[i] = sum[0];
        sum[1] += scratch[i + ord] * den[0];
        scratch[i + ord + 1] = -sum[1];
        y[i + 1] = sum[1];
        sum[2] += scratch[i + ord + 1] * den[0];
        sum[2] += scratch[i + ord] * den[1];
        scratch[i + ord + 2] = -sum[2];
        y[i + 2] = sum[2];

        sum[3] += scratch[i + ord + 2] * den[0];
        sum[3] += scratch[i + ord + 1] * den[1];
        sum[3] += scratch[i + ord] * den[2];
        scratch[i + ord + 3] = -sum[3];
        y[i + 3] = sum[3];
        i += 4;
    }
    while i < n {
        let mut s = x[i];
        for j in 0..ord {
            s -= rden[j] * scratch[i + j];
        }
        scratch[i + ord] = s;
        y[i] = s;
        i += 1;
    }
    for j in 0..ord {
        mem[j] = y[n - 1 - j];
    }
}

/// `_celt_autocorr`: windowed autocorrelation.
pub(crate) fn _celt_autocorr(
    x: &[f32],
    ac: &mut [f32],
    window: Option<&[f32]>,
    overlap: usize,
    lag: usize,
    n: usize,
) -> i32 {
    let fast_n = n - lag;
    // Apply the window if needed. The largest caller uses n = MAX_PERIOD
    // (1024), so a fixed 1048-float stack buffer covers every case.
    let use_window = overlap != 0;
    let mut xx = [0f32; 1048];
    let xptr: &[f32] = if use_window {
        xx[..n].copy_from_slice(&x[..n]);
        let win = window.unwrap();
        for i in 0..overlap {
            xx[i] *= win[i];
            xx[n - i - 1] *= win[i];
        }
        &xx[..n]
    } else {
        x
    };

    celt_pitch_xcorr_f(xptr, xptr, ac, fast_n, lag + 1);
    for k in 0..=lag {
        let mut d = 0f32;
        for i in k + fast_n..n {
            d += xptr[i] * xptr[i - k];
        }
        ac[k] += d;
    }
    0
}

/// Local alias so `celt_lpc` does not depend on the pitch module.
fn celt_pitch_xcorr_f(x: &[f32], y: &[f32], xcorr: &mut [f32], len: usize, max_pitch: usize) {
    for i in 0..max_pitch {
        let mut sum = 0f32;
        for j in 0..len {
            sum += x[j] * y[i + j];
        }
        xcorr[i] = sum;
    }
}
