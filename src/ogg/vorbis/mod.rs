//! Adapted packet-only Vorbis synthesis, MIT; see ../NOTICE.md.
#![allow(
    dead_code,
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    rustdoc::broken_intra_doc_links
)]
mod bitreader;
mod codebook;
mod fft;
mod floor;
mod header;
mod mdct;
mod residue;
use super::error::{Budget, CodecError as CadenceError};
use bitreader::BitReader;
use header::{IdHeader, Setup};
use mdct::{Mdct, vector_fmul_window};
use residue::ResidueWorkspace;
const WAV_ORDER_PERMUTATIONS: [[usize; 8]; 6] = [
    [0, 2, 1, 0, 0, 0, 0, 0],
    [0, 1, 2, 3, 0, 0, 0, 0],
    [0, 2, 1, 3, 4, 0, 0, 0],
    [0, 2, 1, 5, 3, 4, 0, 0],
    [0, 2, 1, 6, 5, 3, 4, 0],
    [0, 2, 1, 7, 5, 6, 3, 4],
];

pub(super) struct Core {
    id: IdHeader,
    setup: Setup,
    mdct: [Mdct; 2],
    residues: Vec<Box<[f32]>>,
    floors: Vec<Box<[f32]>>,
    saved: Vec<Box<[f32]>>,
    ola_scratch: Box<[f32]>,
    workspace: ResidueWorkspace,
    staging: Box<[f32]>,
    prev_blockflag: Option<bool>,
    first_audio: bool,
    next_flag: bool,
}
fn corrupt(what: &str) -> CadenceError {
    CadenceError::CorruptData(format!("Vorbis: {what}"))
}
impl Core {
    pub fn packet_size(&self, packet: &[u8]) -> crate::Result<usize> {
        let mut bits = BitReader::new(packet);
        if bits.read_bit()? {
            return Err(corrupt("unexpected audio header").into());
        }
        let mode = bits.read_bits(BitReader::ilog(self.setup.modes.len() as i64 - 1))? as usize;
        let mode = self
            .setup
            .modes
            .get(mode)
            .ok_or_else(|| corrupt("mode out of range"))?;
        if mode.blockflag {
            bits.read_bits(2)?;
        }
        Ok(1 << if mode.blockflag {
            self.id.blocksize_1_exp
        } else {
            self.id.blocksize_0_exp
        })
    }
    pub fn id(packet: &[u8]) -> crate::Result<(usize, u32, usize)> {
        let id = header::parse_id(packet)?;
        if id.channels > 8 {
            return Err(CadenceError::UnsupportedFeature(
                "Vorbis supports at most eight channels".into(),
            )
            .into());
        }
        Ok((
            id.channels as usize,
            id.sample_rate,
            1 << id.blocksize_1_exp,
        ))
    }
    pub fn new(
        id_packet: &[u8],
        setup_packet: &[u8],
        limits: &crate::Limits,
        packet_bytes: usize,
    ) -> crate::Result<Self> {
        let (channels, rate, n) = Self::id(id_packet)?;
        limits.check(0, channels, rate)?;
        let mut budget = Budget {
            remaining: limits.max_decoded_bytes.min(64 * 1024 * 1024),
        };
        budget.charge(n * 96 * (channels + 2) + packet_bytes + 256 * 1024)?;
        let id = header::parse_id(id_packet)?;
        let mut setup = header::parse_setup(setup_packet, &id, &mut budget)?;
        let n0 = 1 << id.blocksize_0_exp;
        for f in &mut setup.floors {
            if let floor::Floor::Zero(f0) = f {
                f0.build_maps(n0, n);
            }
        }
        let words = header::max_classbook_dimensions(&setup).max(1);
        Ok(Self {
            id,
            setup,
            mdct: [Mdct::new(n0), Mdct::new(n)],
            residues: (0..channels)
                .map(|_| vec![0.0; n / 2].into_boxed_slice())
                .collect(),
            floors: (0..channels)
                .map(|_| vec![0.0; n / 2].into_boxed_slice())
                .collect(),
            saved: (0..channels)
                .map(|_| vec![0.0; n / 4].into_boxed_slice())
                .collect(),
            ola_scratch: vec![0.0; n / 2].into_boxed_slice(),
            workspace: ResidueWorkspace::new(channels, n, words),
            staging: vec![0.0; n * channels].into_boxed_slice(),
            prev_blockflag: None,
            first_audio: true,
            next_flag: false,
        })
    }
    pub fn reset(&mut self) {
        for c in &mut self.saved {
            c.fill(0.0);
        }
        self.prev_blockflag = None;
        self.first_audio = true;
        self.next_flag = false;
    }
    pub fn decode(&mut self, packet: &[u8]) -> crate::Result<&[f32]> {
        let (frames, _, first) = self.decode_packet(packet)?;
        self.first_audio = false;
        let out = &self.staging[..if first {
            0
        } else {
            frames * self.id.channels as usize
        }];
        if out.iter().any(|s| !s.is_finite()) {
            return Err(crate::Error::decode("Nonfinite Vorbis PCM"));
        }
        Ok(out)
    }
    /// Decodes one audio packet, writing its `retlen` frames interleaved
    /// into `self.staging`. Returns `(frames, blockflag, was_first)`.
    fn decode_packet(&mut self, packet: &[u8]) -> Result<(usize, bool, bool), CadenceError> {
        let channels = self.id.channels as usize;
        let was_first = self.first_audio;
        let mut br = BitReader::new(packet);
        if br.read_bit()? {
            // A header packet mid-stream: chained-link boundary. The first
            // logical stream is the whole decode.
            return Err(corrupt("unexpected header in audio packets"));
        }
        let mode_count = self.setup.modes.len();
        let mode_number = if mode_count == 1 {
            0
        } else {
            br.read_bits(BitReader::ilog(mode_count as i64 - 1))? as usize
        };
        if mode_number >= mode_count {
            return Err(corrupt("mode number out of range"));
        }
        let blockflag = self.setup.modes[mode_number].blockflag;
        let mapping_idx = self.setup.modes[mode_number].mapping;
        // Spec §4.3.1: a long-window packet carries exactly TWO flag bits —
        // [previous_window_flag] then [next_window_flag]. The current window
        // comes from the mode's blockflag; there is no separate "window" bit.
        let mut prev_flag = false;
        if blockflag {
            prev_flag = br.read_bit()?;
            self.next_flag = br.read_bit()?;
        } else {
            self.next_flag = false;
        }
        let n = if blockflag {
            1usize << self.id.blocksize_1_exp
        } else {
            1usize << self.id.blocksize_0_exp
        };
        let vlen = n / 2;

        // Floor decode (channel order).
        let mut no_residue = [false; 256];
        for (ch, is_empty) in no_residue.iter_mut().enumerate().take(channels) {
            let (floor_idx, submap) = {
                let m = &self.setup.mappings[mapping_idx];
                let submap = if m.submaps > 1 { m.mux[ch] as usize } else { 0 };
                (m.submap_floor[submap] as usize, submap)
            };
            let _ = submap;
            let curve = &mut self.floors[ch][..vlen];
            let nonzero = crate::ogg::vorbis::floor::floor_decode(
                &mut self.setup.floors[floor_idx],
                &self.setup.codebooks,
                &mut br,
                blockflag as usize,
                curve,
                vlen,
            )?;
            *is_empty = !nonzero;
            if !nonzero {
                self.floors[ch][..vlen].fill(0.0);
            }
        }

        // Nonzero propagate through coupling pairs.
        {
            let m = &self.setup.mappings[mapping_idx];
            for i in (0..m.coupling_steps).rev() {
                let (a, b) = (m.magnitude[i], m.angle[i]);
                if !(no_residue[a] && no_residue[b]) {
                    no_residue[a] = false;
                    no_residue[b] = false;
                }
            }
        }

        // Residue decode (submap order). The spec allocates and zeroes the
        // return vectors per packet (§8: "allocate and zero all vectors")
        // because residue decoding ADDS into them — without this, stale
        // values from the previous packet accumulate into the output
        // (libvorbis memsets pcm[i] to n/2 in mapping0_inverse).
        for ch in 0..channels {
            self.residues[ch][..vlen].fill(0.0);
        }
        for submap in 0..self.setup.mappings[mapping_idx].submaps {
            let mut do_not_decode = [false; 256];
            let mut members = [usize::MAX; 256];
            let mut ch_count = 0usize;
            let m = &self.setup.mappings[mapping_idx];
            for (j, empty) in no_residue.iter().enumerate().take(channels) {
                if m.submaps == 1 || m.mux[j] as usize == submap {
                    members[ch_count] = j;
                    do_not_decode[ch_count] = *empty;
                    ch_count += 1;
                }
            }
            if ch_count == 0 {
                continue;
            }
            let res_idx = m.submap_residue[submap] as usize;
            let res = &self.setup.residues[res_idx];
            res.decode(
                &self.setup.codebooks,
                &mut br,
                &mut self.residues[..],
                &members[..ch_count],
                &do_not_decode[..ch_count],
                &mut self.workspace,
                vlen,
            )?;
        }
        for ch in 0..channels {
            self.residues[ch][vlen..].fill(0.0);
        }

        // Inverse coupling (reverse order; square polar, spec 4.3.5).
        for i in (0..self.setup.mappings[mapping_idx].coupling_steps).rev() {
            let (m_idx, a_idx) = {
                let m = &self.setup.mappings[mapping_idx];
                (m.magnitude[i], m.angle[i])
            };
            // Indices are distinct (validated at setup).
            let (lo, hi) = if m_idx < a_idx {
                (m_idx, a_idx)
            } else {
                (a_idx, m_idx)
            };
            let (first, second) = self.residues.split_at_mut(hi);
            let (mag, ang) = if m_idx < a_idx {
                (&mut first[lo], &mut second[0])
            } else {
                (&mut second[0], &mut first[lo])
            };
            inverse_coupling(mag, ang, vlen);
        }

        // Dot product with the floor curve + inverse MDCT per channel.
        for (ch, empty) in no_residue.iter().enumerate().take(channels) {
            if *empty {
                self.residues[ch][..vlen].fill(0.0);
                continue;
            }
            for k in 0..vlen {
                self.residues[ch][k] *= self.floors[ch][k];
            }
            let spectra = &mut self.residues[ch][..vlen];
            if blockflag {
                self.mdct[1].imdct_half(spectra);
            } else {
                self.mdct[0].imdct_half(spectra);
            }
        }

        // Overlap-add (the FFmpeg lapping scheme; the short window applies
        // whenever either neighbor block is short).
        let prev = if was_first {
            if blockflag { prev_flag } else { false }
        } else {
            self.prev_blockflag.unwrap_or(blockflag)
        };
        let bs0 = 1usize << self.id.blocksize_0_exp;
        let bs1 = 1usize << self.id.blocksize_1_exp;
        let retlen = (n + if prev { bs1 } else { bs0 }) / 4;
        let win_idx = (blockflag as usize) & (prev as usize);
        // `dst` indexes both the permutation table and the destination
        // stride; iterator style obscures the dual indexing.
        #[allow(clippy::needless_range_loop)]
        for dst in 0..channels {
            let win: &[f32] = if win_idx == 1 {
                self.mdct[1].window()
            } else {
                self.mdct[0].window()
            };
            // Vorbis orders multichannel output in its own stream order
            // (e.g. 6 ch: L C R SL SR LFE); the decoded PCM handed to the
            // engine follows WAV order (L R C LFE SL SR), matching
            // FFmpeg's ff_vorbis_channel_layout_offsets: WAV channel
            // `dst` carries the content of source channel perm[dst].
            // Channel state (residues/saved) stays indexed by source
            // channel; only the staging destination is permuted.
            let src = if channels <= 2 {
                dst
            } else {
                WAV_ORDER_PERMUTATIONS[(channels - 3).min(5)][dst]
            };
            overlap_add_channel(
                &mut self.staging[..],
                dst,
                channels,
                &self.residues[src][..],
                &mut self.saved[src][..],
                win,
                blockflag,
                prev,
                bs0,
                bs1,
                retlen,
                &mut self.ola_scratch[..],
            );
        }
        self.prev_blockflag = Some(blockflag);
        Ok((retlen, blockflag, was_first))
    }
}

