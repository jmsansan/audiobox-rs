// Adapted from tpt-cadence, MIT; see src/ogg/NOTICE.md.
//! Iterative radix-2 complex FFT used by the synthesis MDCT.
//!
//! Sizes are powers of two up to 8192 (a Vorbis long block is at most 8192
//! samples, and the MDCT runs one size-`n` transform per channel per block).
//! Twiddles and the bit-reversal permutation are computed once at
//! construction; [`Fft::run`] is allocation-free.

/// A bare complex float pair (avoids a dependency on an external complex
/// crate; the operations needed are trivial).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct C {
    pub re: f32,
    pub im: f32,
}

impl C {
    pub(crate) fn new(re: f32, im: f32) -> Self {
        C { re, im }
    }

    fn mul(self, o: C) -> C {
        C {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }
}

/// Forward (+i exponent) radix-2 FFT of a power-of-two length.
pub struct Fft {
    n: usize,
    /// Bit-reversal permutation of 0..n.
    bitrev: Box<[usize]>,
    /// Twiddle factors `e^{-i*2*pi*k/n}` for the decimation-in-time stages,
    /// stored for stride `n/2, n/4, .., 1` concatenated per stage.
    twiddle: Box<[C]>,
    /// Scratch, so `run` can be called on borrowed buffers without allocation.
    scratch: Box<[C]>,
}

impl Fft {
    /// Plans a transform of size `n` (must be a power of two, n >= 2).
    ///
    /// `forward` selects the sign of the exponent: `true` computes
    /// `X[k] = sum_j x[j] * e^{-i*2*pi*j*k/n}`; `false` (the "inverse")
    /// computes `X[k] = sum_j x[j] * e^{+i*2*pi*j*k/n}`, unnormalized.
    pub fn new(n: usize, forward: bool) -> Self {
        assert!(n.is_power_of_two() && n >= 2);
        let bitrev = (0..n)
            .map(|i| i.reverse_bits() >> (usize::BITS - n.trailing_zeros()))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let mut twiddle = Vec::with_capacity(n);
        let sign = if forward { -1.0f64 } else { 1.0f64 };
        let mut stage = 2usize;
        while stage <= n {
            let count = stage / 2;
            for k in 0..count {
                let ang = sign * 2.0 * std::f64::consts::PI * k as f64 / stage as f64;
                twiddle.push(C::new(ang.cos() as f32, ang.sin() as f32));
            }
            stage *= 2;
        }
        Fft {
            n,
            bitrev,
            twiddle: twiddle.into_boxed_slice(),
            scratch: vec![C::default(); n].into_boxed_slice(),
        }
    }

    /// Runs the transform from `input` into `output` (both length `n`).
    /// Allocation-free.
    pub fn run(&mut self, input: &[C], output: &mut [C]) {
        assert_eq!(input.len(), self.n);
        assert!(output.len() >= self.n);
        let n = self.n;
        // Bit-reversal permute into scratch.
        for (i, &src) in self.bitrev.iter().enumerate() {
            self.scratch[i] = input[src];
        }
        // Butterflies.
        let mut stage = 2usize;
        let mut tw_base = 0usize;
        while stage <= n {
            let half = stage / 2;
            let mut block = 0usize;
            while block < n {
                for k in 0..half {
                    let w = self.twiddle[tw_base + k];
                    let a = self.scratch[block + k];
                    let b = self.scratch[block + k + half].mul(w);
                    self.scratch[block + k] = C::new(a.re + b.re, a.im + b.im);
                    self.scratch[block + k + half] = C::new(a.re - b.re, a.im - b.im);
                }
                block += stage;
            }
            tw_base += half;
            stage *= 2;
        }
        output[..n].copy_from_slice(&self.scratch);
    }
}
