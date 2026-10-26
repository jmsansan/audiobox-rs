// Adapted from TPT Cadence and the BSD-licensed Opus reference; see ../NOTICE.md.
//! SILK excitation decoding (RFC 6716 §4.2.7.8.2–§4.2.7.8.5).
//!
//! Ports the quantized-excitation bitstream layer of the reference
//! decoder:
//!
//! - [`decode_pulses`] (`silk/decode_pulses.c`): the per-block pulse
//!   counts (rate level + per-block count with the `SILK_MAX_PULSES + 1`
//!   LSB-shift escape), the shell-coded pulse positions per block of
//!   [`SHELL_CODEC_FRAME_LENGTH`] samples ([`shell_decoder`],
//!   `silk/shell_coder.c`), the LSB refinement bits, and the sign bits
//!   ([`decode_signs`], `silk/code_signs.c`).
//! - [`reconstruct_excitation`]: the "Decode excitation" block at the
//!   top of `silk/decode_core.c` — the ±`QUANT_LEVEL_ADJUST_Q10` pull
//!   toward zero, the quantization offset, and the per-sample sign
//!   dither from the linear congruential generator seeded by the frame
//!   seed (RFC 6716 §4.2.7.7/§4.2.7.8.5).
//!
//! The output of [`decode_pulses`] is the signed quantization index
//! `q[n]` ("pulses"); [`reconstruct_excitation`] turns that into the
//! Q14 excitation `exc_Q14[n]` the LPC/LTP synthesis consumes.
//!
//! SOURCE: Xiph.Org libopus 1.5.2, `silk/decode_pulses.c`,
//! `silk/shell_coder.c`, `silk/code_signs.c`, `silk/decode_core.c`,
//! `silk/encode_pulses.c` (round-trip reference), `silk/define.h`,
//! `silk/SigProc_FIX.h` (BSD-3-Clause); cross-checked against
//! RFC 6716 §4.2.7.4–§4.2.7.8.
#![allow(dead_code)]

use crate::ogg::opus::Result;
use crate::ogg::opus::range::RangeDecoder;
use crate::ogg::opus::silk::tables::{
    LSB_ICDF, PULSES_PER_BLOCK_ICDF, QUANTIZATION_OFFSETS_Q10, RATE_LEVELS_ICDF,
    SHELL_CODE_TABLE_OFFSETS, SHELL_CODE_TABLE0, SHELL_CODE_TABLE1, SHELL_CODE_TABLE2,
    SHELL_CODE_TABLE3, SIGN_ICDF,
};

/// `SHELL_CODEC_FRAME_LENGTH`: samples per shell-coded block
/// (`silk/define.h`).
pub(crate) const SHELL_CODEC_FRAME_LENGTH: usize = 16;
/// `LOG2_SHELL_CODEC_FRAME_LENGTH`.
pub(crate) const LOG2_SHELL_CODEC_FRAME_LENGTH: u32 = 4;
/// `MAX_NB_SHELL_BLOCKS = MAX_FRAME_LENGTH / SHELL_CODEC_FRAME_LENGTH`
/// (320 samples / 16).
pub(crate) const MAX_NB_SHELL_BLOCKS: usize = 20;
/// `N_RATE_LEVELS` (`silk/define.h`).
pub(crate) const N_RATE_LEVELS: usize = 10;
/// `SILK_MAX_PULSES` (`silk/define.h`): largest per-block pulse count
/// the shell code carries directly; the count `SILK_MAX_PULSES + 1` is
/// the LSB-shift escape.
pub(crate) const SILK_MAX_PULSES: i32 = 16;
/// `QUANT_LEVEL_ADJUST_Q10` (`silk/define.h`): the decoder-side pull of
/// nonzero excitation values toward zero, in Q10.
pub(crate) const QUANT_LEVEL_ADJUST_Q10: i32 = 80;
/// `RAND_MULTIPLIER` (`silk/SigProc_FIX.h`).
const RAND_MULTIPLIER: i32 = 196314165;
/// `RAND_INCREMENT` (`silk/SigProc_FIX.h`).
const RAND_INCREMENT: i32 = 907633515;

