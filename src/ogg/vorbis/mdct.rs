// Adapted from tpt-cadence, MIT; see src/ogg/NOTICE.md.
//! Synthesis MDCT and Vorbis window.
//!
//! The transform is numerically identical to FFmpeg's `av_tx` inverse MDCT
//! (`AV_TX_FLOAT_MDCT`, inverse, scale −1) as used by its Vorbis decoder —
//! the same half-length output array, so the overlap-add that follows
//! reproduces the reference PCM. The unit tests pin the fast path to a
//! transcription of FFmpeg's `ff_tx_mdct_naive_inv` definition.
//!
//! The window is the Vorbis window
//! `w[n] = sin( (pi/2) * sin^2( pi (n + 1/2) / N ) )`.

use crate::ogg::vorbis::fft::{C, Fft};

/// Per-block-size synthesis transform (allocation confined to [`Mdct::new`]).
pub struct Mdct {
    /// Full block length `N`.
    n: usize,
    fft: Fft,
    gbuf: Box<[C]>,
    obuf: Box<[C]>,
    /// Pre-rotation twiddles: `g[j] = (-1)^j * e^{i pi (2j+1) / N}`.
    gtw: Box<[C]>,
    phase_sin: Box<[f32]>,
    phase_cos: Box<[f32]>,
    /// Vorbis window, `N/2` samples (the half used by the lapping scheme).
    window: Box<[f32]>,
}

impl Mdct {
    /// Plans the transform and window for a full block of `n` samples.
    pub fn new(n: usize) -> Self {
        assert!(n.is_power_of_two() && (64..=8192).contains(&n));
        let nf = n as f64;
        let window = (0..n / 2)
            .map(|m| {
                let s = (std::f64::consts::PI * (m as f64 + 0.5) / nf).sin();
                (0.5 * std::f64::consts::PI * s * s).sin() as f32
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let gtw = (0..n / 2)
            .map(|j| {
                let ang = std::f64::consts::PI * (2 * j + 1) as f64 / (2.0 * nf);
                let sign = if j & 1 == 0 { 1.0 } else { -1.0 };
                C::new((ang.cos() as f32) * sign, (ang.sin() as f32) * sign)
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let phase_sin = (0..n / 2)
            .map(|m| (std::f64::consts::PI * m as f64 / nf).sin() as f32)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let phase_cos = (0..n / 2)
            .map(|m| (std::f64::consts::PI * m as f64 / nf).cos() as f32)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Mdct {
            n,
            fft: Fft::new(n, false),
            gbuf: vec![C::default(); n].into_boxed_slice(),
            obuf: vec![C::default(); n].into_boxed_slice(),
            gtw,
            phase_sin,
            phase_cos,
            window,
        }
    }

    /// The `N/2`-sample Vorbis window.
    pub fn window(&self) -> &[f32] {
        &self.window
    }

    /// Inverse MDCT with FFmpeg's half-length convention and scale −1.
    ///
    /// Transforms `N/2` spectral coefficients into the `N/2` (redundant-half)
    /// time samples, in place. Allocation-free.
    pub fn imdct_half(&mut self, spec_out: &mut [f32]) {
        let n = self.n;
        let half = n / 2;
        debug_assert!(spec_out.len() >= half);
        {
            let (g, gtw) = (&mut self.gbuf, &self.gtw);
            for j in 0..half {
                let x = spec_out[j];
                g[j] = C::new(x * gtw[j].re, x * gtw[j].im);
            }
            for g in g[half..].iter_mut() {
                *g = C::default();
            }
        }
        self.fft.run(&self.gbuf, &mut self.obuf);
        // out[m] = -Im{ e^{i phi_m} * G[m] }, phi_m = pi m / N
        for (m, ((g, sin), cos)) in self
            .obuf
            .iter()
            .zip(self.phase_sin.iter())
            .zip(self.phase_cos.iter())
            .enumerate()
            .take(half)
        {
            spec_out[m] = -(g.re * sin + g.im * cos);
        }
    }
}

/// FFmpeg's `vector_fmul_window` overlap step (`libavutil/float_dsp.c`).
///
/// Combines the previous block's saved (unwindowed) right half `src0` with
/// the current block's half-MDCT output `src1` through the `2*len`-sample
/// window `win`, writing `2*len` samples to `dst`:
///
/// ```text
/// dst[a]        = src0[a] * win[2len-1-a] - src1[len-1-a] * win[a]
/// dst[2len-1-a] = src0[a] * win[a]        + src1[len-1-a] * win[2len-1-a]
/// ```
pub fn vector_fmul_window(dst: &mut [f32], src0: &[f32], src1: &[f32], win: &[f32], len: usize) {
    debug_assert!(
        dst.len() >= 2 * len && src0.len() >= len && src1.len() >= len && win.len() >= 2 * len
    );
    for a in 0..len {
        let b = 2 * len - 1 - a;
        let s0 = src0[a];
        let s1 = src1[len - 1 - a];
        let wa = win[a];
        let wb = win[b];
        dst[a] = s0 * wb - s1 * wa;
        dst[b] = s0 * wa + s1 * wb;
    }
}
