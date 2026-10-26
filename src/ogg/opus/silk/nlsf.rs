// Adapted from TPT Cadence and the BSD-licensed Opus reference; see ../NOTICE.md.
//! SILK NLSF decoding and LPC conversion (Tier 2).
//!
//! Ports the normalized line spectral frequency decode path from
//! libopus 1.5.2 with bit-exact integer arithmetic:
//!
//! - [`nlsf_residual_dequant`] + [`nlsf_decode`]: predictive stage-2
//!   residual dequantization and stage-1 codebook add
//!   (`silk/NLSF_decode.c`).
//! - [`nlsf_stabilize`]: enforces minimum LSF spacing and border
//!   distance, converging or falling back to an insertion sort
//!   (`silk/NLSF_stabilize.c`).
//! - [`nlsf_interpolate`]: the inter-frame interpolation formula from
//!   `silk/decode_parameters.c` (NLSF0 for the leading subframes when
//!   `NLSFInterpCoef_Q2 < 4`).
//! - [`nlsf2a`]: NLSF → LPC via the piecewise-linear cos table and
//!   even/odd polynomial convolution, with the Q12 fit and the
//!   bandwidth-expansion stability loop (`silk/NLSF2A.c`,
//!   `silk/LPC_fit.c`, `silk/LPC_inv_pred_gain.c`).
//! - [`bwexpander`] / [`bwexpander_32`]: LPC bandwidth expansion
//!   (`silk/bwexpander.c`, `silk/bwexpander_32.c`).
//!
//! SOURCE: Xiph.Org libopus 1.5.2, `silk/NLSF_decode.c`,
//! `silk/NLSF_stabilize.c`, `silk/NLSF2A.c`, `silk/LPC_fit.c`,
//! `silk/LPC_inv_pred_gain.c`, `silk/bwexpander.c`,
//! `silk/bwexpander_32.c`, `silk/Inlines.h` (BSD-3-Clause).

// Unused until the Tier 3/4 synthesis and decoder assembly consume this
// module — same convention as `tables`/`sigproc`.
#![allow(dead_code)]

use super::decode_indices::{MAX_LPC_ORDER, nlsf_unpack};
use super::sigproc::{
    add_lshift32, add_sat16, div32, div32_16, insertion_sort_increasing_all_values_int16, mul,
    rshift_round, rshift_round64, smlawb, smlaww, smmul, smulbb, smulwb, smulww, sub_sat32,
};
use super::tables::{LSF_COS_TAB_FIX_Q12, NlsfCbStruct};

/// `MAX_LOOPS` (`silk/NLSF_stabilize.c`).
const MAX_LOOPS: u32 = 20;

/// `MAX_LPC_STABILIZE_ITERATIONS` (`silk/define.h`).
const MAX_LPC_STABILIZE_ITERATIONS: u32 = 16;

/// `SILK_FIX_CONST(NLSF_QUANT_LEVEL_ADJ, 10)` with
/// `NLSF_QUANT_LEVEL_ADJ = 0.1` (`silk/define.h`).
const NLSF_QUANT_LEVEL_ADJ_Q10: i32 = 102;

/// `SILK_FIX_CONST(0.999, 16)` (`silk/LPC_fit.c` chirp bias).
const SILK_FIX_0999_Q16: i32 = 65470;

/// `SILK_FIX_CONST(0.99975, 24)` — `A_LIMIT` in `silk/LPC_inv_pred_gain.c`.
const A_LIMIT_QA24: i32 = 16_773_022;

/// `SILK_FIX_CONST(1.0f / MAX_PREDICTION_POWER_GAIN, 30)` with
/// `MAX_PREDICTION_POWER_GAIN = 1e4f` (`silk/define.h`).
const MIN_INVERSE_PRED_GAIN_Q30: i32 = 107_374;

/// `#define QA` inside `silk/LPC_inv_pred_gain.c`.
const INVERSE_PRED_GAIN_QA: u32 = 24;

/// `#define QA` inside `silk/NLSF2A.c`.
const NLSF2A_QA: u32 = 16;

/// Coefficient reordering found to maximize the numerical accuracy of
/// [`nlsf2a_find_poly`] (`silk/NLSF2A.c`).
const NLSF2A_ORDERING16: [u8; 16] = [0, 15, 8, 7, 4, 11, 12, 3, 2, 13, 10, 5, 6, 9, 14, 1];
const NLSF2A_ORDERING10: [u8; 10] = [0, 9, 6, 3, 4, 5, 8, 1, 2, 7];