/// Shell decoder for one block of 16 pulses — port of
/// `silk_shell_decoder` (`silk/shell_coder.c`).
///
/// Reconstructs the 16 nonnegative per-sample pulse amplitudes from a
/// binary pulse-count tree: `pulses4` is the block's total pulse count
/// (1..=16; the caller skips zero-count blocks), and each
/// [`decode_split`] divides a node's count between its two children
/// with one ICDF symbol whose table row is selected by the node's
/// count. `pulses0` must be at least [`SHELL_CODEC_FRAME_LENGTH`] long
/// and is fully overwritten.
pub(crate) fn shell_decoder(
    dec: &mut RangeDecoder<'_>,
    pulses0: &mut [i16],
    pulses4: i32,
) -> Result<()> {
    debug_assert!(pulses0.len() >= SHELL_CODEC_FRAME_LENGTH);
    debug_assert!((1..=SILK_MAX_PULSES).contains(&pulses4));

    /* Split order matches the reference exactly; it fixes the order in
     * which ICDF symbols are consumed. */
    let (p3_0, p3_1) = decode_split(dec, &SHELL_CODE_TABLE3, pulses4)?;
    let (p2_0, p2_1) = decode_split(dec, &SHELL_CODE_TABLE2, p3_0 as i32)?;
    let (p1_0, p1_1) = decode_split(dec, &SHELL_CODE_TABLE1, p2_0 as i32)?;
    let (p0_0, p0_1) = decode_split(dec, &SHELL_CODE_TABLE0, p1_0 as i32)?;
    pulses0[0] = p0_0;
    pulses0[1] = p0_1;
    let (p0_2, p0_3) = decode_split(dec, &SHELL_CODE_TABLE0, p1_1 as i32)?;
    pulses0[2] = p0_2;
    pulses0[3] = p0_3;

    let (p1_2, p1_3) = decode_split(dec, &SHELL_CODE_TABLE1, p2_1 as i32)?;
    let (p0_4, p0_5) = decode_split(dec, &SHELL_CODE_TABLE0, p1_2 as i32)?;
    pulses0[4] = p0_4;
    pulses0[5] = p0_5;
    let (p0_6, p0_7) = decode_split(dec, &SHELL_CODE_TABLE0, p1_3 as i32)?;
    pulses0[6] = p0_6;
    pulses0[7] = p0_7;

    let (p2_2, p2_3) = decode_split(dec, &SHELL_CODE_TABLE2, p3_1 as i32)?;

    let (p1_4, p1_5) = decode_split(dec, &SHELL_CODE_TABLE1, p2_2 as i32)?;
    let (p0_8, p0_9) = decode_split(dec, &SHELL_CODE_TABLE0, p1_4 as i32)?;
    pulses0[8] = p0_8;
    pulses0[9] = p0_9;
    let (p0_10, p0_11) = decode_split(dec, &SHELL_CODE_TABLE0, p1_5 as i32)?;
    pulses0[10] = p0_10;
    pulses0[11] = p0_11;

    let (p1_6, p1_7) = decode_split(dec, &SHELL_CODE_TABLE1, p2_3 as i32)?;
    let (p0_12, p0_13) = decode_split(dec, &SHELL_CODE_TABLE0, p1_6 as i32)?;
    pulses0[12] = p0_12;
    pulses0[13] = p0_13;
    let (p0_14, p0_15) = decode_split(dec, &SHELL_CODE_TABLE0, p1_7 as i32)?;
    pulses0[14] = p0_14;
    pulses0[15] = p0_15;
    Ok(())
}

/// `decode_split` (`silk/shell_coder.c`): divides `p` pulses between
/// two children by decoding the first child's amplitude from the table
/// row for count `p`; a zero-count node splits trivially.
fn decode_split(dec: &mut RangeDecoder<'_>, shell_table: &[u8], p: i32) -> Result<(i16, i16)> {
    if p > 0 {
        let offset = SHELL_CODE_TABLE_OFFSETS[p as usize] as usize;
        let child1 = dec.decode_icdf(&shell_table[offset..], 8)? as i16;
        Ok((child1, (p - child1 as i32) as i16))
    } else {
        Ok((0, 0))
    }
}

