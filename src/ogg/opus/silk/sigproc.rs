// Adapted from TPT Cadence and the BSD-licensed Opus reference; see ../NOTICE.md.
//! SILK fixed-point arithmetic helpers.
//!
//! One-to-one ports of the generic (non-ARM/MIPS inline) definitions in
//! libopus 1.5.2 `silk/SigProc_FIX.h`, `silk/macros.h` (the
//! `OPUS_FAST_INT64` variants where conditional), `silk/Inlines.h`, and
//! `silk/sort.c`, preserving the reference's exact truncation and wrapping
//! semantics: 16-bit operands are truncated via `as i16` (the `SMUL*B`
//! "bottom-half" convention), 64-bit products are truncated back to `i32`,
//! additions wrap, and right shifts are arithmetic (floor).
//!
//! SOURCE: Xiph.Org libopus 1.5.2, `silk/SigProc_FIX.h`, `silk/macros.h`,
//! `silk/Inlines.h`, `silk/sort.c` (BSD-3-Clause).
#![allow(dead_code)]

/// `silk_SMULBB(a, b)`: `(opus_int16)a * (opus_int16)b` — the product of
/// the bottom 16-bit halves (result always fits in `i32`).
#[inline]
pub(crate) fn smulbb(a: i32, b: i32) -> i32 {
    (a as i16) as i32 * (b as i16) as i32
}

/// `silk_SMULWB(a, b)`: `(a * (opus_int16)b) >> 16`, computed in 64-bit
/// like the reference macro and truncated to `i32`.
#[inline]
pub(crate) fn smulwb(a: i32, b: i32) -> i32 {
    (((a as i64) * ((b as i16) as i64)) >> 16) as i32
}

/// `silk_SMLAWB(a, b, c)`: `a + ((b * (opus_int16)c) >> 16)` — accumulate
/// a Q16-shifted product (wrapping, like the reference's `i32` adds).
#[inline]
pub(crate) fn smlawb(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_add(smulwb(b, c))
}

/// `silk_SMLAWT(a, b, c)`: `a + ((b * (c >> 16)) >> 16)` — accumulate a
/// Q16-shifted product using the **top** 16 bits of `c` (the full 32-bit
/// arithmetic shift, not an `i16` truncation). The packed LF-shaping
/// coefficient relies on this: `silk_SMULWB` picks the low half
/// (`LF_MA_shp`) while `silk_SMLAWT` picks the high half (`LF_AR_shp`).
#[inline]
pub(crate) fn smlawt(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_add((((b as i64) * ((c as i64) >> 16)) >> 16) as i32)
}

/// `silk_SMLABB(a, b, c)`: `a + (opus_int16)b * (opus_int16)c`.
#[inline]
pub(crate) fn smlabb(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_add(smulbb(b, c))
}

/// `silk_RSHIFT_ROUND(a, shift)`: right shift with rounding to nearest
/// (halves away from... the reference's exact form: `((a >> (shift-1)) + 1)
/// >> 1`, with the `shift == 1` fast path `(a >> 1) + (a & 1)`).
#[inline]
pub(crate) fn rshift_round(a: i32, shift: u32) -> i32 {
    debug_assert!((1..32).contains(&shift));
    if shift == 1 {
        (a >> 1) + (a & 1)
    } else {
        ((a >> (shift - 1)).wrapping_add(1)) >> 1
    }
}

/// `silk_SAT16(a)`: clamp to the `i16` range.
#[inline]
pub(crate) fn sat16(a: i32) -> i16 {
    a.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// `silk_MUL(a, b)`: wrapping 32-bit product.
#[inline]
pub(crate) fn mul(a: i32, b: i32) -> i32 {
    a.wrapping_mul(b)
}

/// `silk_SMULL(a, b)`: 64-bit product of two 32-bit operands.
#[inline]
pub(crate) fn smull(a: i32, b: i32) -> i64 {
    (a as i64) * (b as i64)
}

/// `silk_SMULWW(a, b)`: `(a * b) >> 16` over the full 64-bit product
/// (`OPUS_FAST_INT64` variant — no `i16` truncation of `b`, unlike
/// `silk_SMULWB`), truncated back to `i32`.
#[inline]
pub(crate) fn smulww(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> 16) as i32
}

