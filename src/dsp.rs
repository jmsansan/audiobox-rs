use crate::{Audio, Error, Limits, Result};
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, Default)]
pub enum FadeCurve {
    #[default]
    Linear,
    EqualPower,
    Exponential,
}
#[derive(Clone, Copy, Debug)]
pub enum FilterType {
    Lowpass,
    Highpass,
    Bandpass,
    Notch,
    Peaking,
    Lowshelf,
    Highshelf,
    Allpass,
}
#[derive(Clone, Copy, Debug)]
pub struct FilterOptions {
    pub kind: FilterType,
    pub frequency: f64,
    pub q: f64,
    pub gain_db: f64,
}
impl FilterOptions {
    pub fn new(kind: FilterType, frequency: f64) -> Self {
        Self {
            kind,
            frequency,
            q: std::f64::consts::FRAC_1_SQRT_2,
            gain_db: 0.0,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub enum NormalizeUnit {
    #[default]
    Peak,
    TruePeak,
    Lufs,
}
#[derive(Clone, Copy, Debug)]
pub struct NormalizeOptions {
    pub to: f64,
    pub unit: NormalizeUnit,
    pub peak_ceiling_db: f64,
}
impl Default for NormalizeOptions {
    fn default() -> Self {
        Self {
            to: -1.0,
            unit: NormalizeUnit::Peak,
            peak_ceiling_db: -1.0,
        }
    }
}
#[derive(Clone, Debug)]
pub struct NormalizeReport {
    pub measured: f64,
    pub gain_db: f64,
    pub limited_by_peak: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct SilenceOptions {
    pub threshold_db: f64,
    pub min_duration_ms: f64,
    pub window_ms: f64,
    pub padding_ms: f64,
}
impl Default for SilenceOptions {
    fn default() -> Self {
        Self {
            threshold_db: -50.0,
            min_duration_ms: 100.0,
            window_ms: 10.0,
            padding_ms: 0.0,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct SilentRange {
    pub from: usize,
    pub to: usize,
}
#[derive(Clone, Copy, Debug)]
pub struct LimiterOptions {
    pub ceiling_db: f64,
    pub lookahead_ms: f64,
    pub release_ms: f64,
}
impl Default for LimiterOptions {
    fn default() -> Self {
        Self {
            ceiling_db: -1.0,
            lookahead_ms: 5.0,
            release_ms: 50.0,
        }
    }
}

pub(crate) fn biquad(input: &[f32], c: [f64; 5]) -> Vec<f32> {
    let [b0, b1, b2, a1, a2] = c;
    let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
    input
        .iter()
        .map(|&s| {
            let x = s as f64;
            let y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            x2 = x1;
            x1 = x;
            y2 = y1;
            y1 = y;
            y as f32
        })
        .collect()
}
fn i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..50 {
        term *= x * x / (4.0 * (k * k) as f64);
        sum += term;
        if term < sum * 1e-15 {
            break;
        }
    }
    sum
}
fn sinc_table() -> &'static Vec<f64> {
    static TABLE: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let norm = i0(10.0);
        (0..=32 * 512)
            .map(|i| {
                let d = i as f64 / 512.0;
                let t = d / 32.0;
                let sinc = if d == 0.0 {
                    1.0
                } else {
                    (PI * d).sin() / (PI * d)
                };
                sinc * i0(10.0 * (1.0 - t * t).max(0.0).sqrt()) / norm
            })
            .collect()
    })
}
pub(crate) fn sinc_sample(center: f64, ratio: f64, mut sample: impl FnMut(i64) -> f32) -> f32 {
    let cutoff = ratio.min(1.0) * 0.97;
    let half = 32.0 / cutoff;
    let table = sinc_table();
    let mut sum = 0.0;
    let mut weight = 0.0;
    for j in (center - half).ceil() as i64..=(center + half).floor() as i64 {
        let pos = (center - j as f64).abs() * cutoff * 512.0;
        let idx = pos as usize;
        if idx >= table.len() - 1 {
            continue;
        }
        let frac = pos - idx as f64;
        let w = table[idx] + (table[idx + 1] - table[idx]) * frac;
        sum += w * sample(j) as f64;
        weight += w;
    }
    if weight.abs() > 1e-15 {
        (sum / weight) as f32
    } else {
        0.0
    }
}
pub(crate) fn resample_channel(input: &[f32], ratio: f64, n: usize) -> Vec<f32> {
    if input.is_empty() {
        return vec![0.0; n];
    }
    (0..n)
        .map(|i| {
            sinc_sample(i as f64 / ratio, ratio, |j| {
                input[j.clamp(0, input.len() as i64 - 1) as usize]
            })
        })
        .collect()
}
impl Audio {
    pub fn gain(&self, linear: f64) -> Result<Self> {
        if !linear.is_finite() {
            return Err(Error::invalid("Gain must be finite"));
        }
        let data: Vec<Vec<f32>> = self
            .data
            .iter()
            .map(|c| c.iter().map(|&s| (s as f64 * linear) as f32).collect())
            .collect();
        if data.iter().flatten().any(|s| !s.is_finite()) {
            return Err(Error::invalid("Gain overflow"));
        }
        Ok(self.with_data(data))
    }
    pub fn gain_db(&self, db: f64) -> Result<Self> {
        if !db.is_finite() {
            return Err(Error::invalid("Gain must be finite"));
        }
        self.gain(10f64.powf(db / 20.0))
    }
    pub fn normalize(&self, options: NormalizeOptions) -> Result<Self> {
        self.normalize_with_report(options).map(|(a, _)| a)
    }
    pub fn normalize_with_report(
        &self,
        options: NormalizeOptions,
    ) -> Result<(Self, NormalizeReport)> {
        if !options.to.is_finite() || !options.peak_ceiling_db.is_finite() {
            return Err(Error::invalid("Normalization levels must be finite"));
        }
        let measured = match options.unit {
            NormalizeUnit::Peak => self.peak_db(),
            NormalizeUnit::TruePeak => self.true_peak_db(),
            NormalizeUnit::Lufs => self.loudness().integrated,
        };
        if !measured.is_finite() {
            return Ok((
                self.clone(),
                NormalizeReport {
                    measured,
                    gain_db: 0.0,
                    limited_by_peak: false,
                },
            ));
        }
        let mut gain = options.to - measured;
        let mut limited = false;
        if matches!(options.unit, NormalizeUnit::Lufs) {
            let max = options.peak_ceiling_db - self.true_peak_db();
            if gain > max {
                gain = max;
                limited = true;
            }
        }
        let audio = self.gain_db(gain)?;
        Ok((
            audio,
            NormalizeReport {
                measured,
                gain_db: gain,
                limited_by_peak: limited,
            },
        ))
    }
    pub fn fade(&self, in_seconds: f64, out_seconds: f64, curve: FadeCurve) -> Result<Self> {
        if !in_seconds.is_finite()
            || !out_seconds.is_finite()
            || in_seconds < 0.0
            || out_seconds < 0.0
        {
            return Err(Error::invalid(
                "Fade durations must be finite and non-negative",
            ));
        }
        let n_in = (in_seconds * self.rate as f64)
            .round()
            .min(self.frames() as f64) as usize;
        let n_out = (out_seconds * self.rate as f64)
            .round()
            .min(self.frames() as f64) as usize;
        let shape = |t: f64| match curve {
            FadeCurve::Linear => t,
            FadeCurve::EqualPower => (t * PI / 2.0).sin(),
            FadeCurve::Exponential => t * t,
        };
        let mut data = self.data.clone();
        for c in &mut data {
            let len = c.len();
            for (i, s) in c.iter_mut().enumerate() {
                let mut g = 1.0;
                if i < n_in {
                    g *= shape(i as f64 / n_in.saturating_sub(1).max(1) as f64);
                }
                if i >= len - n_out && n_out > 0 {
                    g *= shape((len - 1 - i) as f64 / n_out.saturating_sub(1).max(1) as f64);
                }
                *s *= g as f32;
            }
        }
        Ok(self.with_data(data))
    }
    pub fn remove_dc_offset(&self) -> Self {
        self.with_data(
            self.data
                .iter()
                .map(|c| {
                    let mean = if c.is_empty() {
                        0.0
                    } else {
                        c.iter().map(|&s| s as f64).sum::<f64>() / c.len() as f64
                    };
                    c.iter().map(|&s| (s as f64 - mean) as f32).collect()
                })
                .collect(),
        )
    }
    pub fn resample(&self, sample_rate: u32) -> Result<Self> {
        let ratio = sample_rate as f64 / self.rate as f64;
        let frames = (self.frames() as f64 * ratio).round() as usize;
        Limits::default().check(frames, self.channels(), sample_rate)?;
        if sample_rate == self.rate {
            return Ok(self.clone());
        }
        let mut audio = self.with_data(
            self.data
                .iter()
                .map(|c| resample_channel(c, ratio, frames))
                .collect(),
        );
        audio.rate = sample_rate;
        Ok(audio)
    }
    pub fn filter(&self, options: FilterOptions) -> Result<Self> {
        let FilterOptions {
            kind,
            frequency,
            q,
            gain_db,
        } = options;
        if !frequency.is_finite()
            || frequency <= 0.0
            || frequency >= self.rate as f64 / 2.0
            || !q.is_finite()
            || q <= 0.0
            || !gain_db.is_finite()
        {
            return Err(Error::invalid("Invalid filter frequency, Q or gain"));
        }
        let w = 2.0 * PI * frequency / self.rate as f64;
        let s = w.sin();
        let c = w.cos();
        let alpha = s / (2.0 * q);
        let a = 10f64.powf(gain_db / 40.0);
        let beta = 2.0 * a.sqrt() * alpha;
        let (b0, b1, b2, a0, a1, a2) = match kind {
            FilterType::Lowpass => (
                (1.0 - c) / 2.0,
                1.0 - c,
                (1.0 - c) / 2.0,
                1.0 + alpha,
                -2.0 * c,
                1.0 - alpha,
            ),
            FilterType::Highpass => (
                (1.0 + c) / 2.0,
                -(1.0 + c),
                (1.0 + c) / 2.0,
                1.0 + alpha,
                -2.0 * c,
                1.0 - alpha,
            ),
            FilterType::Bandpass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
            FilterType::Notch => (1.0, -2.0 * c, 1.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
            FilterType::Allpass => (
                1.0 - alpha,
                -2.0 * c,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * c,
                1.0 - alpha,
            ),
            FilterType::Peaking => (
                1.0 + alpha * a,
                -2.0 * c,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * c,
                1.0 - alpha / a,
            ),
            FilterType::Lowshelf => (
                a * ((a + 1.0) - (a - 1.0) * c + beta),
                2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                a * ((a + 1.0) - (a - 1.0) * c - beta),
                (a + 1.0) + (a - 1.0) * c + beta,
                -2.0 * ((a - 1.0) + (a + 1.0) * c),
                (a + 1.0) + (a - 1.0) * c - beta,
            ),
            FilterType::Highshelf => (
                a * ((a + 1.0) + (a - 1.0) * c + beta),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                a * ((a + 1.0) + (a - 1.0) * c - beta),
                (a + 1.0) - (a - 1.0) * c + beta,
                2.0 * ((a - 1.0) - (a + 1.0) * c),
                (a + 1.0) - (a - 1.0) * c - beta,
            ),
        };
        let co = [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0];
        if co.iter().any(|x| !x.is_finite()) {
            return Err(Error::invalid("Filter coefficient overflow"));
        }
        let data: Vec<Vec<f32>> = self.data.iter().map(|c| biquad(c, co)).collect();
        if data.iter().flatten().any(|x| !x.is_finite()) {
            return Err(Error::invalid("Filter overflow"));
        }
        Ok(self.with_data(data))
    }
    pub fn limit(&self, options: LimiterOptions) -> Result<Self> {
        if !options.ceiling_db.is_finite()
            || !options.lookahead_ms.is_finite()
            || !options.release_ms.is_finite()
            || options.lookahead_ms < 0.0
            || options.release_ms <= 0.0
        {
            return Err(Error::invalid("Invalid limiter settings"));
        }
        let ceiling = 10f64.powf(options.ceiling_db / 20.0);
        if !ceiling.is_finite() || ceiling <= 0.0 {
            return Err(Error::invalid("Invalid limiter ceiling"));
        }
        let look = (options.lookahead_ms * self.rate as f64 / 1000.0)
            .round()
            .min(self.frames() as f64) as usize;
        let release = (-1.0 / (options.release_ms * self.rate as f64 / 1000.0)).exp();
        let mut data = self.data.clone();
        let mut gain = 1.0;
        let mut deque = std::collections::VecDeque::<(usize, f64)>::new();
        let mut next = 0;
        for i in 0..self.frames() {
            while next < self.frames() && next <= i.saturating_add(look) {
                let peak = self
                    .data
                    .iter()
                    .map(|c| c[next].abs() as f64)
                    .fold(0.0, f64::max);
                while deque.back().is_some_and(|(_, p)| *p <= peak) {
                    deque.pop_back();
                }
                deque.push_back((next, peak));
                next += 1;
            }
            while deque.front().is_some_and(|(at, _)| *at < i) {
                deque.pop_front();
            }
            let peak = deque.front().map_or(0.0, |(_, p)| *p);
            let target = if peak > ceiling { ceiling / peak } else { 1.0 };
            gain = if target < gain {
                target
            } else {
                target + (gain - target) * release
            };
            for c in &mut data {
                c[i] *= gain as f32;
            }
        }
        Ok(self.with_data(data))
    }
    pub fn detect_silence(&self, options: SilenceOptions) -> Result<Vec<SilentRange>> {
        if !options.threshold_db.is_finite()
            || !options.min_duration_ms.is_finite()
            || !options.window_ms.is_finite()
            || !options.padding_ms.is_finite()
            || options.min_duration_ms < 0.0
            || options.window_ms <= 0.0
            || options.padding_ms < 0.0
        {
            return Err(Error::invalid("Invalid silence settings"));
        }
        let window = (options.window_ms * self.rate as f64 / 1000.0)
            .round()
            .max(1.0)
            .min(self.frames().max(1) as f64) as usize;
        let min = (options.min_duration_ms * self.rate as f64 / 1000.0).round() as usize;
        let threshold = 10f64.powf(options.threshold_db / 10.0);
        let mut ranges = Vec::new();
        let mut start = None;
        for i in (0..self.frames()).step_by(window) {
            let end = (i + window).min(self.frames());
            let silent = self.data.iter().all(|c| {
                c[i..end].iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / (end - i) as f64
                    <= threshold
            });
            if silent {
                if start.is_none() {
                    start = Some(i);
                }
            } else if let Some(from) = start.take() {
                if i - from >= min {
                    ranges.push(SilentRange { from, to: i });
                }
            }
        }
        if let Some(from) = start {
            if self.frames() - from >= min {
                ranges.push(SilentRange {
                    from,
                    to: self.frames(),
                });
            }
        }
        Ok(ranges)
    }
    pub fn trim_silence(&self, options: SilenceOptions) -> Result<Self> {
        let ranges = self.detect_silence(options)?;
        let padding = (options.padding_ms * self.rate as f64 / 1000.0).round() as usize;
        let start = ranges
            .first()
            .filter(|r| r.from == 0)
            .map_or(0, |r| r.to.saturating_sub(padding));
        let end = ranges
            .last()
            .filter(|r| r.to == self.frames())
            .map_or(self.frames(), |r| {
                r.from.saturating_add(padding).min(self.frames())
            })
            .max(start);
        Ok(self.with_data(self.data.iter().map(|c| c[start..end].to_vec()).collect()))
    }
    pub fn speed(&self, factor: f64) -> Result<Self> {
        if !factor.is_finite() || factor <= 0.0 {
            return Err(Error::invalid("Speed must be positive and finite"));
        }
        let frames = (self.frames() as f64 / factor).round() as usize;
        Limits::default().check(frames, self.channels(), self.rate)?;
        if factor == 1.0 {
            return Ok(self.clone());
        }
        Ok(self.with_data(
            self.data
                .iter()
                .map(|c| resample_channel(c, 1.0 / factor, frames))
                .collect(),
        ))
    }
    /// WSOLA tempo adjustment; channel offsets are shared to preserve stereo phase.
    pub fn tempo(&self, factor: f64) -> Result<Self> {
        if !factor.is_finite() || factor <= 0.0 {
            return Err(Error::invalid("Tempo must be positive and finite"));
        }
        if factor == 1.0 {
            return Ok(self.clone());
        }
        let frames = (self.frames() as f64 / factor).round() as usize;
        Limits::default().check(frames, self.channels(), self.rate)?;
        let window = ((self.rate as f64 * 0.05).round() as usize).max(256) & !1;
        let hop = window / 2;
        let radius = window / 8;
        let mut out = vec![vec![0.0; frames]; self.channels()];
        let mut weights = vec![0.0; frames];
        let mut previous = 0;
        for (block, pos) in (0..frames).step_by(hop).enumerate() {
            let target = (block as f64 * hop as f64 * factor).round() as usize;
            if target >= self.frames() {
                break;
            }
            let mut chosen = target;
            if block > 0 && previous + hop < self.frames() {
                let template = previous + hop;
                let mut best = f64::NEG_INFINITY;
                for candidate in target.saturating_sub(radius)
                    ..=target.saturating_add(radius).min(self.frames() - 1)
                {
                    let count = hop
                        .min(self.frames() - template)
                        .min(self.frames() - candidate);
                    let (mut dot, mut energy) = (0.0, 0.0);
                    for i in (0..count).step_by(2) {
                        let a = self.data[0][template + i] as f64;
                        let b = self.data[0][candidate + i] as f64;
                        dot += a * b;
                        energy += b * b;
                    }
                    let score = if energy > 0.0 {
                        dot / energy.sqrt()
                    } else {
                        0.0
                    };
                    if score > best {
                        best = score;
                        chosen = candidate;
                    }
                }
            }
            let count = window.min(frames - pos).min(self.frames() - chosen);
            for i in 0..count {
                let w = (0.5 - 0.5 * (2.0 * PI * (i as f64 + 0.5) / window as f64).cos()) as f32;
                weights[pos + i] += w;
                for (c, dst) in out.iter_mut().enumerate() {
                    dst[pos + i] += self.data[c][chosen + i] * w;
                }
            }
            previous = chosen;
        }
        for c in &mut out {
            for (s, w) in c.iter_mut().zip(&weights) {
                if *w > 1e-12 {
                    *s /= *w;
                }
            }
        }
        Ok(self.with_data(out))
    }
    pub fn pitch(&self, semitones: f64) -> Result<Self> {
        if !semitones.is_finite() {
            return Err(Error::invalid("Pitch must be finite"));
        }
        let ratio = 2f64.powf(semitones / 12.0);
        if !ratio.is_finite() || ratio == 0.0 {
            return Err(Error::invalid("Pitch out of range"));
        }
        let stretched = self.tempo(1.0 / ratio)?;
        let mut shifted = stretched.speed(ratio)?;
        for c in &mut shifted.data {
            c.resize(self.frames(), 0.0);
        }
        Ok(shifted)
    }
}
