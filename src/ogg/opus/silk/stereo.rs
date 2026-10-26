// Adapted from TPT Cadence and the BSD-licensed Opus reference; see ../NOTICE.md.
//! SILK stereo mid/side prediction and unmixing (RFC 6716 §4.2.7–§4.2.8).
//!
//! Ports `silk/stereo_decode_pred.c` (predictor index entropy decoding +
//! dequantization, and the mid-only flag) and `silk/stereo_MS_to_LR.c`
//! (the MS→LR conversion) from libopus 1.5.2, preserving the exact
//! fixed-point operation order of the reference's generic macros.
//!
//! Bitstream layout per mid-channel SILK frame (decoded once per frame
//! before the per-channel side info, cf. `silk/dec_API.c`):
//! 1. a joint 25-symbol stage-1 index `n`, split as `n/5` for w0 and
//!    `n % 5` for w1,
//! 2. per weight, in order: the low part of the table index (uniform 3),
//!    then the interpolation sub-step (uniform 5) — first for w0's pair,
//!    then for w1's,
//! 3. the mid-only flag (one 2-symbol ICDF; only when the side channel
//!    is not otherwise known to be coded — the caller decides).
//!
//! The decoded weights are interpolated from the previous frame's values
//! over the first 8 ms of each frame while adding the side prediction,
//! then the processed mid/side pair is converted to left/right.
//!
//! SOURCE: Xiph.Org libopus 1.5.2, `silk/stereo_decode_pred.c`,
//! `silk/stereo_MS_to_LR.c`, `silk/define.h`, `silk/structs.h`
//! (BSD-3-Clause); cross-checked against RFC 6716 §4.2.7.1–§4.2.8.
#![allow(dead_code)]

use crate::ogg::opus::Result;
use crate::ogg::opus::range::{RangeDecoder, RangeEncoder};
use crate::ogg::opus::silk::sigproc::{rshift_round, sat16, smlabb, smlawb, smulbb, smulwb};
use crate::ogg::opus::silk::tables::{
    STEREO_ONLY_CODE_MID_ICDF, STEREO_PRED_JOINT_ICDF, STEREO_PRED_QUANT_Q13, UNIFORM3_ICDF,
    UNIFORM5_ICDF,
};

/// `STEREO_INTERP_LEN_MS` (`silk/define.h`; "must be even").
const STEREO_INTERP_LEN_MS: i32 = 8;

/// `SILK_FIX_CONST(0.5 / STEREO_QUANT_SUB_STEPS, 16)` with
/// `STEREO_QUANT_SUB_STEPS == 5`: `(0.1 * 2^16 + 0.5)` truncated = 6554.
/// (RFC 6716 §4.2.7.1 states this constant directly.)
const PRED_SUBSTEP_WEIGHT_Q16: i32 = 6554;

/// Mirrors `stereo_dec_state` (`silk/structs.h`): persistent stereo
/// decoder state, all zeros after a reset / mono↔stereo transition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StereoDecState {
    /// Q13 predictors applied to the previous frame, stored as the
    /// *combined* `w0 - w1` in slot 0. Like the reference's
    /// `opus_int16` field, out-of-range combined values wrap on store.
    pub(crate) pred_prev_q13: [i16; 2],
    /// Last two mid samples (the one-sample output delay history).
    pub(crate) s_mid: [i16; 2],
    /// Last two side-channel residual samples from the previous frame.
    pub(crate) s_side: [i16; 2],
}

