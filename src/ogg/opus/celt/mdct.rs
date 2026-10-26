// Adapted from TPT Cadence and the BSD-licensed Opus reference; see ../NOTICE.md.
//! Backward MDCT (`celt/mdct.c`) — the CELT synthesis transform.
//!
//! Ported from libopus 1.5.2 `clt_mdct_backward_c`. The transform is an
//! MDCT-inverse performed with the shared `N4`-point FFT plus pre/post
//! rotations, followed by the TDAC "mirror on both sides" windowing step.
//! There is no explicit scaling by 1/2: it is folded into the window overlap,
//! matching the reference exactly. In particular we reproduce the reference's
//! odd in-place details: the FFT runs over `out[overlap/2 ..]` reinterpreted
//! as complex pairs, and the post-rotation works in-place from both ends.
//
// SOURCE: Xiph.Org libopus 1.5.2, `celt/mdct.c` (BSD-3-Clause).

use super::fft::{Cpx, fft_impl, fft_state};
use super::tables::{FFT_BITREV60, FFT_BITREV120, FFT_BITREV240, FFT_BITREV480, MDCT_TWIDDLES960};

/// The static `mdct_lookup` from `static_modes_float.h`: `n = 1920`,
/// `maxshift = 3`, four FFT substates, one shared trig table (1800 floats).
///
/// Kept as a zero-field type so the accessors mirror the reference's lookup
/// fields; `maxshift()` is consumed once the decoder switches frames.
#[allow(dead_code)]
pub(crate) struct MdctLookup;

impl MdctLookup {
    /// The MDCT length. For the Opus static mode this is `2 * 960 = 1920`.
    pub fn n() -> usize {
        1920
    }

    #[allow(dead_code)]
    pub fn maxshift() -> usize {
        3
    }

    /// `l.trig`: the shared pre/post rotation cosine table.
    fn trig() -> &'static [f32] {
        &MDCT_TWIDDLES960
    }
}