/// `silk_SMLAWW(a, b, c)`: `a + ((b * c) >> 16)` over the full 64-bit
/// product, truncated back to `i32`.
#[inline]
pub(crate) fn smlaww(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_add((((b as i64) * (c as i64)) >> 16) as i32)
}

/// `silk_SMMUL(a, b)`: `(opus_int32)(silk_SMULL(a, b) >> 32)` — the high
/// word of the 64-bit product (arithmetic shift, no rounding).
#[inline]
pub(crate) fn smmul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> 32) as i32
}

/// `silk_RSHIFT_ROUND64(a, shift)`: 64-bit right shift with rounding to
/// nearest, same algebraic form as the 32-bit version.
#[inline]
pub(crate) fn rshift_round64(a: i64, shift: u32) -> i64 {
    debug_assert!((1..64).contains(&shift));
    if shift == 1 {
        (a >> 1) + (a & 1)
    } else {
        (a >> (shift - 1)).wrapping_add(1) >> 1
    }
}

/// `silk_ADD_LSHIFT32(a, b, shift)`: `a + (b << shift)` (wrapping).
#[inline]
pub(crate) fn add_lshift32(a: i32, b: i32, shift: u32) -> i32 {
    a.wrapping_add(b.wrapping_shl(shift))
}

/// `silk_DIV32_16(a, b)`: C integer division of `a` by the `i16`-truncated
/// `b` (truncates toward zero, like Rust's `/`).
#[inline]
pub(crate) fn div32_16(a: i32, b: i32) -> i32 {
    a / (b as i16 as i32)
}

/// `silk_DIV32(a, b)`: C integer division (truncates toward zero).
#[inline]
pub(crate) fn div32(a: i32, b: i32) -> i32 {
    a / b
}

/// `silk_ADD_SAT16(a, b)`: add and saturate the result to the `i16` range.
#[inline]
pub(crate) fn add_sat16(a: i32, b: i32) -> i16 {
    sat16(a.wrapping_add(b))
}

/// `silk_SUB_SAT32(a, b)`: subtract and saturate the result to the `i32`
/// range (the generic `silk_SUB_SAT32` macro in `silk/macros.h`).
#[inline]
pub(crate) fn sub_sat32(a: i32, b: i32) -> i32 {
    a.saturating_sub(b)
}

/// `silk_ADD_SAT32(a, b)`: add and saturate the result to the `i32`
/// range (the generic `silk_ADD_SAT32` macro in `silk/macros.h`).
#[inline]
pub(crate) fn add_sat32(a: i32, b: i32) -> i32 {
    a.saturating_add(b)
}

/// `RAND_MULTIPLIER` (`silk/SigProc_FIX.h`): LCG multiplier used by the
/// excitation dither (`decode_core.c`), the PLC noise fill (`PLC.c`) and
/// CNG (`CNG.c`).
pub(crate) const RAND_MULTIPLIER: i32 = 196314165;
/// `RAND_INCREMENT` (`silk/SigProc_FIX.h`).
pub(crate) const RAND_INCREMENT: i32 = 907633515;

/// `silk_RAND(seed)` (`silk/SigProc_FIX.h`):
/// `silk_MLA_ovflw(RAND_INCREMENT, seed, RAND_MULTIPLIER)` — the multiply
/// and the add both wrap modulo 2^32.
#[inline]
pub(crate) fn rand(seed: i32) -> i32 {
    seed.wrapping_mul(RAND_MULTIPLIER)
        .wrapping_add(RAND_INCREMENT)
}

/// `silk_SMULTT(a, b)`: product of the *top* 16-bit halves.
#[inline]
pub(crate) fn smultt(a: i32, b: i32) -> i32 {
    (a >> 16).wrapping_mul(b >> 16)
}