/// `MUL32_FRAC_Q(a, b, Q)` (`silk/LPC_inv_pred_gain.c`).
fn mul32_frac_q(a: i32, b: i32, q: u32) -> i32 {
    rshift_round64(a as i64 * b as i64, q) as i32
}

/// `silk_INVERSE32_varQ` (`silk/Inlines.h`): approximation of
/// `(1 << Qres) / b32`. Requires `b32 != 0` and `Qres > 0`; all
/// intermediate wrapping mirrors the reference's unchecked/`_ovflw`
/// arithmetic.
pub(crate) fn inverse32_varq(b32: i32, qres: u32) -> i32 {
    let b_headrm = b32.abs().leading_zeros() - 1;
    let b32_nrm = b32.wrapping_shl(b_headrm);
    // Inverse of b32, with 14 bits of precision
    let b32_inv = div32_16(i32::MAX >> 2, b32_nrm >> 16);
    // First approximation
    let mut result = b32_inv.wrapping_shl(16);
    // Compute residual by subtracting product of denominator and first
    // approximation from one
    let err_q32 = (1_i32 << 29)
        .wrapping_sub(smulwb(b32_nrm, b32_inv))
        .wrapping_shl(3);
    // Refinement
    result = smlaww(result, err_q32, b32_inv);
    // Convert to Qres domain
    let lshift = 61 - b_headrm as i32 - qres as i32;
    if lshift <= 0 {
        // silk_LSHIFT_SAT32: the `_ovflw` shift already wraps, so the
        // outer SAT32 can never trigger and is omitted
        result.wrapping_shl(-lshift as u32)
    } else if lshift < 32 {
        result >> lshift
    } else {
        // Avoid undefined result
        0
    }
}

/// `silk_NLSF_residual_dequant` (`silk/NLSF_decode.c`): predictive
/// dequantizer for the stage-2 NLSF residuals. `indices` holds `order`
/// quantization indices; `x_q10` receives the dequantized residuals in
/// Q10.
fn nlsf_residual_dequant(
    x_q10: &mut [i16],
    indices: &[i8],
    pred_coef_q8: &[u8],
    quant_step_size_q16: i32,
) {
    let order = x_q10.len();
    debug_assert_eq!(indices.len(), order);
    debug_assert_eq!(pred_coef_q8.len(), order);
    let mut out_q10: i32 = 0;
    for i in (0..order).rev() {
        let pred_q10 = smulbb(out_q10, pred_coef_q8[i] as i32) >> 8;
        out_q10 = (indices[i] as i32) << 10;
        if out_q10 > 0 {
            out_q10 -= NLSF_QUANT_LEVEL_ADJ_Q10;
        } else if out_q10 < 0 {
            out_q10 += NLSF_QUANT_LEVEL_ADJ_Q10;
        }
        out_q10 = smlawb(pred_q10, out_q10, quant_step_size_q16);
        x_q10[i] = out_q10 as i16;
    }
}

/// `silk_NLSF_decode` (`silk/NLSF_decode.c`): reconstructs the
/// quantized NLSF vector (Q15) from the codebook path `nlsf_indices`
/// (`[LPC_order + 1]`: stage-1 index followed by the stage-2
/// residuals) and stabilizes it.
pub(crate) fn nlsf_decode(p_nlsf_q15: &mut [i16], nlsf_indices: &[i8], cb: &NlsfCbStruct) {
    let order = cb.order as usize;
    debug_assert_eq!(p_nlsf_q15.len(), order);
    debug_assert_eq!(nlsf_indices.len(), order + 1);
    debug_assert!((0..cb.n_vectors as i32).contains(&(nlsf_indices[0] as i32)));

    // Unpack entropy table indices and predictor for current CB1 index
    let (_, pred_q8) = nlsf_unpack(cb, nlsf_indices[0] as usize);

    // Predictive residual dequantizer
    let mut res_q10 = [0i16; MAX_LPC_ORDER];
    nlsf_residual_dequant(
        &mut res_q10[..order],
        &nlsf_indices[1..=order],
        &pred_q8[..order],
        cb.quant_step_size_q16 as i32,
    );

    // Apply inverse square-rooted weights to first stage and add to output
    let cb1_index = nlsf_indices[0] as usize;
    let cb1_element = &cb.cb1_nlsf_q8[cb1_index * order..][..order];
    let cb1_wght_q9 = &cb.cb1_wght_q9[cb1_index * order..][..order];
    for i in 0..order {
        let nlsf_q15_tmp = add_lshift32(
            div32_16((res_q10[i] as i32) << 14, cb1_wght_q9[i] as i32),
            cb1_element[i] as i32,
            7,
        );
        p_nlsf_q15[i] = nlsf_q15_tmp.clamp(0, 32767) as i16;
    }

    // NLSF stabilization
    nlsf_stabilize(p_nlsf_q15, cb.delta_min_q15);
}