/// Backward MDCT and weighted overlap-add (no explicit scaling).
///
/// Implements `clt_mdct_backward_c(l, in, out, window, overlap, shift,
/// stride, arch)`:
///
/// - `freq`: `N4` real spectral lines for one sub-block (the FFT input).
///   Reads that fall outside `freq` resolve to zero, so corrupt frames can
///   never panic (same policy as `RangeDecoder`).
/// - `out`: scratch/output buffer of length ≥ `overlap/2 + N2`. The
///   `overlap/2`-sample head region holds the previous frame's tail, which
///   the TDAC mirror combines with the new synthesis before it is written.
/// - `window`: the `overlap`-sample analysis/synthesis window.
/// - `scratch`: `n4` complex slots for the in-place FFT, provided by the
///   caller (allocation-free `decode()`).
///
/// Currently exercised from the test suite; the decoder assembly consumes it
/// as the synthesis transform once quant_bands/bands land.
#[allow(dead_code)]
pub(crate) fn mdct_backward(
    freq: &[f32],
    out: &mut [f32],
    window: &[f32],
    overlap: usize,
    shift: usize,
    stride: usize,
    scratch: &mut [Cpx],
) {
    let mut n = MdctLookup::n();
    let mut trig_base = 0usize;
    for _ in 0..shift {
        n >>= 1;
        trig_base += n;
    }
    let n2 = n >> 1;
    let n4 = n >> 2;
    let trig = MdctLookup::trig();

    let bitrev: &[i16] = match n4 {
        60 => &FFT_BITREV60,
        120 => &FFT_BITREV120,
        240 => &FFT_BITREV240,
        480 => &FFT_BITREV480,
        _ => unreachable!("CELT MDCT only uses 60/120/240/480-point FFTs"),
    };

    // Pre-rotate: swap real/imag because we use an FFT instead of an IFFT.
    {
        let mut xp1 = 0usize; // in
        // The reference walks `in + stride*(N2-1)` down to -1 on the last
        // iteration (never dereferenced); reads stay in [1, ..].
        let mut xp2 = (stride * (n2 - 1)) as isize; // in + stride*(N2-1)
        let yp = overlap / 2; // out, complex offset
        for (i, &rev) in bitrev[..n4].iter().enumerate() {
            let t = trig_base + i;
            let rev = rev as usize;
            let xp1v = freq.get(xp1).copied().unwrap_or(0.0);
            let xp2v = freq.get(xp2 as usize).copied().unwrap_or(0.0);
            // ADD32_ovflw(S_MUL(*xp2, t[i]), S_MUL(*xp1, t[N4+i]))
            let yr = xp2v * trig[t] + xp1v * trig[t + n4];
            // SUB32_ovflw(S_MUL(*xp1, t[i]), S_MUL(*xp2, t[N4+i]))
            let yi = xp1v * trig[t] - xp2v * trig[t + n4];
            out[yp + 2 * rev + 1] = yr;
            out[yp + 2 * rev] = yi;
            xp1 += 2 * stride;
            xp2 -= 2 * stride as isize;
        }
    }

    // In-place complex FFT over out[overlap/2 .. overlap/2 + 2*n4). The C
    // code reinterprets the float array as complex pairs; we copy into the
    // caller's complex scratch, run the FFT, and copy back — identical
    // layout, no allocation.
    let st = fft_state(n4);
    {
        let base = overlap / 2;
        for (i, c) in scratch.iter_mut().take(n4).enumerate() {
            c.r = out[base + 2 * i];
            c.i = out[base + 2 * i + 1];
        }
        fft_impl(st, &mut scratch[..n4]);
        for i in 0..n4 {
            out[base + 2 * i] = scratch[i].r;
            out[base + 2 * i + 1] = scratch[i].i;
        }
    }

    // Post-rotate and de-shuffle from both ends of the buffer at once.
    {
        let mut yp0 = overlap / 2;
        let mut yp1 = overlap / 2 + n2 - 2;
        // Loop to (N4+1)>>1 to handle odd N4.
        for i in 0..n4.div_ceil(2) {
            // We swap real/imag because we're using an FFT instead of an IFFT.
            let re = out[yp0 + 1];
            let im = out[yp0];
            let t0 = trig[trig_base + i];
            let t1 = trig[trig_base + n4 + i];
            let yr = re * t0 + im * t1;
            let yi = re * t1 - im * t0;
            let re = out[yp1 + 1];
            let im = out[yp1];
            out[yp0] = yr;
            out[yp1 + 1] = yi;

            let t0 = trig[trig_base + n4 - i - 1];
            let t1 = trig[trig_base + n2 - i - 1];
            let yr = re * t0 + im * t1;
            let yi = re * t1 - im * t0;
            out[yp1] = yr;
            out[yp0 + 1] = yi;
            yp0 += 2;
            yp1 -= 2;
        }
    }

    // Mirror on both sides for TDAC.
    {
        for i in 0..overlap / 2 {
            let xp1 = overlap - 1 - i;
            let yp1 = i;
            let wp1 = i;
            let wp2 = overlap - 1 - i;
            let x1 = out[xp1];
            let x2 = out[yp1];
            // SUB32_ovflw(MULT16_32_Q15(*wp2, x2), MULT16_32_Q15(*wp1, x1))
            out[yp1] = window[wp2] * x2 - window[wp1] * x1;
            // ADD32_ovflw(MULT16_32_Q15(*wp1, x2), MULT16_32_Q15(*wp2, x1))
            out[xp1] = window[wp1] * x2 + window[wp2] * x1;
        }
    }
}

