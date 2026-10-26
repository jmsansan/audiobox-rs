// Adapted from TPT Cadence and the BSD-licensed Opus reference; see ../NOTICE.md.
//! SILK pitch lag reconstruction and LTP filter-codebook lookup
//! (RFC 6716 §4.2.7.6).
//!
//! Ports `silk/decode_pitch.c` (`silk_decode_pitch`) plus the voiced
//! branch of `silk/decode_parameters.c` that turns the per-subframe LTP
//! codebook indices and the LTP scale index into the Q14 values the
//! synthesis consumes. The *bitstream* side of the pitch parameters —
//! the absolute/relative (delta) primary lag index and the contour,
//! periodicity, and per-subframe LTP index symbols — is entropy-decoded
//! by [`super::decode_indices`] into the `lag_index`, `contour_index`,
//! `per_index`, `ltp_index`, and `ltp_scale_index` fields of
//! `SideInfoIndices`; this module owns the reconstruction:
//!
//! - **Primary lag → per-subframe lags.** The primary lag index counts
//!   samples above `2 ms · fs_kHz` (the encoder's pitch search floor,
//!   `PE_MIN_LAG_MS`); it was assembled by `decode_indices` as
//!   `lag_high * fs_kHz/2 + lag_low` (absolute) or
//!   `prev_lag_index + delta − 9` (relative). Each subframe's lag is
//!   the primary lag plus a small VQ-contour offset
//!   (`CB_LAGS_STAGE*`), clamped to the encoder's 2–18 ms search range
//!   — RFC 6716 §4.2.7.6.1, Tables 33–36.
//! - **LTP filter coefficients.** Each voiced subframe uses a 5-tap
//!   pitch predictor whose taps come from one of three codebooks
//!   selected by the periodicity index (`LTP_VQ_PTRS_Q7`, RFC Tables
//!   39–41). The Q7 taps are widened to Q14 by `<< 7`
//!   (`silk/decode_parameters.c`).
//! - **LTP scaling.** The Q14 scale factor (RFC §4.2.7.6.3, values
//!   15565/12288/8192 ≈ 0.95/0.75/0.5) that dampens the long-term
//!   prediction to trade prediction gain against packet-loss recovery.
//!
//! For unvoiced frames the caller (Tier 4's `decode_parameters` port)
//! zeroes the lag/LTP arrays instead, matching the reference's `else`
//! branch — nothing here needs to run.
//!
//! SOURCE: Xiph.Org libopus 1.5.2, `silk/decode_pitch.c`,
//! `silk/decode_parameters.c`, `silk/pitch_est_defines.h`,
//! `silk/define.h` (BSD-3-Clause).
#![allow(dead_code)]

use crate::ogg::opus::silk::decode_indices::MAX_NB_SUBFR;
use crate::ogg::opus::silk::sigproc::smulbb;
use crate::ogg::opus::silk::tables::{
    CB_LAGS_STAGE2, CB_LAGS_STAGE2_10_MS, CB_LAGS_STAGE3, CB_LAGS_STAGE3_10_MS, LTP_VQ_PTRS_Q7,
    LTPSCALES_TABLE_Q14,
};
use crate::ogg::opus::{CadenceError, Result};

/// `LTP_ORDER` (`silk/define.h`): taps per subframe's long-term
/// predictor.
pub(crate) const LTP_ORDER: usize = 5;

/// `PE_MIN_LAG_MS` / `PE_MAX_LAG_MS` (`silk/pitch_est_defines.h`): the
/// encoder's pitch search range in ms — 500 Hz down to 55.6 Hz. The
/// primary lag index counts samples above `PE_MIN_LAG_MS · fs_kHz` and
/// the reconstructed subframe lags are clamped to
/// `[PE_MIN_LAG_MS · fs_kHz, PE_MAX_LAG_MS · fs_kHz]`.
const PE_MIN_LAG_MS: i32 = 2;
const PE_MAX_LAG_MS: i32 = 18;

