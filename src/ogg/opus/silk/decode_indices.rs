// Adapted from TPT Cadence and the BSD-licensed Opus reference; see ../NOTICE.md.
//! SILK per-frame side-information indices (RFC 6716 §4.2.4).
//!
//! Ports `silk/decode_indices.c`, plus the VAD/LBRR flag prologue that
//! the reference performs once per payload in `silk/dec_API.c` (it is
//! kept here because it is part of the same side-info bitstream layout).
//!
//! Bitstream order per frame:
//! 1. frame type + quantization offset (one ICDF symbol, table chosen by
//!    VAD flag / LBRR),
//! 2. gain indices (subframe 0 conditionally delta-coded against the
//!    previous frame when `condCoding == CODE_CONDITIONALLY`, otherwise
//!    MSB stage + 3 uniform LSBs; remaining subframes always delta),
//! 3. NLSF indices (stage-1 vector, then `LPC_order` stage-2 residuals
//!    with escape extension at both ends),
//! 4. NLSF interpolation factor (20 ms frames only),
//! 5. for voiced frames only: pitch lag (absolute, or delta against the
//!    previous frame when allowed), pitch contour, LTP filter index +
//!    per-subframe LTP gain indices, LTP scale (independent coding only),
//! 6. a uniform 2-bit seed.
//!
//! Fields not touched for a frame type (e.g. pitch fields of an
//! unvoiced frame) keep the previous frame's values, matching the
//! reference, where `indices` lives in the persistent channel state.
//!
//! SOURCE: Xiph.Org libopus 1.5.2, `silk/decode_indices.c`,
//! `silk/dec_API.c`, `silk/NLSF_unpack.c`, `silk/decoder_set_fs.c`,
//! `silk/define.h`, `silk/structs.h` (BSD-3-Clause).
#![allow(dead_code)]

use crate::ogg::opus::range::RangeDecoder;
use crate::ogg::opus::silk::tables::{
    DELTA_GAIN_ICDF, GAIN_ICDF, LBRR_FLAGS_ICDF_PTR, LTP_GAIN_ICDF_PTRS, LTP_PER_INDEX_ICDF,
    LTPSCALE_ICDF, NLSF_EXT_ICDF, NLSF_INTERPOLATION_FACTOR_ICDF, NlsfCbStruct,
    PITCH_CONTOUR_10_MS_ICDF, PITCH_CONTOUR_10_MS_NB_ICDF, PITCH_CONTOUR_ICDF,
    PITCH_CONTOUR_NB_ICDF, PITCH_DELTA_ICDF, PITCH_LAG_ICDF, TYPE_OFFSET_NO_VAD_ICDF,
    TYPE_OFFSET_VAD_ICDF, UNIFORM4_ICDF, UNIFORM6_ICDF, UNIFORM8_ICDF,
};
use crate::ogg::opus::{CadenceError, Result};

/// `TYPE_NO_VOICE_ACTIVITY` (`silk/define.h`).
pub(crate) const TYPE_NO_VOICE_ACTIVITY: i8 = 0;
/// `TYPE_UNVOICED` (`silk/define.h`).
pub(crate) const TYPE_UNVOICED: i8 = 1;
/// `TYPE_VOICED` (`silk/define.h`).
pub(crate) const TYPE_VOICED: i8 = 2;

/// `MAX_NB_SUBFR`: 4 × 5 ms subframes in a 20 ms frame.
pub(crate) const MAX_NB_SUBFR: usize = 4;
/// `MAX_LPC_ORDER`.
pub(crate) const MAX_LPC_ORDER: usize = 16;
/// `NLSF_QUANT_MAX_AMPLITUDE`.
pub(crate) const NLSF_QUANT_MAX_AMPLITUDE: i32 = 4;
/// Maximum number of SILK frames per packet (20 ms each, so up to 60 ms).
pub(crate) const MAX_FRAMES_PER_PACKET: usize = 3;

