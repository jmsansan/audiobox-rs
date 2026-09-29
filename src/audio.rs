use crate::{DecodeInfo, Error, Limits, Result, TimePosition};
use std::fmt;

/// Planar floating-point audio. All channels have the same length.
/// Samples may exceed ±1 to retain headroom; encoders clamp at quantization.
#[derive(Clone, Debug)]
pub struct Audio {
    pub(crate) data: Vec<Vec<f32>>,
    pub(crate) rate: u32,
    pub(crate) info: DecodeInfo,
}
impl Audio {
    pub fn new(data: Vec<Vec<f32>>, sample_rate: u32) -> Result<Self> {
        let frames = data.first().map_or(0, Vec::len);
        Limits::default().check(frames, data.len(), sample_rate)?;
        if data
            .iter()
            .any(|c| c.len() != frames || c.iter().any(|s| !s.is_finite()))
        {
            return Err(Error::invalid(
                "Channels must have equal lengths and finite samples",
            ));
        }
        Ok(Self {
            data,
            rate: sample_rate,
            info: DecodeInfo::default(),
        })
    }
    pub(crate) fn decoded(data: Vec<Vec<f32>>, rate: u32, info: DecodeInfo) -> Result<Self> {
        if data.is_empty()
            || rate == 0
            || data
                .iter()
                .any(|c| c.len() != data[0].len() || c.iter().any(|s| !s.is_finite()))
        {
            return Err(Error::decode("Invalid decoded PCM"));
        }
        Ok(Self { data, rate, info })
    }
    pub(crate) fn with_data(&self, data: Vec<Vec<f32>>) -> Self {
        Self {
            data,
            rate: self.rate,
            info: self.info.clone(),
        }
    }
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
    pub fn channels(&self) -> usize {
        self.data.len()
    }
    pub fn frames(&self) -> usize {
        self.data[0].len()
    }
    pub fn duration(&self) -> f64 {
        self.frames() as f64 / self.rate as f64
    }
    pub fn info(&self) -> &DecodeInfo {
        &self.info
    }
    pub fn channel_data(&self, channel: usize) -> Result<&[f32]> {
        self.data
            .get(channel)
            .map(Vec::as_slice)
            .ok_or_else(|| Error::invalid("Channel index out of range"))
    }
    pub fn all_channels(&self) -> &[Vec<f32>] {
        &self.data
    }
    pub fn to_interleaved(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.frames() * self.channels());
        for frame in 0..self.frames() {
            for channel in &self.data {
                out.push(channel[frame]);
            }
        }
        out
    }
    pub fn from_interleaved(samples: &[f32], channels: usize, rate: u32) -> Result<Self> {
        if channels == 0 || samples.len() % channels != 0 {
            return Err(Error::invalid("Incomplete interleaved frames"));
        }
        Limits::default().check(samples.len() / channels, channels, rate)?;
        let mut data = vec![vec![0.0; samples.len() / channels]; channels];
        for (i, frame) in samples.chunks_exact(channels).enumerate() {
            for (c, &v) in frame.iter().enumerate() {
                data[c][i] = v;
            }
        }
        Self::new(data, rate)
    }
    pub fn silence(duration: impl Into<TimePosition>, channels: usize, rate: u32) -> Result<Self> {
        let frames = duration.into().duration(rate)?;
        Limits::default().check(frames, channels, rate)?;
        Self::new(vec![vec![0.0; frames]; channels], rate)
    }
    pub fn cut(&self, from: impl Into<TimePosition>, to: impl Into<TimePosition>) -> Result<Self> {
        let start = from.into().index(self.rate, self.frames())?;
        let end = to.into().index(self.rate, self.frames())?.max(start);
        Ok(self.with_data(self.data.iter().map(|c| c[start..end].to_vec()).collect()))
    }
    pub fn remove(
        &self,
        from: impl Into<TimePosition>,
        to: impl Into<TimePosition>,
    ) -> Result<Self> {
        let start = from.into().index(self.rate, self.frames())?;
        let end = to.into().index(self.rate, self.frames())?.max(start);
        Ok(self.with_data(
            self.data
                .iter()
                .map(|c| c[..start].iter().chain(&c[end..]).copied().collect())
                .collect(),
        ))
    }
    pub(crate) fn same_shape(&self, other: &Self) -> Result<()> {
        if self.rate != other.rate || self.channels() != other.channels() {
            return Err(Error::invalid("Sample rates and channel counts must match"));
        }
        Ok(())
    }
    pub fn concat(&self, other: &Self) -> Result<Self> {
        self.same_shape(other)?;
        let frames = self
            .frames()
            .checked_add(other.frames())
            .ok_or_else(|| Error::limit("Frame count overflow"))?;
        Limits::default().check(frames, self.channels(), self.rate)?;
        Ok(self.with_data(
            self.data
                .iter()
                .zip(&other.data)
                .map(|(a, b)| a.iter().chain(b).copied().collect())
                .collect(),
        ))
    }
    pub fn pad(
        &self,
        before: impl Into<TimePosition>,
        after: impl Into<TimePosition>,
    ) -> Result<Self> {
        let before = before.into().duration(self.rate)?;
        let after = after.into().duration(self.rate)?;
        let frames = before
            .checked_add(self.frames())
            .and_then(|n| n.checked_add(after))
            .ok_or_else(|| Error::limit("Frame count overflow"))?;
        Limits::default().check(frames, self.channels(), self.rate)?;
        Ok(self.with_data(
            self.data
                .iter()
                .map(|c| {
                    let mut out = vec![0.0; frames];
                    out[before..before + c.len()].copy_from_slice(c);
                    out
                })
                .collect(),
        ))
    }
    pub fn reverse(&self) -> Self {
        self.with_data(
            self.data
                .iter()
                .map(|c| c.iter().rev().copied().collect())
                .collect(),
        )
    }
    pub fn mix(&self, overlay: &Self, offset: i64, gain: f32, extend: bool) -> Result<Self> {
        self.same_shape(overlay)?;
        if !gain.is_finite() {
            return Err(Error::invalid("Gain must be finite"));
        }
        let needed = (self.frames() as i128).max(offset as i128 + overlay.frames() as i128);
        let frames = if extend {
            usize::try_from(needed).map_err(|_| Error::limit("Frame count overflow"))?
        } else {
            self.frames()
        };
        Limits::default().check(frames, self.channels(), self.rate)?;
        let mut out = self.data.clone();
        for (c, dst) in out.iter_mut().enumerate() {
            dst.resize(frames, 0.0);
            for (i, &s) in overlay.data[c].iter().enumerate() {
                let at = offset as i128 + i as i128;
                if at >= 0 && at < frames as i128 {
                    dst[at as usize] += s * gain;
                }
            }
        }
        if out.iter().flatten().any(|s| !s.is_finite()) {
            return Err(Error::invalid("Mix overflow"));
        }
        Ok(self.with_data(out))
    }
    pub fn crossfade_to(&self, other: &Self, duration: impl Into<TimePosition>) -> Result<Self> {
        self.same_shape(other)?;
        let n = duration
            .into()
            .duration(self.rate)?
            .min(self.frames())
            .min(other.frames());
        let frames = self
            .frames()
            .checked_add(other.frames())
            .and_then(|v| v.checked_sub(n))
            .ok_or_else(|| Error::limit("Frame count overflow"))?;
        Limits::default().check(frames, self.channels(), self.rate)?;
        let mut data = Vec::new();
        for (a, b) in self.data.iter().zip(&other.data) {
            let mut out = a[..a.len() - n].to_vec();
            for i in 0..n {
                let t = i as f64 / n.saturating_sub(1).max(1) as f64 * std::f64::consts::FRAC_PI_2;
                out.push(a[a.len() - n + i] * t.cos() as f32 + b[i] * t.sin() as f32);
            }
            out.extend_from_slice(&b[n..]);
            data.push(out);
        }
        Ok(self.with_data(data))
    }
    pub fn to_mono(&self) -> Self {
        let mut mono = vec![0.0; self.frames()];
        for c in &self.data {
            for (v, s) in mono.iter_mut().zip(c) {
                *v += *s / self.channels() as f32;
            }
        }
        self.with_data(vec![mono])
    }
    pub fn to_stereo(&self) -> Self {
        if self.channels() == 2 {
            return self.clone();
        }
        let mono = self.to_mono();
        self.with_data(vec![mono.data[0].clone(), mono.data[0].clone()])
    }
    pub fn to_channels(&self, count: usize) -> Result<Self> {
        Limits::default().check(self.frames(), count, self.rate)?;
        match count {
            1 => Ok(self.to_mono()),
            2 => Ok(self.to_stereo()),
            _ => Ok(self.with_data(
                (0..count)
                    .map(|c| self.data[c % self.channels()].clone())
                    .collect(),
            )),
        }
    }
    pub fn map_channels(&self, mapping: &[usize]) -> Result<Self> {
        Limits::default().check(self.frames(), mapping.len(), self.rate)?;
        let data = mapping
            .iter()
            .map(|&i| self.channel_data(i).map(<[f32]>::to_vec))
            .collect::<Result<_>>()?;
        Ok(self.with_data(data))
    }
    pub fn pan(&self, position: f64) -> Result<Self> {
        if !position.is_finite() {
            return Err(Error::invalid("Pan must be finite"));
        }
        let angle = (position.clamp(-1.0, 1.0) + 1.0) * std::f64::consts::FRAC_PI_4;
        let mut out = self.to_stereo();
        for s in &mut out.data[0] {
            *s *= angle.cos() as f32;
        }
        for s in &mut out.data[1] {
            *s *= angle.sin() as f32;
        }
        Ok(out)
    }
    pub fn split(&self) -> Vec<Self> {
        self.data
            .iter()
            .map(|c| self.with_data(vec![c.clone()]))
            .collect()
    }
    pub fn merge(parts: &[Self]) -> Result<Self> {
        let first = parts
            .first()
            .ok_or_else(|| Error::invalid("No audio to merge"))?;
        if parts.iter().any(|p| p.rate != first.rate) {
            return Err(Error::invalid("Sample rates must match"));
        }
        let frames = parts.iter().map(Self::frames).max().unwrap_or(0);
        let channels = parts
            .iter()
            .try_fold(0usize, |n, p| n.checked_add(p.channels()))
            .ok_or_else(|| Error::limit("Channel count overflow"))?;
        Limits::default().check(frames, channels, first.rate)?;
        let data = parts
            .iter()
            .flat_map(|p| {
                p.data.iter().map(|c| {
                    let mut out = c.clone();
                    out.resize(frames, 0.0);
                    out
                })
            })
            .collect();
        Ok(first.with_data(data))
    }
    pub fn duration_formatted(&self) -> String {
        let ms = (self.duration() * 1000.0).round() as u64;
        let h = ms / 3600000;
        let m = ms / 60000 % 60;
        let s = ms / 1000 % 60;
        if h > 0 {
            format!("{h}:{m:02}:{s:02}.{:03}", ms % 1000)
        } else {
            format!("{m}:{s:02}.{:03}", ms % 1000)
        }
    }
}
impl fmt::Display for Audio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Audio({}, {} Hz, {}ch)",
            self.duration_formatted(),
            self.rate,
            self.channels()
        )
    }
}
