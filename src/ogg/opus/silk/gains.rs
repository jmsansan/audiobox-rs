// Adapted from TPT Cadence and the BSD-licensed Opus reference; see ../NOTICE.md.
//! SILK gain dequantization (RFC 6716 §4.2.4).
//!
//! Ports the decoder half of `silk/gain_quant.c`
//! (`silk_gains_dequant`, invoked from `silk/decode_parameters.c`),
//! `silk/log2lin.c`, and the gain-related cross-frame state handling
//! from `silk/dec_API.c`/`silk/init_decoder.c`.
//!
//! The gain index *symbols* are entropy-decoded by
//! [`super::decode_indices`] into `SideInfoIndices::gains_indices`
//! (absolute 0..=63 for the first subframe of an independently coded
//! frame, delta 0..=40 otherwise); this module accumulates them onto the
//! persistent `psDec->LastGainIndex` state and converts the result into
//! the linear Q16 gains the synthesis consumes:
//!
//! - First subframe, independent coding: the index is absolute, but not
//!   allowed to drop more than 16 steps (~21.8 dB) below the previous
//!   frame's last index. The clamp is inert right after a decoder reset
//!   (state 0) and is disabled on packet loss by forcing the state to
//!   [`LAST_GAIN_INDEX_ON_PACKET_LOSS`], so the energy cannot "bounce
//!   back" when loss hits during a fade-down (`silk/dec_API.c`; the RFC
//!   only says the clamp MAY be skipped, libopus does it this way).
//! - Every other subframe (and the first one under conditional coding):
//!   a delta index 0..=40, i.e. an accumulated step of
//!   `[MIN_DELTA_GAIN_QUANT, MAX_DELTA_GAIN_QUANT]` = −4..=36, with the
//!   step size doubled for large increases so the top level stays
//!   reachable (the encoder-side mapping in `silk_gains_quant`).
//! - The accumulated log-domain index (0..=63) maps through
//!   [`log2lin`] to a Q16 gain in `81920..=1686110208`
//!   (RFC 6716 §4.2.4's stated bounds; both endpoints are pinned by
//!   tests).
//!
//! The remaining gain smoothing lives where the reference puts it: the
//! intra-subframe gain ramp keyed on `psDec->prev_gain_Q16`
//! (`silk/decode_core.c`) belongs to `synthesis.rs` (Tier 3), and the
//! comfort-noise gain smoothing (`silk/CNG.c`) to the CNG/PLC work.
//!
//! SOURCE: Xiph.Org libopus 1.5.2, `silk/gain_quant.c`,
//! `silk/log2lin.c`, `silk/dec_API.c`, `silk/init_decoder.c`,
//! `silk/define.h` (BSD-3-Clause).
#![allow(dead_code)]

use crate::ogg::opus::silk::decode_indices::MAX_NB_SUBFR;
use crate::ogg::opus::silk::sigproc::{smlawb, smulbb, smulwb};

/// `MIN_QGAIN_DB` / `MAX_QGAIN_DB` / `N_LEVELS_QGAIN` (`silk/define.h`):
/// the quantized gain spans 2–88 dB over 64 levels, uniform in the log
/// domain.
const MIN_QGAIN_DB: i32 = 2;
const MAX_QGAIN_DB: i32 = 88;
const N_LEVELS_QGAIN: i32 = 64;
/// `MAX_DELTA_GAIN_QUANT` / `MIN_DELTA_GAIN_QUANT` (`silk/define.h`):
/// bounds of one accumulated delta step; the transmitted symbol is the
/// step shifted up by 4 into `0..=40`.
const MAX_DELTA_GAIN_QUANT: i32 = 36;
const MIN_DELTA_GAIN_QUANT: i32 = -4;

/// `OFFSET` (`gain_quant.c`): log-domain (Q7) gain of index 0 —
/// `2 dB·128/6` (floor) plus a 16 dB offset = 2090.
const OFFSET: i32 = (MIN_QGAIN_DB * 128) / 6 + 16 * 128;
/// `SCALE_Q16` (`gain_quant.c`, encoder side; kept for the round-trip
/// test) = 2251.
const SCALE_Q16: i32 = (65536 * (N_LEVELS_QGAIN - 1)) / (((MAX_QGAIN_DB - MIN_QGAIN_DB) * 128) / 6);
/// `INV_SCALE_Q16` (`gain_quant.c`) = `0x1D1C71` = 1907825, the constant
/// the RFC quotes in §4.2.4's `gain_Q16[k]` formula.
const INV_SCALE_Q16: i32 =
    (65536 * (((MAX_QGAIN_DB - MIN_QGAIN_DB) * 128) / 6)) / (N_LEVELS_QGAIN - 1);