/// `silk_NLSF_stabilize` (`silk/NLSF_stabilize.c`): moves NLSFs apart
/// that are too close to each other or to the 0/1 borders, with
/// minimum Euclidean distance. `n_delta_min_q15` has `len + 1` entries.
pub(crate) fn nlsf_stabilize(nlsf_q15: &mut [i16], n_delta_min_q15: &[i16]) {
    let l = nlsf_q15.len();
    debug_assert_eq!(n_delta_min_q15.len(), l + 1);
    // This is necessary to ensure an output within range of a opus_int16
    debug_assert!(n_delta_min_q15[l] >= 1);

    for _loops in 0..MAX_LOOPS {
        // Find smallest distance
        // First element
        let mut min_diff_q15 = nlsf_q15[0] as i32 - n_delta_min_q15[0] as i32;
        let mut i_min: usize = 0;
        // Middle elements
        for i in 1..l {
            let diff_q15 =
                nlsf_q15[i] as i32 - (nlsf_q15[i - 1] as i32 + n_delta_min_q15[i] as i32);
            if diff_q15 < min_diff_q15 {
                min_diff_q15 = diff_q15;
                i_min = i;
            }
        }
        // Last element
        let diff_q15 = (1 << 15) - (nlsf_q15[l - 1] as i32 + n_delta_min_q15[l] as i32);
        if diff_q15 < min_diff_q15 {
            min_diff_q15 = diff_q15;
            i_min = l;
        }

        // Now check if the smallest distance is non-negative
        if min_diff_q15 >= 0 {
            return;
        }

        if i_min == 0 {
            // Move away from lower limit
            nlsf_q15[0] = n_delta_min_q15[0];
        } else if i_min == l {
            // Move away from higher limit
            nlsf_q15[l - 1] = ((1 << 15) - n_delta_min_q15[l] as i32) as i16;
        } else {
            // Find the lower extreme for the location of the current
            // center frequency
            let mut min_center_q15: i32 = 0;
            for &delta in &n_delta_min_q15[..i_min] {
                min_center_q15 += delta as i32;
            }
            min_center_q15 += (n_delta_min_q15[i_min] >> 1) as i32;

            // Find the upper extreme for the location of the current
            // center frequency
            let mut max_center_q15: i32 = 1 << 15;
            for &delta in n_delta_min_q15[i_min + 1..=l].iter().rev() {
                max_center_q15 -= delta as i32;
            }
            max_center_q15 -= (n_delta_min_q15[i_min] >> 1) as i32;

            // Move apart, sorted by value, keeping the same center frequency
            let center_freq_q15 =
                rshift_round(nlsf_q15[i_min - 1] as i32 + nlsf_q15[i_min] as i32, 1)
                    .clamp(min_center_q15, max_center_q15) as i16;
            nlsf_q15[i_min - 1] =
                (center_freq_q15 as i32 - (n_delta_min_q15[i_min] >> 1) as i32) as i16;
            nlsf_q15[i_min] = (nlsf_q15[i_min - 1] as i32 + n_delta_min_q15[i_min] as i32) as i16;
        }
    }

    // Safe and simple fall back method, which is less ideal than the
    // above (runs when the loop above completed MAX_LOOPS iterations)
    // Insertion sort (fast for already almost sorted arrays)
    insertion_sort_increasing_all_values_int16(nlsf_q15);

    // First NLSF should be no less than NDeltaMin[0]
    nlsf_q15[0] = nlsf_q15[0].max(n_delta_min_q15[0]);

    // Keep delta_min distance between the NLSFs
    for i in 1..l {
        nlsf_q15[i] = nlsf_q15[i].max(add_sat16(nlsf_q15[i - 1] as i32, n_delta_min_q15[i] as i32));
    }

    // Last NLSF should be no higher than 1 - NDeltaMin[L]
    nlsf_q15[l - 1] = (nlsf_q15[l - 1] as i32).min((1 << 15) - n_delta_min_q15[l] as i32) as i16;

    // Keep NDeltaMin distance between the NLSFs
    for i in (0..l - 1).rev() {
        nlsf_q15[i] =
            (nlsf_q15[i] as i32).min(nlsf_q15[i + 1] as i32 - n_delta_min_q15[i + 1] as i32) as i16;
    }
}