/// Decodes the quantization indices of the excitation signal — port of
/// `silk_decode_pulses` (`silk/decode_pulses.c`).
///
/// Writes the signed quantization indices into `pulses`, which must be
/// at least `ceil(frame_length / 16) * 16` long (the reference operates
/// on `MAX_FRAME_LENGTH` buffers; a 10 ms @ 12 kHz frame's final
/// partial block still fills a whole 16-sample block, and every entry
/// in the padded region is always written).
///
/// Bitstream order: one rate-level symbol, then per block one
/// pulse-count symbol (with the `SILK_MAX_PULSES + 1` escape repeated
/// per LSB shift, reading the last rate level's table — offset by one
/// entry from the 10th shift on, which removes the escape from that
/// table), then the per-block shell codes, then the LSB bits, then the
/// sign bits.
pub(crate) fn decode_pulses(
    dec: &mut RangeDecoder<'_>,
    pulses: &mut [i16],
    signal_type: i32,
    quant_offset_type: i32,
    frame_length: usize,
) -> Result<()> {
    /*********************/
    /* Decode rate level */
    /*********************/
    let rate_level_index =
        dec.decode_icdf(&RATE_LEVELS_ICDF[(signal_type >> 1) as usize], 8)? as usize;
    debug_assert!(rate_level_index < N_RATE_LEVELS);

    /* Calculate number of shell blocks */
    let mut iter = frame_length >> LOG2_SHELL_CODEC_FRAME_LENGTH;
    if iter << LOG2_SHELL_CODEC_FRAME_LENGTH < frame_length {
        /* Make sure only happens for 10 ms @ 12 kHz */
        debug_assert!(
            frame_length == 12 * 10,
            "unexpected partial shell block for frame_length {frame_length}"
        );
        iter += 1;
    }
    debug_assert!(iter <= MAX_NB_SHELL_BLOCKS);
    debug_assert!(pulses.len() >= iter * SHELL_CODEC_FRAME_LENGTH);

    /***************************************************/
    /* Sum-Weighted-Pulses Decoding                    */
    /***************************************************/
    let mut sum_pulses = [0i32; MAX_NB_SHELL_BLOCKS];
    let mut n_lshifts = [0i32; MAX_NB_SHELL_BLOCKS];
    let cdf_row = &PULSES_PER_BLOCK_ICDF[rate_level_index];
    for i in 0..iter {
        n_lshifts[i] = 0;
        let mut s = dec.decode_icdf(cdf_row, 8)? as i32;

        /* LSB indication */
        while s == SILK_MAX_PULSES + 1 {
            n_lshifts[i] += 1;
            /* When we've already got 10 LSBs, we shift the table to
             * not allow (SILK_MAX_PULSES + 1) */
            let row = &PULSES_PER_BLOCK_ICDF[N_RATE_LEVELS - 1][(n_lshifts[i] == 10) as usize..];
            s = dec.decode_icdf(row, 8)? as i32;
        }
        sum_pulses[i] = s;
    }

    /***************************************************/
    /* Shell decoding                                  */
    /***************************************************/
    for i in 0..iter {
        let block = &mut pulses[i * SHELL_CODEC_FRAME_LENGTH..(i + 1) * SHELL_CODEC_FRAME_LENGTH];
        if sum_pulses[i] > 0 {
            shell_decoder(dec, block, sum_pulses[i])?;
        } else {
            block.fill(0);
        }
    }

    /***************************************************/
    /* LSB Decoding                                    */
    /***************************************************/
    for i in 0..iter {
        if n_lshifts[i] > 0 {
            let n_ls = n_lshifts[i];
            for k in 0..SHELL_CODEC_FRAME_LENGTH {
                let mut abs_q = pulses[i * SHELL_CODEC_FRAME_LENGTH + k] as i32;
                for _ in 0..n_ls {
                    abs_q <<= 1;
                    abs_q += dec.decode_icdf(&LSB_ICDF, 8)? as i32;
                }
                pulses[i * SHELL_CODEC_FRAME_LENGTH + k] = abs_q as i16;
            }
            /* Mark the number of pulses non-zero for sign decoding. */
            sum_pulses[i] |= n_ls << 5;
        }
    }

    /****************************************/
    /* Decode and add signs to pulse signal */
    /****************************************/
    decode_signs(
        dec,
        pulses,
        frame_length,
        signal_type,
        quant_offset_type,
        &sum_pulses,
    )
}