/// The type of conditional coding in effect for a frame — mirrors
/// `CODE_INDEPENDENTLY`, `CODE_INDEPENDENTLY_NO_LTP_SCALING`, and
/// `CODE_CONDITIONALLY` (`silk/define.h`). The distinction matters for
/// voiced frames: only [`CondCoding::Independently`] codes the
/// LTP-scale symbol (`silk/decode_indices.c`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CondCoding {
    /// `CODE_INDEPENDENTLY`: full side info; the LTP-scale symbol is
    /// coded for voiced frames.
    Independently,
    /// `CODE_INDEPENDENTLY_NO_LTP_SCALING`: like `Independently` except
    /// the LTP-scale symbol is not coded (the side channel after a
    /// skipped side frame needs no LTP scaling).
    IndependentlyNoLtpScaling,
    /// `CODE_CONDITIONALLY`: delta-coded side info.
    Conditionally,
}

/// Decoded per-frame side information — mirrors `SideInfoIndices`
/// (`silk/structs.h`).
///
/// # PROVISIONAL (Tier 1 contract)
///
/// This struct is published early so the Tier 2 modules (`nlsf.rs`,
/// `gains.rs`, `pitch.rs`, `excitation.rs`) can code against it without
/// waiting for this module's bitstream handling to be finalized. Field
/// names map 1:1 onto the reference struct and are not expected to
/// change; field *types* may still be adjusted (e.g. if a Tier 2 module
/// wants wider intermediates), and Tier 4 may add derived accessors.
/// Code against the field set now, but treat the definition as
/// not-yet-frozen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SideInfoIndices {
    /// Quantized gain indices, one per subframe (0–63 independently,
    /// 0–39 delta).
    pub gains_indices: [i8; MAX_NB_SUBFR],
    /// LTP gain codebook index per subframe (into
    /// `LTP_GAIN_ICDF_PTRS[per_index]`).
    pub ltp_index: [i8; MAX_NB_SUBFR],
    /// NLSF stage-1 codebook index (`[0]`) followed by `LPC_order`
    /// stage-2 residuals (each in `-10..=10`).
    pub nlsf_indices: [i8; MAX_LPC_ORDER + 1],
    /// Absolute pitch lag index.
    pub lag_index: i16,
    /// Pitch contour codebook index.
    pub contour_index: i8,
    /// `TYPE_NO_VOICE_ACTIVITY` / `TYPE_UNVOICED` / `TYPE_VOICED`.
    pub signal_type: i8,
    /// Quantization offset type (0 or 1) — selects the row of
    /// `QUANTIZATION_OFFSETS_Q10`.
    pub quant_offset_type: i8,
    /// NLSF interframe interpolation factor in Q2 (0–4; forced 4 for
    /// 10 ms frames, which carry no symbol).
    pub nlsf_interp_coef_q2: i8,
    /// Periodicity (LTP codebook) index, 0–2.
    pub per_index: i8,
    /// LTP scaling index, 0–2 (into `LTPSCALES_TABLE_Q14`).
    pub ltp_scale_index: i8,
    /// Uniform random seed for excitation dithering, 0–3.
    pub seed: i8,
}

/// Cross-frame decoder state that the side-info decode mutates — the
/// `ec_prevSignalType` / `ec_prevLagIndex` pair of
/// `silk_decoder_state`. The Tier 4 `SilkDecoder` will hold one per
/// channel and reset it to `Default` on init/reset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EcPrevState {
    /// Signal type of the previously decoded frame.
    pub ec_prev_signal_type: i8,
    /// Pitch lag index of the previously decoded voiced frame.
    pub ec_prev_lag_index: i16,
}