/// NLSF interframe interpolation (the `NLSFInterpCoef_Q2 < 4` branch of
/// `silk_decode_parameters` in `silk/decode_parameters.c`):
/// `dst[i] = prev[i] + (coef * (cur[i] - prev[i])) >> 2`.
pub(crate) fn nlsf_interpolate(
    dst: &mut [i16],
    prev_nlsf_q15: &[i16],
    cur_nlsf_q15: &[i16],
    coef_q2: i32,
) {
    debug_assert!((0..=4).contains(&coef_q2));
    debug_assert_eq!(dst.len(), prev_nlsf_q15.len());
    debug_assert_eq!(dst.len(), cur_nlsf_q15.len());
    for i in 0..dst.len() {
        dst[i] = (prev_nlsf_q15[i] as i32
            + (mul(coef_q2, cur_nlsf_q15[i] as i32 - prev_nlsf_q15[i] as i32) >> 2))
            as i16;
    }
}

/// `silk_NLSF2A_find_poly` (`silk/NLSF2A.c`): builds one intermediate
/// polynomial (QA coefficients, `dd + 1` entries) from the interleaved
/// `2*cos(LSF)` vector starting at `clsf_off`.
fn nlsf2a_find_poly(
    out: &mut [i32],
    cos_lsf_qa: &[i32; MAX_LPC_ORDER],
    clsf_off: usize,
    dd: usize,
) {
    out[0] = 1 << NLSF2A_QA;
    out[1] = -cos_lsf_qa[clsf_off];
    for k in 1..dd {
        let ftmp = cos_lsf_qa[clsf_off + 2 * k]; /* QA */
        out[k + 1] = (out[k - 1] << 1)
            .wrapping_sub(rshift_round64(ftmp as i64 * out[k] as i64, NLSF2A_QA) as i32);
        for n in (2..=k).rev() {
            out[n] = out[n]
                .wrapping_add(out[n - 2])
                .wrapping_sub(rshift_round64(ftmp as i64 * out[n - 1] as i64, NLSF2A_QA) as i32);
        }
        out[1] = out[1].wrapping_sub(ftmp);
    }
}

/// `silk_NLSF2A` (`silk/NLSF2A.c`): computes whitening filter
/// coefficients (Q12, monic without the leading 1) from normalized line
/// spectral frequencies (Q15). `d` must be 10 or 16.
pub(crate) fn nlsf2a(a_q12: &mut [i16], nlsf: &[i16], d: usize) {
    debug_assert!(d == 10 || d == 16);
    debug_assert_eq!(a_q12.len(), d);
    debug_assert_eq!(nlsf.len(), d);

    // convert LSFs to 2*cos(LSF), using piecewise linear curve from table
    let ordering: &[u8] = if d == 16 {
        &NLSF2A_ORDERING16
    } else {
        &NLSF2A_ORDERING10
    };
    let mut cos_lsf_qa = [0i32; MAX_LPC_ORDER];
    for k in 0..d {
        debug_assert!(nlsf[k] >= 0);

        // f_int on a scale 0-127 (rounded down)
        let f_int = (nlsf[k] >> (15 - 7)) as usize;

        // f_frac, range: 0..255
        let f_frac = nlsf[k] as i32 - ((f_int as i32) << (15 - 7));

        debug_assert!(f_int < LSF_COS_TAB_FIX_Q12.len() - 1);

        // Read start and end value from table
        let cos_val = LSF_COS_TAB_FIX_Q12[f_int] as i32; /* Q12 */
        let delta = LSF_COS_TAB_FIX_Q12[f_int + 1] as i32 - cos_val; /* Q12, range 0..200 */

        // Linear interpolation
        cos_lsf_qa[ordering[k] as usize] = rshift_round(
            (cos_val << 8).wrapping_add(mul(delta, f_frac)),
            20 - NLSF2A_QA,
        ); /* QA */
    }

    let dd = d >> 1;

    // generate even and odd polynomials using convolution
    let mut p_poly = [0i32; MAX_LPC_ORDER / 2 + 1];
    let mut q_poly = [0i32; MAX_LPC_ORDER / 2 + 1];
    nlsf2a_find_poly(&mut p_poly, &cos_lsf_qa, 0, dd);
    nlsf2a_find_poly(&mut q_poly, &cos_lsf_qa, 1, dd);

    // convert even and odd polynomials to opus_int32 Q12 filter coefs
    let mut a32_qa1 = [0i32; MAX_LPC_ORDER];
    for k in 0..dd {
        let ptmp = p_poly[k + 1].wrapping_add(p_poly[k]);
        let qtmp = q_poly[k + 1].wrapping_sub(q_poly[k]);

        // the Ptmp and Qtmp values at this stage need to fit in int32
        a32_qa1[k] = qtmp.wrapping_neg().wrapping_sub(ptmp); /* QA+1 */
        a32_qa1[d - k - 1] = qtmp.wrapping_sub(ptmp); /* QA+1 */
    }

    // Convert int32 coefficients to Q12 int16 coefs
    lpc_fit(a_q12, &mut a32_qa1[..d], 12, NLSF2A_QA + 1, d);

    let mut i = 0;
    while lpc_inverse_pred_gain(a_q12, d) == 0 && i < MAX_LPC_STABILIZE_ITERATIONS {
        // Prediction coefficients are (too close to) unstable; apply
        // bandwidth expansion on the unscaled coefficients, convert to
        // Q12 and measure again
        bwexpander_32(&mut a32_qa1[..d], 65536 - (2 << i));
        for (a, &a32) in a_q12[..d].iter_mut().zip(&a32_qa1[..d]) {
            *a = rshift_round(a32, NLSF2A_QA + 1 - 12) as i16; /* QA+1 -> Q12 */
        }
        i += 1;
    }
}