/// The `silk_min_32(..., 3967)` clamp ("31 in Q7") applied to the log
/// gain before conversion.
const LOG_GAIN_MAX_Q7: i32 = 3967;

/// The `psDec->LastGainIndex` value libopus forces on packet loss
/// ("remove the gain clamping to prevent having the energy bounce back
/// if we lose packets when the energy is going down") and when the side
/// channel starts coding again after a mid-only period
/// (`silk/dec_API.c`). A freshly initialized/reset decoder state starts
/// at 0 instead (`silk_init_decoder`), which leaves the 16-step-down
/// clamp inert either way — both values are below the clamp's 16-step
/// reach, matching the RFC's note that the clamp is skipped when no
/// previous gain is meaningful.
pub(crate) const LAST_GAIN_INDEX_ON_PACKET_LOSS: i8 = 10;

/// `silk_log2lin` (`silk/log2lin.c`): approximation of
/// `2^(in_log_q7 / 128.0)` — inverse of the encoder's `silk_lin2log`.
///
/// Bit-exact port of the piece-wise parabolic approximation, including
/// the branch split at `inLog_Q7 == 2048`: the two branches differ only
/// in where the truncating `>> 7` lands (`(out*t) >> 7` vs
/// `(out >> 7)*t`), which is observable for large outputs, so the
/// reference's branch structure is preserved rather than unified.
pub(crate) fn log2lin(in_log_q7: i32) -> i32 {
    if in_log_q7 < 0 {
        return 0;
    } else if in_log_q7 >= LOG_GAIN_MAX_Q7 {
        return i32::MAX;
    }
    let out = 1i32 << (in_log_q7 >> 7);
    let frac_q7 = in_log_q7 & 0x7F;
    /* Q7 parabolic correction term shared by both branches */
    let correction_q7 = smlawb(frac_q7, smulbb(frac_q7, 128 - frac_q7), -174);
    if in_log_q7 < 2048 {
        /* Piece-wise parabolic approximation */
        out + ((out * correction_q7) >> 7)
    } else {
        /* Piece-wise parabolic approximation */
        out + ((out >> 7) * correction_q7)
    }
}

/// `silk_gains_dequant` (`silk/gain_quant.c`): dequantizes the
/// transmitted gain index symbols into linear Q16 gains, threading
/// `prev_ind` — the persistent `psDec->LastGainIndex` — through the
/// delta accumulation exactly as the reference does, once per subframe.
///
/// `conditional` is `condCoding == CODE_CONDITIONALLY`: when false the
/// first subframe's symbol is an absolute index (with the inter-frame
/// 16-step-down clamp), otherwise it — and every later subframe
/// regardless — is a delta in `0..=40`. `nb_subfr` is 4 (20 ms frames)
/// or 2 (10 ms); only that many entries of `gain_q16` are written.
pub(crate) fn gains_dequant(
    gain_q16: &mut [i32; MAX_NB_SUBFR],
    ind: &[i8; MAX_NB_SUBFR],
    prev_ind: &mut i8,
    conditional: bool,
    nb_subfr: usize,
) {
    debug_assert!(nb_subfr == MAX_NB_SUBFR || nb_subfr == MAX_NB_SUBFR / 2);
    let mut prev = i32::from(*prev_ind);
    for k in 0..nb_subfr {
        if k == 0 && !conditional {
            /* Gain index is not allowed to go down more than 16 steps
             * (~21.8 dB). Inert after reset/packet loss: the state is
             * then 0 or LAST_GAIN_INDEX_ON_PACKET_LOSS, both < 16. */
            prev = i32::from(ind[0]).max(prev - 16);
        } else {
            /* Delta index */
            let ind_tmp = i32::from(ind[k]) + MIN_DELTA_GAIN_QUANT;

            /* Accumulate deltas; past `double_step_size_threshold` the
             * effective step size doubles so the max gain level can be
             * reached (mirroring the encoder's `silk_gains_quant`). */
            let double_step_size_threshold = 2 * MAX_DELTA_GAIN_QUANT - N_LEVELS_QGAIN + prev;
            if ind_tmp > double_step_size_threshold {
                prev += ind_tmp * 2 - double_step_size_threshold;
            } else {
                prev += ind_tmp;
            }
        }
        prev = prev.clamp(0, N_LEVELS_QGAIN - 1);

        /* Scale and convert to linear scale */
        let log_gain = (smulwb(INV_SCALE_Q16, prev) + OFFSET).min(LOG_GAIN_MAX_Q7);
        gain_q16[k] = log2lin(log_gain);
    }
    *prev_ind = prev as i8;
}

