//! Parsing for opt-in tracker instrument keys.
use super::super::{
    ChipInstrument, ChipSample, ChipSource, MAX_MACRO_VALUES, MAX_SAMPLE_VALUES, Macro, MacroStep,
    Pitch, PitchTable, Wavetable,
};
use super::{Parsed, Pos};
use crate::ErrorCode;

pub(super) fn is_key(key: &str) -> bool {
    matches!(
        key,
        "wavetable"
            | "sample"
            | "sample-bits"
            | "sample-rate"
            | "sample-root"
            | "sample-loop"
            | "noise-period"
            | "quantize"
    ) || key.starts_with("macro-")
}
#[derive(Default)]
struct Lane {
    values: Option<Vec<i16>>,
    step: MacroStep,
    loop_start: Option<usize>,
    pos: Option<Pos>,
}

pub(super) fn apply(
    options: Vec<(String, String, Pos)>,
    instrument: &mut super::super::Instrument,
    wave_key: Option<Pos>,
) -> Parsed<()> {
    let first = options[0].2;
    let mut chip = ChipInstrument::default();
    let mut table = None;
    let mut sample = None;
    let mut noise = None;
    let (mut bits, mut rate, mut root, mut looping) = (4u8, 8000u32, Pitch::C4, false);
    let mut sample_control = None;
    let mut lanes: [Lane; 4] = std::array::from_fn(|_| Lane::default());
    for (key, value, pos) in options {
        let number = || integer(&value, pos);
        match key.as_str() {
            "wavetable" => {
                table = Some((
                    Wavetable::new(hex(&value, false, 256, pos)?).map_err(|e| pos.wrap(e))?,
                    pos,
                ))
            }
            "sample" => sample = Some((hex(&value, true, MAX_SAMPLE_VALUES, pos)?, pos)),
            "sample-bits" => {
                bits =
                    u8::try_from(number()?).map_err(|_| pos.invalid("Sample bits out of range"))?;
                sample_control = Some(pos);
            }
            "sample-rate" => {
                rate = number()?;
                sample_control = Some(pos);
            }
            "sample-root" => {
                root = Pitch::new(
                    u8::try_from(number()?).map_err(|_| pos.invalid("Sample root out of range"))?,
                )
                .map_err(|e| pos.wrap(e))?;
                sample_control = Some(pos);
            }
            "sample-loop" => {
                looping = match value.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return Err(pos.syntax("sample-loop must be on or off")),
                };
                sample_control = Some(pos);
            }
            "noise-period" => {
                noise = Some((
                    u8::try_from(number()?)
                        .map_err(|_| pos.invalid("Noise period out of range"))?,
                    pos,
                ))
            }
            "quantize" => {
                (chip.volume_4bit, chip.pitch_table) = match value.as_str() {
                    "none" => (false, None),
                    "volume" => (true, None),
                    "ntsc-pulse" => (true, Some(PitchTable::NtscPulse)),
                    "ntsc-triangle" => (true, Some(PitchTable::NtscTriangle)),
                    _ => return Err(pos.syntax("Unknown quantization model")),
                };
            }
            _ => {
                let key = key.strip_prefix("macro-").unwrap_or("");
                let (lane, field) = if let Some(k) = key.strip_suffix("-step") {
                    (k, 1)
                } else if let Some(k) = key.strip_suffix("-loop") {
                    (k, 2)
                } else {
                    (key, 0)
                };
                let lane = &mut lanes[match lane {
                    "volume" => 0,
                    "duty" => 1,
                    "arpeggio" => 2,
                    "pitch" => 3,
                    _ => return Err(pos.syntax("Unknown tracker instrument key")),
                }];
                lane.pos = Some(pos);
                match field {
                    0 => {
                        let values = value
                            .split(',')
                            .map(|v| {
                                v.parse::<i16>().map_err(|_| {
                                    pos.syntax("Macro values must be comma-separated integers")
                                })
                            })
                            .collect::<Parsed<Vec<_>>>()?;
                        if values.len() > MAX_MACRO_VALUES {
                            return Err(
                                pos.error(ErrorCode::LimitExceeded, "Macro exceeds 64 values")
                            );
                        }
                        lane.values = Some(values);
                    }
                    1 => {
                        let parts: Vec<_> = value.split(',').collect();
                        let small = |v| {
                            u16::try_from(integer(v, pos)?)
                                .map_err(|_| pos.invalid("Macro clock out of range"))
                        };
                        lane.step = match parts.as_slice() {
                            ["frames", frames, hz] => MacroStep::Frames {
                                frames: small(frames)?,
                                hz: small(hz)?,
                            },
                            ["ticks", ticks] => MacroStep::Ticks(small(ticks)?),
                            _ => {
                                return Err(
                                    pos.syntax("Macro step needs frames,COUNT,HZ or ticks,COUNT")
                                );
                            }
                        };
                    }
                    _ => {
                        lane.loop_start = if value == "hold" {
                            None
                        } else {
                            Some(number()? as usize)
                        }
                    }
                }
            }
        }
    }
    if usize::from(table.is_some()) + usize::from(sample.is_some()) + usize::from(noise.is_some())
        > 1
    {
        return Err(first.invalid("Select only one custom chip source"));
    }
    chip.source = if let Some((table, pos)) = table {
        if let Some(pos) = wave_key {
            return Err(pos.invalid("Wavetables do not use wave/duty/noise keys"));
        }
        if let Some(pos) = sample_control {
            return Err(pos.invalid("Sample controls require sample data"));
        }
        let _ = pos;
        ChipSource::Wavetable(table)
    } else if let Some((values, pos)) = sample {
        if let Some(pos) = wave_key {
            return Err(pos.invalid("PCM does not use wave/duty/noise keys"));
        }
        let sample = ChipSample {
            values,
            bits,
            sample_rate: rate,
            root,
            looping,
        };
        sample.validate().map_err(|e| pos.wrap(e))?;
        ChipSource::Sample(sample)
    } else {
        if let Some(pos) = sample_control {
            return Err(pos.invalid("Sample controls require sample data"));
        }
        if let Some((period, _)) = noise {
            ChipSource::Noise { period }
        } else {
            ChipSource::Waveform
        }
    };
    let mut parsed: [Option<Macro>; 4] = [None, None, None, None];
    for (slot, lane) in parsed.iter_mut().zip(lanes) {
        if let Some(pos) = lane.pos {
            let values = lane
                .values
                .ok_or_else(|| pos.invalid("Macro controls require a value sequence"))?;
            let m = Macro {
                values,
                step: lane.step,
                loop_start: lane.loop_start,
            };
            m.validate().map_err(|e| pos.wrap(e))?;
            *slot = Some(m);
        }
    }
    let [volume, duty, arpeggio, pitch] = parsed;
    chip.volume = volume;
    chip.duty = duty;
    chip.arpeggio = arpeggio;
    chip.pitch = pitch;
    chip.validate(instrument.waveform)
        .map_err(|e| first.wrap(e))?;
    instrument.chip = Some(chip);
    Ok(())
}
fn integer(value: &str, pos: Pos) -> Parsed<u32> {
    value
        .parse::<u32>()
        .map_err(|_| pos.syntax("Expected an unsigned integer"))
}
fn hex(value: &str, pairs: bool, maximum: usize, pos: Pos) -> Parsed<Vec<u8>> {
    let width = if pairs { 2 } else { 1 };
    if value.len() > maximum * width {
        return Err(pos.error(ErrorCode::LimitExceeded, "Chip data exceeds its size limit"));
    }
    if value.is_empty() || value.len() % width != 0 {
        return Err(pos.syntax("Expected complete hexadecimal values"));
    }
    let digits = value
        .bytes()
        .map(|b| match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            b'A'..=b'F' => Ok(b - b'A' + 10),
            _ => Err(pos.syntax("Chip data must be ASCII hexadecimal")),
        })
        .collect::<Parsed<Vec<_>>>()?;
    Ok(if pairs {
        digits.chunks_exact(2).map(|p| p[0] * 16 + p[1]).collect()
    } else {
        digits
    })
}