/// `silk_LPC_fit` (`silk/LPC_fit.c`): converts `a_qin` (Q`qin`) to
/// `a_qout` (Q`qout`), bandwidth-expanding (and finally clipping) so
/// the Q`qout` coefficients never wrap around.
pub(crate) fn lpc_fit(a_qout: &mut [i16], a_qin: &mut [i32], qout: u32, qin: u32, d: usize) {
    debug_assert_eq!(a_qout.len(), d);
    debug_assert!(a_qin.len() >= d);
    let qshift = qin - qout;
    let mut idx: usize = 0;
    let mut iterations = 0usize;

    // Limit the maximum absolute value of the prediction coefficients,
    // so that they'll fit in int16
    while iterations < 10 {
        // Find maximum absolute value and its index
        let mut maxabs: i32 = 0;
        for (k, &a) in a_qin[..d].iter().enumerate() {
            let absval = a.abs();
            if absval > maxabs {
                maxabs = absval;
                idx = k;
            }
        }
        maxabs = rshift_round(maxabs, qshift);

        if maxabs > i16::MAX as i32 {
            // Reduce magnitude of prediction coefficients
            // ( silk_int32_MAX >> 14 ) + silk_int16_MAX = 163838
            maxabs = maxabs.min(163838);
            let chirp_q16 = SILK_FIX_0999_Q16.wrapping_sub(div32(
                (maxabs - i16::MAX as i32) << 14,
                mul(maxabs, idx as i32 + 1) >> 2,
            ));
            bwexpander_32(&mut a_qin[..d], chirp_q16);
        } else {
            break;
        }
        iterations += 1;
    }

    if iterations == 10 {
        // Reached the last iteration, clip the coefficients
        for k in 0..d {
            a_qout[k] =
                rshift_round(a_qin[k], qshift).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            a_qin[k] = (a_qout[k] as i32) << qshift;
        }
    } else {
        for (a, &a_in) in a_qout[..d].iter_mut().zip(&a_qin[..d]) {
            *a = rshift_round(a_in, qshift) as i16;
        }
    }
}