/// Reconstructs the per-subframe pitch lags (in samples at the internal
/// rate) from the primary lag index and the contour codebook index — a
/// direct port of `silk_decode_pitch`.
///
/// Writes exactly `nb_subfr` entries of `pitch_lags` (2 for 10 ms
/// frames, 4 for 20 ms); the remaining entries are left untouched,
/// matching the reference's caller-owned array.
///
/// `fs_kHz` selects NB's stage-2 contour codebooks (11 offsets for 20 ms
/// frames, 3 for 10 ms) versus MB/WB's stage-3 ones (34/12); the same
/// selection drove the contour ICDF `decode_indices` decoded
/// `contour_index` from, so a structurally consistent call always has
/// `contour_index` in range (asserted in debug builds).
pub(crate) fn decode_pitch(
    lag_index: i16,
    contour_index: i8,
    pitch_lags: &mut [i32; MAX_NB_SUBFR],
    fs_khz: u32,
    nb_subfr: usize,
) -> Result<()> {
    debug_assert!(nb_subfr == MAX_NB_SUBFR || nb_subfr == MAX_NB_SUBFR / 2);
    if !matches!(fs_khz, 8 | 12 | 16) {
        return Err(CadenceError::CorruptData(format!(
            "unsupported SILK internal sample rate {fs_khz} kHz"
        )));
    }
    let cbk_size = if fs_khz == 8 {
        if nb_subfr == MAX_NB_SUBFR {
            CB_LAGS_STAGE2[0].len() // PE_NB_CBKS_STAGE2_EXT
        } else {
            CB_LAGS_STAGE2_10_MS[0].len() // PE_NB_CBKS_STAGE2_10MS
        }
    } else if nb_subfr == MAX_NB_SUBFR {
        CB_LAGS_STAGE3[0].len() // PE_NB_CBKS_STAGE3_MAX
    } else {
        CB_LAGS_STAGE3_10_MS[0].len() // PE_NB_CBKS_STAGE3_10MS
    };
    let ci = contour_index as usize;
    debug_assert!(ci < cbk_size, "contour index {ci} out of codebook range");

    let min_lag = smulbb(PE_MIN_LAG_MS, fs_khz as i32);
    let max_lag = smulbb(PE_MAX_LAG_MS, fs_khz as i32);
    let lag = min_lag + i32::from(lag_index);

    for (k, out) in pitch_lags.iter_mut().enumerate().take(nb_subfr) {
        /* matrix_ptr(Lag_CB_ptr, k, contourIndex, cbk_size): row k,
         * column contourIndex of the reference's row-major [nb_subfr]
         * x [cbk_size] codebook. */
        let offset = if fs_khz == 8 {
            if nb_subfr == MAX_NB_SUBFR {
                CB_LAGS_STAGE2[k][ci]
            } else {
                CB_LAGS_STAGE2_10_MS[k][ci]
            }
        } else if nb_subfr == MAX_NB_SUBFR {
            CB_LAGS_STAGE3[k][ci]
        } else {
            CB_LAGS_STAGE3_10_MS[k][ci]
        };
        *out = (lag + i32::from(offset)).clamp(min_lag, max_lag);
    }
    Ok(())
}

/// Looks up the voiced subframes' 5-tap LTP predictor coefficients in
/// the Q7 codebook selected by the periodicity index and widens them to
/// Q14 — the "Decode Codebook Index" block of the voiced branch of
/// `silk_decode_parameters` (`silk/decode_parameters.c`).
///
/// Writes exactly `nb_subfr * [`LTP_ORDER`]` entries of `ltp_coefs_q14`
/// and leaves the tail untouched, mirroring the reference's in-place
/// `psDecCtrl->LTPCoef_Q14` (the unvoiced branch zeroes the same span).
/// `ltp_index[k]` must be within codebook `per_index` (guaranteed by
/// `decode_indices`' ICDF table choice; asserted in debug builds).
pub(crate) fn ltp_coefs_q14(
    per_index: i8,
    ltp_index: &[i8; MAX_NB_SUBFR],
    ltp_coefs_q14: &mut [i16; MAX_NB_SUBFR * LTP_ORDER],
    nb_subfr: usize,
) {
    debug_assert!(nb_subfr == MAX_NB_SUBFR || nb_subfr == MAX_NB_SUBFR / 2);
    debug_assert!((0..LTP_VQ_PTRS_Q7.len() as i8).contains(&per_index));
    let cbk_q7 = &LTP_VQ_PTRS_Q7[per_index as usize];
    for k in 0..nb_subfr {
        let ix = ltp_index[k] as usize;
        debug_assert!(ix < cbk_q7.len(), "LTP index {ix} out of codebook range");
        for (i, coef) in ltp_coefs_q14[k * LTP_ORDER..(k + 1) * LTP_ORDER]
            .iter_mut()
            .enumerate()
        {
            /* silk_LSHIFT(cbk_ptr_Q7[...], 7): Q7 taps to Q14. The tap
             * range (-128..=127) maps into i16 without saturation. */
            *coef = ((cbk_q7[ix][i] as i32) << 7) as i16;
        }
    }
}

/// The voiced frame's LTP scale factor in Q14 — the "Decode LTP
/// scaling" block of `silk_decode_parameters`
/// (`silk_LTPScales_table_Q14`); RFC 6716 §4.2.7.6.3's 15565, 12288,
/// 8192. `decode_indices` stores index 0 (≈ 0.95) when the parameter is
/// not coded.
#[inline]
pub(crate) fn ltp_scale_q14(ltp_scale_index: i8) -> i16 {
    LTPSCALES_TABLE_Q14[ltp_scale_index as usize]
}