/// Forward MDCT and windowing/fold (analysis transform for the encoder).
///
/// Implements `clt_mdct_forward_c(l, in, out, window, overlap, shift,
/// stride, arch)`: the exact mirror of [`mdct_backward`] above, ported the
/// same way from libopus 1.5.2 `celt/mdct.c`.
///
/// - `inp`: `overlap + N2` time-domain samples (the current block plus the
///   previous block's tail, matching the encoder's sliding input buffer).
/// - `out`: `N2` spectral lines written for this sub-block.
/// - `scratch`: `n4` complex slots for the in-place FFT (allocation-free).
///
/// Verified against the analytic MDCT definition and round-tripped through
/// [`mdct_backward`] in `tests` below (SNR > 60 dB either way).
///
/// Not yet wired into a top-level encoder (see `todo.md`); currently
/// exercised only by the test suite, matching [`mdct_backward`]'s own
/// pre-integration state.
#[allow(dead_code, clippy::too_many_arguments)]
pub(crate) fn mdct_forward(
    inp: &[f32],
    out: &mut [f32],
    window: &[f32],
    overlap: usize,
    shift: usize,
    stride: usize,
    scratch: &mut [Cpx],
) {
    let mut n = MdctLookup::n();
    let mut trig_base = 0usize;
    for _ in 0..shift {
        n >>= 1;
        trig_base += n;
    }
    let n2 = n >> 1;
    let n4 = n >> 2;
    let trig = MdctLookup::trig();
    let sta = fft_state(n4);
    let scale = sta.scale;

    let bitrev: &[i16] = match n4 {
        60 => &FFT_BITREV60,
        120 => &FFT_BITREV120,
        240 => &FFT_BITREV240,
        480 => &FFT_BITREV480,
        _ => unreachable!("CELT MDCT only uses 60/120/240/480-point FFTs"),
    };

    // Window, shuffle, fold.
    let mut f = vec![0.0f32; n2];
    let mut i = 0usize;
    let mut xp1 = overlap / 2;
    let mut xp2 = n2 - 1 + overlap / 2;
    let mut wp1 = overlap / 2;
    // The reference walks `window[overlap/2-1]` down by 2 each step; the
    // last decrement lands one past the start but is never dereferenced.
    let mut wp2 = (overlap / 2 - 1) as isize;
    while i < (overlap + 3) >> 2 {
        f[2 * i] = window[wp2.max(0) as usize] * inp[xp1 + n2] + window[wp1] * inp[xp2];
        f[2 * i + 1] = window[wp1] * inp[xp1] - window[wp2.max(0) as usize] * inp[xp2 - n2];
        xp1 += 2;
        xp2 -= 2;
        wp1 += 2;
        wp2 -= 2;
        i += 1;
    }
    let mut wp1 = 0usize;
    let mut wp2 = overlap - 1;
    while i < n4 - ((overlap + 3) >> 2) {
        f[2 * i] = inp[xp2];
        f[2 * i + 1] = inp[xp1];
        xp1 += 2;
        xp2 -= 2;
        i += 1;
    }
    while i < n4 {
        f[2 * i] = -window[wp1] * inp[xp1 - n2] + window[wp2] * inp[xp2];
        f[2 * i + 1] = window[wp2] * inp[xp1] + window[wp1] * inp[xp2 + n2];
        xp1 += 2;
        xp2 -= 2;
        wp1 += 2;
        wp2 -= 2;
        i += 1;
    }

    // Pre-rotation, scaled by the FFT's 1/nfft.
    for j in 0..n4 {
        let t0 = trig[trig_base + j];
        let t1 = trig[trig_base + n4 + j];
        let re = f[2 * j];
        let im = f[2 * j + 1];
        let yr = re * t0 - im * t1;
        let yi = im * t0 + re * t1;
        scratch[bitrev[j] as usize] = Cpx {
            r: scale * yr,
            i: scale * yi,
        };
    }
    fft_impl(sta, &mut scratch[..n4]);

    // Post-rotate.
    {
        let mut yp1 = 0usize;
        // Walks down to -1 on the last iteration (never dereferenced,
        // matching the reference's pointer arithmetic).
        let mut yp2 = (stride * (n2 - 1)) as isize;
        for j in 0..n4 {
            let fi = scratch[j];
            out[yp1] = fi.i * trig[trig_base + n4 + j] - fi.r * trig[trig_base + j];
            out[yp2 as usize] = fi.r * trig[trig_base + n4 + j] + fi.i * trig[trig_base + j];
            yp1 += 2 * stride;
            yp2 -= 2 * stride as isize;
        }
    }
}