/// Everything [`decode_indices`] reads from the decoder context, other
/// than the range decoder and the cross-frame [`EcPrevState`].
pub(crate) struct FrameParams {
    /// NLSF codebook: `&NLSF_CB_NB_MB` at 8/12 kHz, `&NLSF_CB_WB` at
    /// 16 kHz (`silk_decoder_set_fs`).
    pub nlsf_cb: &'static NlsfCbStruct,
    /// Internal sample rate in kHz: 8, 12, or 16 (SILK supports no
    /// other internal rates).
    pub fs_khz: u32,
    /// Subframes per frame: 4 (20 ms) or 2 (10 ms).
    pub nb_subfr: usize,
    /// Frame number within the packet (0-based).
    pub frame_index: usize,
    /// `psDec->VAD_flags[frame_index]`.
    pub vad_flag: bool,
    /// True when decoding an LBRR frame rather than a regular one.
    pub decode_lbrr: bool,
    /// Conditional-coding mode for this frame.
    pub cond_coding: CondCoding,
}

/// Decodes the per-frame side-information indices from the bitstream —
/// a direct port of `silk_decode_indices`.
///
/// The frame's VAD flag comes from [`decode_vad_flags_and_lbrr_flag`],
/// which the caller must have run once per payload before the frame
/// loop.
pub(crate) fn decode_indices(
    dec: &mut RangeDecoder<'_>,
    indices: &mut SideInfoIndices,
    ec_state: &mut EcPrevState,
    params: &FrameParams,
) -> Result<()> {
    debug_assert!(params.nb_subfr == MAX_NB_SUBFR || params.nb_subfr == MAX_NB_SUBFR / 2);
    let order = params.nlsf_cb.order as usize;

    /*******************************************/
    /* Decode signal type and quantizer offset */
    /*******************************************/
    let ix = if params.decode_lbrr || params.vad_flag {
        dec.decode_icdf(&TYPE_OFFSET_VAD_ICDF, 8)? as i32 + 2
    } else {
        dec.decode_icdf(&TYPE_OFFSET_NO_VAD_ICDF, 8)? as i32
    };
    indices.signal_type = (ix >> 1) as i8;
    indices.quant_offset_type = (ix & 1) as i8;

    /****************/
    /* Decode gains */
    /****************/
    /* First subframe */
    if params.cond_coding == CondCoding::Conditionally {
        /* Conditional coding */
        indices.gains_indices[0] = dec.decode_icdf(&DELTA_GAIN_ICDF, 8)? as i8;
    } else {
        /* Independent coding, in two stages: MSB bits followed by 3 LSBs */
        let msb = dec.decode_icdf(&GAIN_ICDF[indices.signal_type as usize], 8)? as i32;
        let lsb = dec.decode_icdf(&UNIFORM8_ICDF, 8)? as i32;
        indices.gains_indices[0] = ((msb << 3) + lsb) as i8;
    }

    /* Remaining subframes */
    for g in indices
        .gains_indices
        .iter_mut()
        .take(params.nb_subfr)
        .skip(1)
    {
        *g = dec.decode_icdf(&DELTA_GAIN_ICDF, 8)? as i8;
    }

    /**********************/
    /* Decode LSF Indices */
    /**********************/
    let cb1_index = dec.decode_icdf(
        &params.nlsf_cb.cb1_icdf
            [(indices.signal_type as usize >> 1) * params.nlsf_cb.n_vectors as usize..],
        8,
    )? as usize;
    indices.nlsf_indices[0] = cb1_index as i8;
    let (ec_ix, _pred_q8) = nlsf_unpack(params.nlsf_cb, cb1_index);
    for (i, &table_offset) in ec_ix.iter().enumerate().take(order) {
        let mut ix = dec.decode_icdf(&params.nlsf_cb.ec_icdf[table_offset..], 8)? as i32;
        if ix == 0 {
            ix -= dec.decode_icdf(&NLSF_EXT_ICDF, 8)? as i32;
        } else if ix == 2 * NLSF_QUANT_MAX_AMPLITUDE {
            ix += dec.decode_icdf(&NLSF_EXT_ICDF, 8)? as i32;
        }
        indices.nlsf_indices[i + 1] = (ix - NLSF_QUANT_MAX_AMPLITUDE) as i8;
    }

    /* Decode LSF interpolation factor */
    if params.nb_subfr == MAX_NB_SUBFR {
        indices.nlsf_interp_coef_q2 = dec.decode_icdf(&NLSF_INTERPOLATION_FACTOR_ICDF, 8)? as i8;
    } else {
        indices.nlsf_interp_coef_q2 = 4;
    }

    if indices.signal_type == TYPE_VOICED {
        /*********************/
        /* Decode pitch lags */
        /*********************/
        /* Get lag index */
        let mut decode_absolute_lag_index = true;
        if params.cond_coding == CondCoding::Conditionally
            && ec_state.ec_prev_signal_type == TYPE_VOICED
        {
            /* Decode Delta index */
            let delta_lag_index = dec.decode_icdf(&PITCH_DELTA_ICDF, 8)? as i32;
            if delta_lag_index > 0 {
                indices.lag_index =
                    (ec_state.ec_prev_lag_index as i32 + delta_lag_index - 9) as i16;
                decode_absolute_lag_index = false;
            }
        }
        if decode_absolute_lag_index {
            /* Absolute decoding: coarse stage scaled by fs_kHz/2, then
             * the uniformly-coded low bits. */
            let low_bits_icdf = pitch_lag_low_bits_icdf(params.fs_khz)?;
            indices.lag_index = (dec.decode_icdf(&PITCH_LAG_ICDF, 8)? as i32
                * (params.fs_khz as i32 >> 1)
                + dec.decode_icdf(low_bits_icdf, 8)? as i32) as i16;
        }
        ec_state.ec_prev_lag_index = indices.lag_index;

        /* Get contour index */
        let contour_icdf = pitch_contour_icdf(params.fs_khz, params.nb_subfr)?;
        indices.contour_index = dec.decode_icdf(contour_icdf, 8)? as i8;

        /********************/
        /* Decode LTP gains */
        /********************/
        /* Decode PERIndex value */
        indices.per_index = dec.decode_icdf(&LTP_PER_INDEX_ICDF, 8)? as i8;

        for ltp in indices.ltp_index.iter_mut().take(params.nb_subfr) {
            *ltp = dec.decode_icdf(LTP_GAIN_ICDF_PTRS[indices.per_index as usize], 8)? as i8;
        }

        /**********************/
        /* Decode LTP scaling */
        /**********************/
        if params.cond_coding == CondCoding::Independently {
            indices.ltp_scale_index = dec.decode_icdf(&LTPSCALE_ICDF, 8)? as i8;
        } else {
            indices.ltp_scale_index = 0;
        }
    }
    ec_state.ec_prev_signal_type = indices.signal_type;

    /***************/
    /* Decode seed */
    /***************/
    indices.seed = dec.decode_icdf(&UNIFORM4_ICDF, 8)? as i8;
    Ok(())
}