/// `silk_LPC_inverse_pred_gain_QA_c` (`silk/LPC_inv_pred_gain.c`):
/// inverse prediction gain in the energy domain, Q30 — 0 signals an
/// unstable (or too-close-to-unstable) filter. Operates on QA
/// coefficients, consuming `a_qa` as scratch.
fn lpc_inverse_pred_gain_qa(a_qa: &mut [i32], order: usize) -> i32 {
    let mut inv_gain_q30: i32 = 1 << 30;
    for k in (1..order).rev() {
        if a_qa[k] > A_LIMIT_QA24 || a_qa[k] < -A_LIMIT_QA24 {
            return 0;
        }

        let rc_q31 = -(a_qa[k] << (31 - INVERSE_PRED_GAIN_QA));

        let rc_mult1_q30 = (1_i32 << 30).wrapping_sub(smmul(rc_q31, rc_q31));

        inv_gain_q30 = smmul(inv_gain_q30, rc_mult1_q30).wrapping_shl(2);
        if inv_gain_q30 < MIN_INVERSE_PRED_GAIN_Q30 {
            return 0;
        }

        let mult2q = 32 - rc_mult1_q30.abs().leading_zeros();
        let rc_mult2 = inverse32_varq(rc_mult1_q30, mult2q + 30);

        for n in 0..((k + 1) >> 1) {
            let tmp1 = a_qa[n];
            let tmp2 = a_qa[k - n - 1];
            let tmp64 = rshift_round64(
                sub_sat32(tmp1, mul32_frac_q(tmp2, rc_q31, 31)) as i64 * rc_mult2 as i64,
                mult2q,
            );
            if tmp64 > i32::MAX as i64 || tmp64 < i32::MIN as i64 {
                return 0;
            }
            a_qa[n] = tmp64 as i32;
            let tmp64 = rshift_round64(
                sub_sat32(tmp2, mul32_frac_q(tmp1, rc_q31, 31)) as i64 * rc_mult2 as i64,
                mult2q,
            );
            if tmp64 > i32::MAX as i64 || tmp64 < i32::MIN as i64 {
                return 0;
            }
            a_qa[k - n - 1] = tmp64 as i32;
        }
    }

    if a_qa[0] > A_LIMIT_QA24 || a_qa[0] < -A_LIMIT_QA24 {
        return 0;
    }

    let rc_q31 = -(a_qa[0] << (31 - INVERSE_PRED_GAIN_QA));

    let rc_mult1_q30 = (1_i32 << 30).wrapping_sub(smmul(rc_q31, rc_q31));

    inv_gain_q30 = smmul(inv_gain_q30, rc_mult1_q30).wrapping_shl(2);
    if inv_gain_q30 < MIN_INVERSE_PRED_GAIN_Q30 {
        return 0;
    }

    inv_gain_q30
}

/// `silk_LPC_inverse_pred_gain_c` (`silk/LPC_inv_pred_gain.c`):
/// computes the inverse prediction gain of the Q12 filter `a_q12`,
/// returning 0 for filters that are (nearly) unstable.
pub(crate) fn lpc_inverse_pred_gain(a_q12: &[i16], order: usize) -> i32 {
    debug_assert!(order <= MAX_LPC_ORDER);
    let mut dc_resp: i32 = 0;
    let mut atmp_qa = [0i32; MAX_LPC_ORDER];
    for (&a, atmp) in a_q12[..order].iter().zip(&mut atmp_qa) {
        dc_resp = dc_resp.wrapping_add(a as i32);
        *atmp = (a as i32) << (INVERSE_PRED_GAIN_QA - 12);
    }
    if dc_resp >= 4096 {
        return 0;
    }
    lpc_inverse_pred_gain_qa(&mut atmp_qa[..order], order)
}

/// `silk_bwexpander` (`silk/bwexpander.c`): chirp (bandwidth expand) an
/// LP AR filter in place. Deliberately uses rounded products instead of
/// `silk_SMULWB`, whose bias can lead to unstable filters.
pub(crate) fn bwexpander(ar: &mut [i16], chirp_q16: i32) {
    let d = ar.len();
    debug_assert!(d >= 1);
    let mut chirp_q16 = chirp_q16;
    let chirp_minus_one_q16 = chirp_q16 - 65536;

    for a in ar.iter_mut().take(d - 1) {
        *a = rshift_round(mul(chirp_q16, *a as i32), 16) as i16;
        chirp_q16 = chirp_q16.wrapping_add(rshift_round(mul(chirp_q16, chirp_minus_one_q16), 16));
    }
    ar[d - 1] = rshift_round(mul(chirp_q16, ar[d - 1] as i32), 16) as i16;
}

/// `silk_bwexpander_32` (`silk/bwexpander_32.c`): 32-input variant of
/// [`bwexpander`].
pub(crate) fn bwexpander_32(ar: &mut [i32], chirp_q16: i32) {
    let d = ar.len();
    debug_assert!(d >= 1);
    let mut chirp_q16 = chirp_q16;
    let chirp_minus_one_q16 = chirp_q16 - 65536;

    for a in ar.iter_mut().take(d - 1) {
        *a = smulww(chirp_q16, *a);
        chirp_q16 = chirp_q16.wrapping_add(rshift_round(mul(chirp_q16, chirp_minus_one_q16), 16));
    }
    ar[d - 1] = smulww(chirp_q16, ar[d - 1]);
}