/// `silk_SUB_LSHIFT32(a, b, shift)`: `a - (b << shift)` (wrapping).
#[inline]
pub(crate) fn sub_lshift32(a: i32, b: i32, shift: u32) -> i32 {
    a.wrapping_sub(b.wrapping_shl(shift))
}

/// `silk_LSHIFT_SAT32(a, shift)`: left shift with the input clamped to
/// the representable output range first (so the shift cannot overflow).
#[inline]
pub(crate) fn lshift_sat32(a: i32, shift: u32) -> i32 {
    debug_assert!((1..32).contains(&shift));
    a.clamp(i32::MIN >> shift, i32::MAX >> shift) << shift
}

/// `silk_CLZ_FRAC` (`silk/Inlines.h`): leading-zero count plus the 7
/// bits right after the leading one. The C form shifts via
/// `silk_ROR32(in, 24 - lz)`, which is undefined for `lz > 24`
/// (`x < 256`); released x86 builds execute the shift-count masking as a
/// rotate by `rot & 31`, which is what [`u32::rotate_right`] expresses
/// for `rot` taken modulo 32. Matches x86 for the whole domain and the
/// exact definition wherever the rotate amount is in range.
#[inline]
pub(crate) fn clz_frac(x: i32) -> (u32, i32) {
    let lz = x.leading_zeros();
    let rot = (24i32 - lz as i32).rem_euclid(32);
    (lz, x.rotate_right(rot as u32) & 0x7f)
}

/// `silk_SQRT_APPROX` (`silk/Inlines.h`): square-root approximation with
/// < ±10% error for outputs > 15.
#[inline]
pub(crate) fn sqrt_approx(x: i32) -> i32 {
    if x <= 0 {
        return 0;
    }
    let (lz, frac_q7) = clz_frac(x);
    let mut y: i32 = if lz & 1 != 0 { 32768 } else { 46214 }; // 46214 = sqrt(2) * 32768
    // get scaling right
    y >>= lz >> 1;
    // increment using fractional part of input
    y.wrapping_add(smulwb(y, smulbb(213, frac_q7)))
}

/// `silk_sum_sqr_shift` (`silk/sum_sqr_shift.c`): energy of an `i16`
/// vector and the right shift applied to fit it in an `i32`. Both passes
/// accumulate in wrapping `u32` exactly like the reference's
/// `silk_ADD_RSHIFT_uint` / `silk_SMLABB_ovflw` forms.
pub(crate) fn sum_sqr_shift(x: &[i16]) -> (i32, u32) {
    let len = x.len();
    debug_assert!(len > 0 && len <= i32::MAX as usize);
    // Do a first run with the maximum shift we could have.
    let mut shft: u32 = 31 - (len as i32).leading_zeros();
    // Let's be conservative with rounding and start with nrg=len.
    let mut nrg: i32 = len as i32;
    let mut i = 0;
    while i + 1 < len {
        let nrg_tmp = (smulbb(x[i] as i32, x[i] as i32) as u32)
            .wrapping_add(smulbb(x[i + 1] as i32, x[i + 1] as i32) as u32);
        nrg = ((nrg as u32).wrapping_add(nrg_tmp) >> shft) as i32;
        i += 2;
    }
    if i < len {
        let nrg_tmp = smulbb(x[i] as i32, x[i] as i32) as u32;
        nrg = ((nrg as u32).wrapping_add(nrg_tmp) >> shft) as i32;
    }
    // Make sure the result will fit in a 32-bit signed integer with two
    // bits of headroom.
    shft = ((shft as i32) + 3 - nrg.leading_zeros() as i32).max(0) as u32;
    nrg = 0;
    let mut i = 0;
    while i + 1 < len {
        let nrg_tmp = (smulbb(x[i] as i32, x[i] as i32) as u32)
            .wrapping_add(smulbb(x[i + 1] as i32, x[i + 1] as i32) as u32);
        nrg = ((nrg as u32).wrapping_add(nrg_tmp) >> shft) as i32;
        i += 2;
    }
    if i < len {
        let nrg_tmp = smulbb(x[i] as i32, x[i] as i32) as u32;
        nrg = ((nrg as u32).wrapping_add(nrg_tmp) >> shft) as i32;
    }
    (nrg, shft)
}