/// Decodes the per-frame VAD flags and the packet-level LBRR flag —
/// performed once per payload before the frame loop (the "Decode VAD
/// flags and LBRR flag" block of `silk_Decode` in `dec_API.c`). Each is
/// a single bit at probability 1/2. Returns the flags for frames
/// `0..n_frames` plus the packet-level LBRR flag.
pub(crate) fn decode_vad_flags_and_lbrr_flag(
    dec: &mut RangeDecoder<'_>,
    n_frames: usize,
) -> Result<([bool; MAX_FRAMES_PER_PACKET], bool)> {
    debug_assert!((1..=MAX_FRAMES_PER_PACKET).contains(&n_frames));
    let mut vad_flags = [false; MAX_FRAMES_PER_PACKET];
    for flag in vad_flags.iter_mut().take(n_frames) {
        *flag = dec.decode_bit_logp(1)?;
    }
    let lbrr_flag = dec.decode_bit_logp(1)?;
    Ok((vad_flags, lbrr_flag))
}

/// Decodes the per-frame LBRR flags of a packet whose packet-level LBRR
/// flag is set — the "Decode LBRR flags" block of `silk_Decode`. With a
/// single frame the flag is implicitly 1 (no bits are read); otherwise
/// one ICDF symbol encodes the whole pattern: bit `i` of `symbol + 1`
/// is frame `i`'s flag.
pub(crate) fn decode_lbrr_flags(
    dec: &mut RangeDecoder<'_>,
    n_frames: usize,
) -> Result<[bool; MAX_FRAMES_PER_PACKET]> {
    debug_assert!((1..=MAX_FRAMES_PER_PACKET).contains(&n_frames));
    let mut flags = [false; MAX_FRAMES_PER_PACKET];
    if n_frames == 1 {
        flags[0] = true;
    } else {
        let lbrr_symbol = dec.decode_icdf(LBRR_FLAGS_ICDF_PTR[n_frames - 2], 8)? + 1;
        for (i, flag) in flags.iter_mut().take(n_frames).enumerate() {
            *flag = (lbrr_symbol >> i) & 1 != 0;
        }
    }
    Ok(flags)
}