/// `silk_stereo_decode_pred`: entropy-decode and dequantize the mid/side
/// prediction weights for one mid-channel SILK frame.
///
/// Returns `[w0_Q13, w1_Q13]`, where `w0` is already the combined
/// `w0 - w1` value the unmixing step applies (the reference subtracts
/// w1 here "because it helps when actually applying these"). Note that
/// the combined w0 can exceed the ±13732 table range.
pub(crate) fn decode_pred(dec: &mut RangeDecoder) -> Result<[i32; 2]> {
    let joint = dec.decode_icdf(&STEREO_PRED_JOINT_ICDF, 8)?;
    let phase = [joint / 5, joint % 5];
    let mut low_ix = [0usize; 2];
    let mut sub_ix = [0i32; 2];
    for n in 0..2 {
        low_ix[n] = (dec.decode_icdf(&UNIFORM3_ICDF, 8)? + 3 * phase[n]) as usize;
        sub_ix[n] = dec.decode_icdf(&UNIFORM5_ICDF, 8)? as i32;
    }

    // Dequantize: interpolate `2*sub+1` tenths of the way into the
    // table cell starting at `low_ix`.
    let mut pred = [0i32; 2];
    for n in 0..2 {
        let low_q13 = STEREO_PRED_QUANT_Q13[low_ix[n]] as i32;
        let step_q13 = smulwb(
            STEREO_PRED_QUANT_Q13[low_ix[n] + 1] as i32 - low_q13,
            PRED_SUBSTEP_WEIGHT_Q16,
        );
        pred[n] = smlabb(low_q13, step_q13, 2 * sub_ix[n] + 1);
    }

    pred[0] -= pred[1];
    Ok(pred)
}

/// `silk_stereo_decode_mid_only`: decode the flag that says only the mid
/// channel is coded for this interval (the side is then fed zeros).
pub(crate) fn decode_mid_only(dec: &mut RangeDecoder) -> Result<bool> {
    Ok(dec.decode_icdf(&STEREO_ONLY_CODE_MID_ICDF, 8)? != 0)
}

/// `silk_stereo_MS_to_LR`: convert the adaptive Mid/Side representation
/// to Left/Right.
///
/// `mid` and `side` each hold `frame_length + 2` samples: entry `[0..2]`
/// is scratch that receives the history from `state`, and entries
/// `[2..frame_length + 2]` are the frame's decoded mid signal and side
/// residual. On return, positions `[1..frame_length + 1]` hold the
/// frame's left/right output (the stereo layer's one-sample delay,
/// RFC 6716 §4.2.8) and the input tails are stashed in `state` for the
/// next frame.
///
/// The side prediction (a two-tap mid predictor: a 3-sample low-pass in
/// `pred0` plus the raw mid in `pred1`) is added over the whole frame,
/// with `pred0`/`pred1` ramping linearly from `state.pred_prev_q13` to
/// `pred_q13` across the first 8 ms. Requires
/// `frame_length >= 8 * fs_khz` (holds for every SILK frame size).
pub(crate) fn ms_to_lr(
    state: &mut StereoDecState,
    mid: &mut [i16],
    side: &mut [i16],
    pred_q13: &[i32; 2],
    fs_khz: u32,
    frame_length: usize,
) {
    let interp_len = (STEREO_INTERP_LEN_MS * fs_khz as i32) as usize;
    debug_assert!(mid.len() >= frame_length + 2 && side.len() >= frame_length + 2);
    debug_assert!(interp_len <= frame_length);

    // Buffering: pull in the previous frame's history, and save the tail
    // of this frame's input for the next call.
    mid[..2].copy_from_slice(&state.s_mid);
    side[..2].copy_from_slice(&state.s_side);
    state
        .s_mid
        .copy_from_slice(&mid[frame_length..frame_length + 2]);
    state
        .s_side
        .copy_from_slice(&side[frame_length..frame_length + 2]);

    // Interpolate predictors and add prediction to side channel. The
    // per-sample ramp step rounds towards nearest (RSHIFT_ROUND), and
    // SMULBB's i16 truncation of the (possibly out-of-i16) combined
    // predictor difference is part of the reference behavior.
    let denom_q16 = (1i32 << 16) / (STEREO_INTERP_LEN_MS * fs_khz as i32);
    let mut pred0 = state.pred_prev_q13[0] as i32;
    let mut pred1 = state.pred_prev_q13[1] as i32;
    let delta0 = rshift_round(smulbb(pred_q13[0] - pred0, denom_q16), 16);
    let delta1 = rshift_round(smulbb(pred_q13[1] - pred1, denom_q16), 16);
    for n in 0..interp_len {
        pred0 += delta0;
        pred1 += delta1;
        predict_side_sample(mid, side, n, pred0, pred1);
    }
    pred0 = pred_q13[0];
    pred1 = pred_q13[1];
    for n in interp_len..frame_length {
        predict_side_sample(mid, side, n, pred0, pred1);
    }
    state.pred_prev_q13 = [pred_q13[0] as i16, pred_q13[1] as i16];

    // Convert to left/right signals.
    for n in 0..frame_length {
        let m = mid[n + 1] as i32;
        let s = side[n + 1] as i32;
        mid[n + 1] = sat16(m + s);
        side[n + 1] = sat16(m - s);
    }
}