/// `silk_LPC_analysis_filter` (`silk/LPC_analysis_filter.c`): the
/// generic (non-`USE_CELT_FIR`) fixed-point form. `out` and `input` are
/// the same length; the first `d` output samples are zeroed and the MA
/// prediction accumulates with wrapping adds so that two wraps can
/// cancel (the reference relies on this for invalid streams).
pub(crate) fn lpc_analysis_filter(out: &mut [i16], input: &[i16], b_q12: &[i16], d: usize) {
    let len = out.len();
    debug_assert_eq!(input.len(), len);
    debug_assert!(d >= 6 && d % 2 == 0 && d <= len && d <= b_q12.len());
    for ix in d..len {
        // in_ptr = &input[ix - 1]: in_ptr[0] is input[ix - 1]
        let mut out32_q12 = smulbb(input[ix - 1] as i32, b_q12[0] as i32);
        for j in 1..d {
            out32_q12 = out32_q12.wrapping_add(smulbb(input[ix - 1 - j] as i32, b_q12[j] as i32));
        }
        // Subtract prediction: in_ptr[1] is input[ix]
        out32_q12 = (input[ix] as i32).wrapping_shl(12).wrapping_sub(out32_q12);
        // Scale to Q0 and saturate
        out[ix] = sat16(rshift_round(out32_q12, 12));
    }
    // Set first d output samples to zero
    out[..d].fill(0);
}

/// `silk_DIV32_varQ(a, b, Qres)` (`silk/Inlines.h`): a good
/// approximation of `(a << Qres) / b` for same-sign, nonzero `a`/`b`,
/// refined once; requires `Qres >= 0`. Mirrors the reference's
/// unchecked/`_ovflw` intermediate arithmetic.
pub(crate) fn div32_varq(a32: i32, b32: i32, qres: u32) -> i32 {
    debug_assert!(b32 != 0);
    // Compute number of bits head room and normalize inputs
    let a_headrm = a32.abs().leading_zeros() - 1;
    let mut a32_nrm = a32.wrapping_shl(a_headrm);
    let b_headrm = b32.abs().leading_zeros() - 1;
    let b32_nrm = b32.wrapping_shl(b_headrm);

    // Inverse of b32, with 14 bits of precision
    let b32_inv = div32_16(i32::MAX >> 2, b32_nrm >> 16);

    // First approximation
    let mut result = smulwb(a32_nrm, b32_inv);

    // Compute residual by subtracting product of denominator and first
    // approximation; OK to overflow because the final value of a32_nrm
    // is always small
    a32_nrm = a32_nrm.wrapping_sub(smmul(b32_nrm, result).wrapping_shl(3));

    // Refinement
    result = smlawb(result, a32_nrm, b32_inv);

    // Convert to Qres domain
    let lshift = 29 + a_headrm as i32 - b_headrm as i32 - qres as i32;
    if lshift < 0 {
        // silk_LSHIFT_SAT32 on the refinement output: the saturation
        // bound never binds for this refinement's range, leaving only
        // the (wrapping) shift, as in the reference expansion
        result.wrapping_shl(-lshift as u32)
    } else if lshift < 32 {
        result >> lshift
    } else {
        // Avoid undefined result
        0
    }
}

/// `silk_insertion_sort_increasing_all_values_int16` (`silk/sort.c`):
/// insertion sort of the whole vector in increasing order.
pub(crate) fn insertion_sort_increasing_all_values_int16(a: &mut [i16]) {
    for i in 1..a.len() {
        let value = a[i];
        let mut j: isize = i as isize - 1;
        while j >= 0 && value < a[j as usize] {
            a[j as usize + 1] = a[j as usize];
            j -= 1;
        }
        // j >= -1 here, so j + 1 never overflows (unlike j as usize + 1)
        a[(j + 1) as usize] = value;
    }
}