/// Decodes and applies the sign bits — port of `silk_decode_signs`
/// (`silk/code_signs.c`).
///
/// One sign symbol is consumed per *nonzero* magnitude in each block
/// whose marked pulse count is positive; the sign table row is selected
/// by the signal/quantization-offset types and the (masked) pulse count
/// determines the sign probability. The count is marked `pulses4 |
/// nLS << 5` by [`decode_pulses`], so the low 5 bits carry the shell
/// count and blocks with only LSB-refined magnitudes still get signs.
fn decode_signs(
    dec: &mut RangeDecoder<'_>,
    pulses: &mut [i16],
    length: usize,
    signal_type: i32,
    quant_offset_type: i32,
    sum_pulses: &[i32; MAX_NB_SHELL_BLOCKS],
) -> Result<()> {
    let mut icdf = [0u8; 2]; /* icdf[1] = 0 */
    let row = 7 * (quant_offset_type + (signal_type << 1)) as usize;
    debug_assert!(row + 6 < SIGN_ICDF.len());
    let icdf_ptr = &SIGN_ICDF[row..];
    let n_blocks = (length + SHELL_CODEC_FRAME_LENGTH / 2) >> LOG2_SHELL_CODEC_FRAME_LENGTH;
    debug_assert!(n_blocks <= sum_pulses.len());
    for i in 0..n_blocks {
        let p = sum_pulses[i];
        if p > 0 {
            icdf[0] = icdf_ptr[(p & 0x1F).min(6) as usize];
            for q in &mut pulses[i * SHELL_CODEC_FRAME_LENGTH..(i + 1) * SHELL_CODEC_FRAME_LENGTH] {
                if *q > 0 {
                    /* attach sign: symbol 0 -> -1, symbol 1 -> +1 */
                    let s = dec.decode_icdf(&icdf, 8)? as i16;
                    *q *= (s << 1) - 1;
                }
            }
        }
    }
    Ok(())
}

/// Reconstructs the Q14 excitation from the signed quantization
/// indices — the "Decode excitation" block at the top of
/// `silk_decode_core` (`silk/decode_core.c`).
///
/// For each sample: `exc_Q14 = (q << 14)`, pulled toward zero by
/// `QUANT_LEVEL_ADJUST_Q10` for nonzero `q`, shifted by the
/// signal-type/quantization-offset-dependent `offset_Q10`, then
/// conditionally negated by the frame's linear congruential generator
/// dither (`seed` is advanced first, the pulse value is accumulated
/// after the sign test — both with wraparound, as in the reference's
/// `silk_RAND`/`silk_ADD32_ovflw`). `exc_q14.len()` is the frame
/// length; `pulses` must be at least as long (its padded tail is
/// ignored, as in the reference).
pub(crate) fn reconstruct_excitation(
    exc_q14: &mut [i32],
    pulses: &[i16],
    seed: i8,
    signal_type: i8,
    quant_offset_type: i8,
) {
    debug_assert!(pulses.len() >= exc_q14.len());
    let offset_q10 =
        QUANTIZATION_OFFSETS_Q10[(signal_type >> 1) as usize][quant_offset_type as usize] as i32;

    let mut rand_seed = seed as i32;
    for (i, exc) in exc_q14.iter_mut().enumerate() {
        rand_seed = rand_seed
            .wrapping_mul(RAND_MULTIPLIER)
            .wrapping_add(RAND_INCREMENT);
        let mut e = (pulses[i] as i32).wrapping_shl(14);
        if e > 0 {
            e = e.wrapping_sub(QUANT_LEVEL_ADJUST_Q10 << 4);
        } else if e < 0 {
            e = e.wrapping_add(QUANT_LEVEL_ADJUST_Q10 << 4);
        }
        e = e.wrapping_add(offset_q10 << 4);
        if rand_seed < 0 {
            e = e.wrapping_neg();
        }
        *exc = e;

        rand_seed = rand_seed.wrapping_add(pulses[i] as i32);
    }
}
