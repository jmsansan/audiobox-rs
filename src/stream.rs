//! Incremental raw PCM I/O. Decoder input chunks may end in the middle of a frame.
use crate::{Audio, Endian, Error, Limits, Result, SampleFormat};

#[derive(Debug)]
pub struct PcmDecoder {
    format: SampleFormat,
    endian: Endian,
    channels: usize,
    rate: u32,
    limits: Limits,
    pending: Vec<u8>,
    frames: usize,
}
impl PcmDecoder {
    pub fn new(
        format: SampleFormat,
        endian: Endian,
        channels: usize,
        rate: u32,
        limits: Limits,
    ) -> Result<Self> {
        limits.check(0, channels, rate)?;
        Ok(Self {
            format,
            endian,
            channels,
            rate,
            limits,
            pending: Vec::new(),
            frames: 0,
        })
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<Audio> {
        let stride = self
            .channels
            .checked_mul(self.format.bytes())
            .ok_or_else(|| Error::invalid("PCM stride overflow"))?;
        let n = self
            .pending
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| Error::limit("PCM input size overflow"))?;
        let frames = n / stride;
        let total = self
            .frames
            .checked_add(frames)
            .ok_or_else(|| Error::limit("PCM frame overflow"))?;
        self.limits.check(total, self.channels, self.rate)?;
        let mut combined = std::mem::take(&mut self.pending);
        combined.extend_from_slice(bytes);
        let complete = frames * stride;
        let result = crate::pcm::decode_pcm(
            &combined[..complete],
            self.format,
            self.endian,
            self.channels,
            self.rate,
            &self.limits,
        )?;
        self.pending = combined[complete..].to_vec();
        self.frames = total;
        Ok(result)
    }
    pub fn finish(self) -> Result<()> {
        if self.pending.is_empty() {
            Ok(())
        } else {
            Err(Error::decode("Incomplete final PCM frame"))
        }
    }
}
#[derive(Debug)]
pub struct PcmEncoder {
    format: SampleFormat,
    endian: Endian,
    shape: Option<(usize, u32)>,
}
impl PcmEncoder {
    pub fn new(format: SampleFormat, endian: Endian) -> Self {
        Self {
            format,
            endian,
            shape: None,
        }
    }
    pub fn push(&mut self, audio: &Audio) -> Result<Vec<u8>> {
        let shape = (audio.channels(), audio.sample_rate());
        if self.shape.is_some_and(|s| s != shape) {
            return Err(Error::invalid("PCM stream shape changed"));
        }
        self.shape = Some(shape);
        crate::pcm::encode_pcm(audio, self.format, self.endian)
    }
}

/// Windowed-sinc resampling across chunk boundaries. Retains only filter history.
/// Call `finish` to drain the final filter tail using the last sample as the edge value.
pub struct Resampler {
    from_rate: u32,
    to_rate: u32,
    channels: usize,
    limits: Limits,
    pending: Vec<std::collections::VecDeque<f32>>,
    first: Vec<f32>,
    start: usize,
    inputs: usize,
    outputs: usize,
}
impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32, channels: usize, limits: Limits) -> Result<Self> {
        limits.check(0, channels, from_rate)?;
        limits.check(0, channels, to_rate)?;
        Ok(Self {
            from_rate,
            to_rate,
            channels,
            limits,
            pending: vec![std::collections::VecDeque::new(); channels],
            first: vec![0.0; channels],
            start: 0,
            inputs: 0,
            outputs: 0,
        })
    }
    pub fn push(&mut self, audio: &Audio) -> Result<Audio> {
        if audio.sample_rate() != self.from_rate || audio.channels() != self.channels {
            return Err(Error::invalid("Resampler input shape changed"));
        }
        let total = self
            .inputs
            .checked_add(audio.frames())
            .ok_or_else(|| Error::limit("Resampler frame count overflow"))?;
        self.limits.check(total, self.channels, self.from_rate)?;
        self.limits.check(
            (total as f64 * self.to_rate as f64 / self.from_rate as f64).round() as usize,
            self.channels,
            self.to_rate,
        )?;
        for (c, data) in self.pending.iter_mut().enumerate() {
            if self.inputs == 0 && audio.frames() > 0 {
                self.first[c] = audio.data[c][0];
            }
            data.extend(audio.data[c].iter().copied());
        }
        self.inputs = total;
        self.drain(false)
    }
    fn drain(&mut self, finish: bool) -> Result<Audio> {
        let ratio = self.to_rate as f64 / self.from_rate as f64;
        let half = 32.0 / (ratio.min(1.0) * 0.97);
        let target = (self.inputs as f64 * ratio).round() as usize;
        let mut data = vec![Vec::new(); self.channels];
        while self.outputs < target {
            let center = self.outputs as f64 / ratio;
            if !finish && center + half >= self.inputs as f64 {
                break;
            }
            for (c, dst) in data.iter_mut().enumerate() {
                let value = if self.from_rate == self.to_rate {
                    self.pending[c][self.outputs - self.start]
                } else {
                    crate::dsp::sinc_sample(center, ratio, |index| {
                        if index < 0 {
                            return self.first[c];
                        }
                        if index as usize >= self.inputs {
                            return self.pending[c].back().copied().unwrap_or(0.0);
                        }
                        self.pending[c][index as usize - self.start]
                    })
                };
                dst.push(value);
            }
            self.outputs += 1;
        }
        let keep = ((self.outputs as f64 / ratio - half).ceil().max(0.0) as usize)
            .min(self.inputs.saturating_sub(1));
        if keep > self.start {
            for c in &mut self.pending {
                c.drain(..keep - self.start);
            }
            self.start = keep;
        }
        Audio::decoded(data, self.to_rate, crate::DecodeInfo::default())
    }
    pub fn finish(mut self) -> Result<Audio> {
        self.drain(true)
    }
}