/// Body of the `silk_stereo_MS_to_LR` sample loop:
/// `side[n+1] += pred0 * (mid[n] + 2*mid[n+1] + mid[n+2])/4 + pred1 *
/// mid[n+1]`, in the reference's Q formats (Q11 window, Q13 weights,
/// Q8 side accumulator).
#[inline]
fn predict_side_sample(mid: &[i16], side: &mut [i16], n: usize, pred0_q13: i32, pred1_q13: i32) {
    let sum_q11 = (mid[n] as i32 + mid[n + 2] as i32 + ((mid[n + 1] as i32) << 1)) << 9;
    let mut sum_q8 = smlawb((side[n + 1] as i32) << 8, sum_q11, pred0_q13);
    sum_q8 = smlawb(sum_q8, (mid[n + 1] as i32) << 11, pred1_q13);
    side[n + 1] = sat16(rshift_round(sum_q8, 8));
}

// ---------------------------------------------------------------------------
// Encoder side: Left/Right → Mid/Side with quantized adaptive prediction
// (mirror of `silk_stereo_LR_to_MS` + `silk_stereo_encode_pred`)
// ---------------------------------------------------------------------------

/// Encoder-side mirror of [`StereoDecState`]: the previous frame's
/// combined Q13 predictors (as stored, `i16`-wrapping) and the one-sample
/// mid/side-residual history the MS transform exchanges between frames.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StereoEncState {
    pub pred_prev_q13: [i16; 2],
    pub s_mid: [i16; 2],
    pub s_side: [i16; 2],
}

/// The quantized predictor indices for one frame, in the order
/// [`decode_pred`] reads them: a joint 25-symbol stage-1 index
/// (`5·phase0 + phase1`), then per weight the low part (uniform 3) and
/// the interpolation sub-step (uniform 5). Weight 0 is the 3-sample
/// low-pass predictor, weight 1 the raw mid predictor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StereoPredIx {
    pub joint: u32,
    pub low: [u32; 2],
    pub sub: [u32; 2],
}

/// Writes [`StereoPredIx`] exactly as [`decode_pred`] reads it.
pub(crate) fn encode_stereo_pred(enc: &mut RangeEncoder, ix: &StereoPredIx) {
    enc.encode_icdf(ix.joint, &STEREO_PRED_JOINT_ICDF, 8);
    for n in 0..2 {
        enc.encode_icdf(ix.low[n], &UNIFORM3_ICDF, 8);
        enc.encode_icdf(ix.sub[n], &UNIFORM5_ICDF, 8);
    }
}

/// Writes the mid-only flag exactly as [`decode_mid_only`] reads it.
pub(crate) fn encode_mid_only_flag(enc: &mut RangeEncoder, mid_only: bool) {
    enc.encode_icdf(u32::from(mid_only), &STEREO_ONLY_CODE_MID_ICDF, 8);
}

/// The decoder-exact quantized Q13 value for table cell `low_ix`
/// interpolated `2·sub+1` tenths of the way in (the dequantization half
/// of [`decode_pred`]).
fn table_value_q13(low_ix: usize, sub_ix: i32) -> i32 {
    let low_q13 = STEREO_PRED_QUANT_Q13[low_ix] as i32;
    let step_q13 = smulwb(
        STEREO_PRED_QUANT_Q13[low_ix + 1] as i32 - low_q13,
        PRED_SUBSTEP_WEIGHT_Q16,
    );
    smlabb(low_q13, step_q13, 2 * sub_ix + 1)
}