/// Square-polar inverse coupling over `len` samples (spec 4.3.5).
fn inverse_coupling(mag: &mut [f32], ang: &mut [f32], len: usize) {
    for k in 0..len {
        let m = mag[k];
        let a = ang[k];
        match (m > 0.0, a > 0.0) {
            (true, true) => {
                ang[k] = m - a;
            }
            (true, false) => {
                ang[k] = m;
                mag[k] = m + a;
            }
            (false, true) => {
                ang[k] = m + a;
            }
            (false, false) => {
                ang[k] = m;
                mag[k] = m - a;
            }
        }
    }
}

/// Applies the FFmpeg overlap-add for one channel, writing `retlen`
/// interleaved frames into `staging` at channel offset `ch`.
#[allow(clippy::too_many_arguments)]
fn overlap_add_channel(
    staging: &mut [f32],
    ch: usize,
    channels: usize,
    buf: &[f32],
    saved: &mut [f32],
    win: &[f32],
    blockflag: bool,
    prev: bool,
    bs0: usize,
    bs1: usize,
    retlen: usize,
    scratch: &mut [f32],
) {
    let n = if blockflag { bs1 } else { bs0 };
    if blockflag == prev {
        vector_fmul_window(&mut scratch[..retlen], saved, buf, win, n / 4);
    } else if blockflag && !prev {
        // Short prev -> long cur.
        let head_len = bs0 / 2;
        vector_fmul_window(&mut scratch[..head_len], saved, buf, win, bs0 / 4);
        let copy = (bs1 - bs0) / 4;
        scratch[head_len..head_len + copy].copy_from_slice(&buf[bs0 / 4..bs0 / 4 + copy]);
    } else {
        // Long prev -> short cur.
        let lead = (bs1 - bs0) / 4;
        scratch[..lead].copy_from_slice(&saved[..lead]);
        vector_fmul_window(
            &mut scratch[lead..lead + bs0 / 2],
            &saved[lead..],
            buf,
            win,
            bs0 / 4,
        );
    }
    saved[..n / 4].copy_from_slice(&buf[n / 4..n / 2]);
    for (i, &s) in scratch[..retlen].iter().enumerate() {
        staging[ch + i * channels] = s;
    }
}