/// `psDec->pitch_lag_low_bits_iCDF` (`silk_decoder_set_fs`): the
/// uniformly-coded low bits of the absolute pitch lag index.
fn pitch_lag_low_bits_icdf(fs_khz: u32) -> Result<&'static [u8]> {
    match fs_khz {
        8 => Ok(&UNIFORM4_ICDF),
        12 => Ok(&UNIFORM6_ICDF),
        16 => Ok(&UNIFORM8_ICDF),
        _ => Err(CadenceError::CorruptData(format!(
            "unsupported SILK internal sample rate {fs_khz} kHz"
        ))),
    }
}

/// `psDec->pitch_contour_iCDF` (`silk_decoder_set_fs`): the pitch
/// contour codebook depends on bandwidth and frame size.
fn pitch_contour_icdf(fs_khz: u32, nb_subfr: usize) -> Result<&'static [u8]> {
    match (fs_khz, nb_subfr == MAX_NB_SUBFR) {
        (8, true) => Ok(&PITCH_CONTOUR_NB_ICDF),
        (8, false) => Ok(&PITCH_CONTOUR_10_MS_NB_ICDF),
        (_, true) => Ok(&PITCH_CONTOUR_ICDF),
        (_, false) => Ok(&PITCH_CONTOUR_10_MS_ICDF),
    }
}

/// Unpacks the per-coefficient entropy-table offsets and prediction
/// weights for a stage-1 NLSF codebook index — a direct port of
/// `silk_NLSF_unpack`. Only the first `order` entries of the returned
/// arrays are valid; `nlsf.rs` (Tier 2) reuses this for NLSF
/// reconstruction.
///
/// Returns `(ec_ix, pred_Q8)`, where `ec_ix[i]` indexes into
/// `NlsfCbStruct::ec_icdf` and `pred_Q8[i]` is the backward-prediction
/// coefficient for coefficient `i`.
pub(crate) fn nlsf_unpack(
    cb: &NlsfCbStruct,
    cb1_index: usize,
) -> ([usize; MAX_LPC_ORDER], [u8; MAX_LPC_ORDER]) {
    debug_assert!(cb1_index < cb.n_vectors as usize);
    let order = cb.order as usize;
    let mut ec_ix = [0usize; MAX_LPC_ORDER];
    let mut pred_q8 = [0u8; MAX_LPC_ORDER];
    let ec_sel = &cb.ec_sel[cb1_index * order / 2..];
    for i in (0..order).step_by(2) {
        let entry = ec_sel[i / 2];
        ec_ix[i] = ((entry >> 1) & 7) as usize * (2 * NLSF_QUANT_MAX_AMPLITUDE as usize + 1);
        pred_q8[i] = cb.pred_q8[i + (entry & 1) as usize * (order - 1)];
        ec_ix[i + 1] = ((entry >> 5) & 7) as usize * (2 * NLSF_QUANT_MAX_AMPLITUDE as usize + 1);
        pred_q8[i + 1] = cb.pred_q8[i + ((entry >> 4) & 1) as usize * (order - 1) + 1];
    }
    (ec_ix, pred_q8)
}
