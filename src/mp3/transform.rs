#![allow(clippy::needless_range_loop)]
use super::{decoder::Granule, tables::*};
use std::{f64::consts::PI, sync::OnceLock};
struct Basis {
    synth: Vec<f64>,
    analysis: Vec<f64>,
    long: Vec<f64>,
    short: Vec<f64>,
    windows: [[f64; 36]; 4],
}
fn basis() -> &'static Basis {
    static BASIS: OnceLock<Basis> = OnceLock::new();
    BASIS.get_or_init(|| {
        let synth = (0..2048)
            .map(|n| {
                let i = n / 32;
                let k = n % 32;
                ((16 + i) as f64 * (2 * k + 1) as f64 * PI / 64.0).cos()
            })
            .collect();
        let analysis = (0..2048)
            .map(|n| {
                let k = n / 64;
                let i = n % 64;
                ((2 * k + 1) as f64 * (i as f64 - 16.0) * PI / 64.0).cos()
            })
            .collect();
        let long = (0..648)
            .map(|n| {
                let p = n / 18;
                let m = n % 18;
                (PI / 72.0 * (2 * p + 19) as f64 * (2 * m + 1) as f64).cos()
            })
            .collect();
        let short = (0..72)
            .map(|n| {
                let p = n / 6;
                let m = n % 6;
                (PI / 24.0 * (2 * p + 7) as f64 * (2 * m + 1) as f64).cos()
            })
            .collect();
        let mut windows = [[0.0; 36]; 4];
        for i in 0..36 {
            windows[0][i] = (PI / 36.0 * (i as f64 + 0.5)).sin();
            windows[1][i] = if i < 18 {
                windows[0][i]
            } else if i < 24 {
                1.0
            } else if i < 30 {
                (PI / 12.0 * (i as f64 - 17.5)).sin()
            } else {
                0.0
            };
            windows[2][i] = if i < 12 {
                (PI / 12.0 * (i as f64 + 0.5)).sin()
            } else {
                0.0
            };
            windows[3][i] = if i < 6 {
                0.0
            } else if i < 12 {
                (PI / 12.0 * (i as f64 - 5.5)).sin()
            } else if i < 18 {
                1.0
            } else {
                windows[0][i]
            };
        }
        Basis {
            synth,
            analysis,
            long,
            short,
            windows,
        }
    })
}
pub(super) fn alias(x: &mut [f32; 576], short: bool, mixed: bool, inverse: bool) {
    if short && !mixed {
        return;
    }
    let boundaries = if short { 1 } else { 31 };
    for b in 0..boundaries {
        for i in 0..8 {
            let a = b * 18 + 17 - i;
            let z = b * 18 + 18 + i;
            let lower = x[a] as f64;
            let upper = x[z] as f64;
            let cs = ALIAS_CS[i] as f64;
            let ca = ALIAS_CA[i] as f64;
            if inverse {
                x[a] = (lower * cs + upper * ca) as f32;
                x[z] = (upper * cs - lower * ca) as f32;
            } else {
                x[a] = (lower * cs - upper * ca) as f32;
                x[z] = (upper * cs + lower * ca) as f32;
            }
        }
    }
}
pub(super) fn invert(x: &mut [f32; 576]) {
    for s in (1..32).step_by(2) {
        for i in (1..18).step_by(2) {
            x[s * 18 + i] = -x[s * 18 + i];
        }
    }
}
pub(super) fn imdct(spectrum: &[f32; 576], g: &Granule, overlap: &mut [f32; 576]) -> [f32; 576] {
    let b = basis();
    let mut out = [0.0; 576];
    let short = g.block == 2;
    for sb in 0..32 {
        let offset = sb * 18;
        let mut transformed = [0.0f64; 36];
        if short && !(g.mixed && sb < 2) {
            for sub in 0..3 {
                for p in 0..12 {
                    let sum = (0..6)
                        .map(|m| spectrum[offset + sub * 6 + m] as f64 * b.short[p * 6 + m])
                        .sum::<f64>();
                    transformed[6 + sub * 6 + p] += sum * b.windows[2][p];
                }
            }
        } else {
            let win = if g.mixed && sb < 2 { 0 } else { g.block };
            for (p, v) in transformed.iter_mut().enumerate() {
                *v = (0..18)
                    .map(|m| spectrum[offset + m] as f64 * b.long[p * 18 + m])
                    .sum::<f64>()
                    * b.windows[win][p];
            }
        }
        for i in 0..18 {
            out[offset + i] = (transformed[i] + overlap[offset + i] as f64) as f32;
            overlap[offset + i] = transformed[i + 18] as f32;
        }
    }
    invert(&mut out);
    out
}
pub(super) struct Synthesis {
    history: [f32; 1024],
    offset: usize,
}
impl Synthesis {
    pub fn new() -> Self {
        Self {
            history: [0.0; 1024],
            offset: 0,
        }
    }
    pub fn process(&mut self, subbands: &[f32; 32]) -> [f32; 32] {
        let b = basis();
        self.offset = (self.offset + 960) & 1023;
        let base = self.offset;
        for i in 0..64 {
            self.history[base + i] = (0..32)
                .map(|k| subbands[k] as f64 * b.synth[i * 32 + k])
                .sum::<f64>() as f32;
        }
        let mut out = [0.0; 32];
        for (j, v) in out.iter_mut().enumerate() {
            let mut sum = 0.0;
            for i in 0..8 {
                sum += self.history[(base + j + 128 * i) & 1023] as f64
                    * SYNTH_WINDOW[j + 64 * i] as f64;
                sum += self.history[(base + 96 + j + 128 * i) & 1023] as f64
                    * SYNTH_WINDOW[j + 64 * i + 32] as f64;
            }
            *v = sum as f32;
        }
        out
    }
    pub fn granule(&mut self, time: &[f32; 576]) -> Vec<f32> {
        let mut out = Vec::with_capacity(576);
        for slot in 0..18 {
            let mut sub = [0.0; 32];
            for sb in 0..32 {
                sub[sb] = time[sb * 18 + slot];
            }
            out.extend_from_slice(&self.process(&sub));
        }
        out
    }
}
pub(super) struct Analysis {
    delay: [f32; 512],
    overlap: [f32; 576],
}
impl Analysis {
    pub fn new() -> Self {
        Self {
            delay: [0.0; 512],
            overlap: [0.0; 576],
        }
    }
    pub fn granule(&mut self, input: &[f32], start: usize) -> [f32; 576] {
        let b = basis();
        let mut subbands = [0.0; 576];
        for slot in 0..18 {
            self.delay.copy_within(..480, 32);
            for i in 0..32 {
                self.delay[31 - i] = input.get(start + slot * 32 + i).copied().unwrap_or(0.0);
            }
            let mut y = [0.0; 64];
            for (i, v) in y.iter_mut().enumerate() {
                *v = (0..8)
                    .map(|j| self.delay[i + 64 * j] as f64 * ANALYSIS_WINDOW[i + 64 * j] as f64)
                    .sum::<f64>();
            }
            for k in 0..32 {
                subbands[k * 18 + slot] =
                    (0..64).map(|i| b.analysis[k * 64 + i] * y[i]).sum::<f64>() as f32;
            }
        }
        invert(&mut subbands);
        let mut out = [0.0; 576];
        for sb in 0..32 {
            let offset = sb * 18;
            let mut frame = [0.0; 36];
            for i in 0..18 {
                frame[i] = self.overlap[offset + i] as f64;
                frame[i + 18] = subbands[offset + i] as f64;
            }
            for k in 0..18 {
                out[offset + k] = (0..36)
                    .map(|n| frame[n] * b.windows[0][n] * b.long[n * 18 + k] * (2.0 / 18.0))
                    .sum::<f64>() as f32;
            }
            self.overlap[offset..offset + 18].copy_from_slice(&subbands[offset..offset + 18]);
        }
        alias(&mut out, false, false, true);
        out
    }
}