/// Quantizes one Q13 predictor weight to the nearest encodable table
/// value, returning its `(low_ix, sub_ix)` indices (the joint phase is
/// derived as `low_ix / 3`).
fn quant_pred_weight(target_q13: i32) -> (usize, i32) {
    let t = target_q13.clamp(
        STEREO_PRED_QUANT_Q13[0] as i32,
        STEREO_PRED_QUANT_Q13[14] as i32,
    );
    let mut best = (0usize, 0i32);
    let mut best_err = i32::MAX;
    // low_ix spans 0..=14 (phase = low_ix / 3 selects the joint field's
    // region; the decoder's uniform-3 symbol re-derives low = low_ix mod 3).
    for low_ix in 0..15usize {
        for sub in 0..5i32 {
            let v = table_value_q13(low_ix, sub);
            let err = (v - t).abs();
            if err < best_err {
                best_err = err;
                best = (low_ix, sub);
            }
        }
    }
    best
}

/// `silk_stereo_LR_to_MS` (foundation shape): converts one frame of
/// internal-rate left/right input into the mid signal and the side
/// *residual* a decoder's [`ms_to_lr`] turns back into left/right.
///
/// `left`/`right` hold exactly `frame_length` samples. The outputs
/// `mid`/`side_resid` receive `frame_length + 2` samples: position 0 is
/// unused, position 1 is the previous frame's trailing sample (the
/// stereo layer's one-sample delay, identical to the decoder's
/// buffering), and positions `2..frame_length + 2` are this frame's
/// signal — the same layout `ms_to_lr` consumes on decode.
///
/// The mid/side predictor is chosen by least squares over the frame's
/// constant-weight region and quantized to the decoder's table; the
/// prediction is then removed with the same ramped arithmetic
/// [`ms_to_lr`] applies when adding it back, so a decode of the emitted
/// indices reconstructs the input mid/side pair bit-for-bit (modulo
/// `i16` saturation, which both sides clamp identically).
///
/// Returns the quantized indices, the combined Q13 predictors (decoder
/// convention: slot 0 = low-pass weight minus raw weight), and the frame
/// energies `(mid, side_residual)` as mean squares for the caller's
/// mid-only decision.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lr_to_ms(
    state: &mut StereoEncState,
    left: &[i16],
    right: &[i16],
    mid: &mut [i16],
    side_resid: &mut [i16],
    fs_khz: u32,
    frame_length: usize,
) -> (StereoPredIx, [i32; 2], f64, f64) {
    let interp_len = (STEREO_INTERP_LEN_MS * fs_khz as i32) as usize;
    debug_assert!(left.len() == frame_length && right.len() == frame_length);
    debug_assert!(mid.len() >= frame_length + 2 && side_resid.len() >= frame_length + 2);
    debug_assert!(interp_len <= frame_length);

    // Basic mid/side representation: mid = (L+R)>>1, side = (L-R)>>1,
    // with the decoder's one-sample-delay buffering (position 1 = the
    // previous frame's trailing sample, exactly what `ms_to_lr`'s
    // `s_mid`/`s_side` stash expects to see at index 0/1).
    mid[0] = state.s_mid[0];
    mid[1] = state.s_mid[1];
    side_resid[0] = state.s_side[0];
    side_resid[1] = state.s_side[1];
    for n in 0..frame_length {
        let l = left[n] as i32;
        let r = right[n] as i32;
        mid[n + 2] = sat16((l + r) >> 1);
        side_resid[n + 2] = sat16((l - r) >> 1);
    }
    // The decoder's per-frame mid/side input windows are `mid[n..]` /
    // `side[n..]` for n in 0..frame_length over the [frame+2] buffers —
    // i.e. frame output positions 1..=frame_length. Stash this frame's
    // trailing samples for the next call exactly like `ms_to_lr` does.
    state.s_mid = [mid[frame_length], mid[frame_length + 1]];
    state.s_side = [side_resid[frame_length], side_resid[frame_length + 1]];

    // Least-squares predictor over the constant-weight region
    // (frame positions interp_len..frame_length, i.e. buffer indices
    // n+1 for n >= interp_len). The decoder applies
    // `(w0q - w1q)·lowpass + w1q·raw_mid`, which in per-weight terms is
    // `w0q·(lowpass - raw_mid) + w1q·raw_mid` — so the regressors are
    // x0 = lowpass - raw_mid and x1 = raw_mid, and the solved weights
    // are quantized directly as w0q and w1q.
    let (mut sx0x0, mut sx1x1, mut sx0x1, mut sx0s, mut sx1s) = (0f64, 0f64, 0f64, 0f64, 0f64);
    for n in interp_len..frame_length {
        let m0 = mid[n] as f64;
        let m1 = mid[n + 1] as f64;
        let m2 = mid[n + 2] as f64;
        let lp = 0.25 * (m0 + m2 + 2.0 * m1);
        let x0 = lp - m1;
        let x1 = m1;
        let tgt = side_resid[n + 2] as f64;
        sx0x0 += x0 * x0;
        sx1x1 += x1 * x1;
        sx0x1 += x0 * x1;
        sx0s += x0 * tgt;
        sx1s += x1 * tgt;
    }
    let det = sx0x0 * sx1x1 - sx0x1 * sx0x1;
    let (w0, w1) = if det.abs() < 1e-9 {
        (0.0, 0.0)
    } else {
        (
            (sx0s * sx1x1 - sx1s * sx0x1) / det,
            (sx1s * sx0x0 - sx0s * sx0x1) / det,
        )
    };
    // Quantize each weight to the decoder's table. `low_ix` spans
    // 0..=14; the written uniform-3 symbol is `low_ix % 3` and the
    // joint index carries the region: joint = 5·(low0_ix/3) + low1_ix/3.
    let mut ix = StereoPredIx::default();
    let mut low_ixs = [0usize; 2];
    let mut pred_q13 = [0i32; 2];
    for (n, &target) in [w0, w1].iter().enumerate() {
        let (low_ix, sub) = quant_pred_weight((target * (1i32 << 13) as f64) as i32);
        low_ixs[n] = low_ix;
        ix.low[n] = (low_ix % 3) as u32;
        ix.sub[n] = sub as u32;
        pred_q13[n] = table_value_q13(low_ix, sub);
    }
    ix.joint = 5 * (low_ixs[0] as u32 / 3) + (low_ixs[1] as u32 / 3);

    // Combined predictors, decoder convention (slot 0 -= slot 1).
    let combined = [pred_q13[0] - pred_q13[1], pred_q13[1]];

    // Remove the prediction with the decoder's exact ramped arithmetic
    // (same denominators, same rounding), in reverse.
    let denom_q16 = (1i32 << 16) / (STEREO_INTERP_LEN_MS * fs_khz as i32);
    let mut pred0 = state.pred_prev_q13[0] as i32;
    let mut pred1 = state.pred_prev_q13[1] as i32;
    let delta0 = rshift_round(smulbb(combined[0] - pred0, denom_q16), 16);
    let delta1 = rshift_round(smulbb(combined[1] - pred1, denom_q16), 16);
    for n in 0..frame_length {
        if n < interp_len {
            pred0 += delta0;
            pred1 += delta1;
        } else if n == interp_len {
            pred0 = combined[0];
            pred1 = combined[1];
        }
        let sum_q11 = (mid[n] as i32 + mid[n + 2] as i32 + ((mid[n + 1] as i32) << 1)) << 9;
        let t0 = smulwb(sum_q11, pred0);
        let t1 = smulwb((mid[n + 1] as i32) << 11, pred1);
        side_resid[n + 2] = sat16(rshift_round(((side_resid[n + 2] as i32) << 8) - t0 - t1, 8));
    }
    state.pred_prev_q13 = [combined[0] as i16, combined[1] as i16];

    let mid_energy = mid[2..frame_length + 2]
        .iter()
        .map(|&v| (v as f64) * v as f64)
        .sum::<f64>()
        / frame_length as f64;
    let side_energy = side_resid[2..frame_length + 2]
        .iter()
        .map(|&v| (v as f64) * v as f64)
        .sum::<f64>()
        / frame_length as f64;
    (ix, combined, mid_energy, side_energy)
}