/// `silk_lin2log` (`silk/lin2log.c`): approximation of
/// `log2(in) * 128` — inverse of [`log2lin`], used by the encoder's gain
/// quantizer (`silk/gain_quant.c`).
///
/// Bit-exact port including the `silk_CLZ_FRAC` inline (`frac_Q7` = the 7
/// bits after the leading one; the reference computes it via
/// `ROR32(in, 24 - lz) & 0x7f`, which equals the shift form below for
/// every positive input).
pub(crate) fn lin2log(in_lin: i32) -> i32 {
    debug_assert!(in_lin > 0);
    let lz = in_lin.leading_zeros() as i32;
    let frac_q7 = ((in_lin << lz) >> 24) & 0x7f;
    smlawb(frac_q7, frac_q7 * (128 - frac_q7), 179) + ((31 - lz) << 7)
}

/// `silk_gains_quant` (`silk/gain_quant.c`): quantizes subframe gains to
/// the transmitted indices — the encoder half that [`gains_dequant`]
/// inverts bit-exactly.
///
/// `gain_q16` is both input and output: it arrives with the desired
/// unquantized gains and is overwritten with the quantized ones (the
/// reference's in-place contract). `prev_ind` threads the persistent
/// `LastGainIndex` state. Hysteresis rounds towards the previous
/// quantized gain, and the double-step delta mapping of
/// [`gains_dequant`] is mirrored exactly.
pub(crate) fn gains_quant(
    ind: &mut [i8; MAX_NB_SUBFR],
    gain_q16: &mut [i32; MAX_NB_SUBFR],
    prev_ind: &mut i8,
    conditional: bool,
    nb_subfr: usize,
) {
    let mut prev = i32::from(*prev_ind);
    for k in 0..nb_subfr {
        /* Convert to log scale, scale, floor() */
        let mut ix = smulwb(SCALE_Q16, lin2log(gain_q16[k]) - OFFSET);

        /* Round towards previous quantized gain (hysteresis) */
        if ix < prev {
            ix += 1;
        }
        ix = ix.clamp(0, N_LEVELS_QGAIN - 1);

        /* Compute delta indices and limit */
        if k == 0 && !conditional {
            /* Full index */
            ix = ix.clamp(prev + MIN_DELTA_GAIN_QUANT, N_LEVELS_QGAIN - 1);
            prev = ix;
        } else {
            /* Delta index */
            ix -= prev;

            /* Double the quantization step size for large gain increases,
             * so that the max gain level can be reached */
            let double_step_size_threshold = 2 * MAX_DELTA_GAIN_QUANT - N_LEVELS_QGAIN + prev;
            if ix > double_step_size_threshold {
                ix = double_step_size_threshold + ((ix - double_step_size_threshold + 1) >> 1);
            }
            ix = ix.clamp(MIN_DELTA_GAIN_QUANT, MAX_DELTA_GAIN_QUANT);

            /* Accumulate deltas */
            if ix > double_step_size_threshold {
                prev = (prev + ix * 2 - double_step_size_threshold).min(N_LEVELS_QGAIN - 1);
            } else {
                prev += ix;
            }

            /* Shift to make non-negative */
            ix -= MIN_DELTA_GAIN_QUANT;
        }

        ind[k] = ix as i8;
        /* Scale and convert to linear scale */
        gain_q16[k] = log2lin((smulwb(INV_SCALE_Q16, prev) + OFFSET).min(LOG_GAIN_MAX_Q7));
    }
    *prev_ind = prev as i8;
}
