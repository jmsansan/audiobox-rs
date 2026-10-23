use super::{Parsed, Pos};
use crate::synth::{Envelope, FilterMode, Instrument, Lfo, LfoRate, Synthwave, VoiceFilter};
use std::collections::BTreeSet;

pub(super) fn is_key(key: &str) -> bool {
    matches!(
        key,
        "synthwave"
            | "unison"
            | "detune"
            | "spread"
            | "pan"
            | "voices"
            | "glide"
            | "legato"
            | "filter"
            | "cutoff"
            | "resonance"
            | "keytrack"
            | "filter-adsr"
            | "filter-env"
    ) || key.starts_with("lfo-")
}
fn number(s: &str, pos: Pos) -> Parsed<i64> {
    use std::num::IntErrorKind;
    s.parse::<i64>().map_err(|e| match e.kind() {
        IntErrorKind::PosOverflow | IntErrorKind::NegOverflow => {
            pos.invalid("Integer out of range")
        }
        _ => pos.syntax("Expected integer"),
    })
}
fn bounded(s: &str, min: i64, max: i64, pos: Pos) -> Parsed<i64> {
    let n = number(s, pos)?;
    if n < min || n > max {
        return Err(pos.invalid(format!("Value must be {min}..={max}")));
    }
    Ok(n)
}
fn switch(s: &str, pos: Pos) -> Parsed<bool> {
    match s {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(pos.syntax("Expected on or off")),
    }
}
fn lfo(s: &str, pos: Pos, max: i64) -> Parsed<Lfo> {
    let parts: Vec<_> = s.split(',').collect();
    let (rate, depth) = match parts.as_slice() {
        [hz, depth] => (
            LfoRate::MilliHertz(bounded(hz, 1, 20000, pos)? as u32),
            *depth,
        ),
        ["beats", num, den, depth] => (
            LfoRate::Beats {
                numerator: bounded(num, 1, 64, pos)? as u16,
                denominator: bounded(den, 1, 64, pos)? as u16,
            },
            *depth,
        ),
        _ => {
            return Err(
                pos.syntax("LFO needs millihertz,depth or beats,numerator,denominator,depth")
            );
        }
    };
    Ok(Lfo {
        rate,
        depth: bounded(depth, 0, max, pos)? as u16,
    })
}
pub(super) fn apply(
    options: Vec<(String, String, Pos)>,
    instrument: &mut Instrument,
) -> Parsed<()> {
    let first = options[0].2;
    let mut wide = Synthwave::default();
    let mut filter = VoiceFilter::default();
    let mut filter_mode = None;
    let mut filter_option = None;
    let mut seen = BTreeSet::new();
    for (key, value, pos) in options {
        if !seen.insert(key.clone()) {
            return Err(pos.syntax("Duplicate synthwave key"));
        }
        let range = |min, max| bounded(&value, min, max, pos);
        match key.as_str() {
            "synthwave" => {
                if value != "on" {
                    return Err(pos.syntax("Expected synthwave=on"));
                }
            }
            "unison" => wide.unison = range(1, 8)? as u8,
            "detune" => wide.detune = range(0, 100)? as u16,
            "spread" => wide.spread = range(0, 100)? as u8,
            "pan" => wide.pan = range(-100, 100)? as i8,
            "voices" => wide.voices = range(1, 16)? as u8,
            "glide" => wide.glide_ms = range(0, 60000)? as u32,
            "legato" => wide.legato = switch(&value, pos)?,
            "filter" => {
                filter_mode = Some(match value.as_str() {
                    "lowpass" => FilterMode::LowPass,
                    "highpass" => FilterMode::HighPass,
                    "bandpass" => FilterMode::BandPass,
                    _ => return Err(pos.syntax("Expected lowpass, highpass or bandpass")),
                })
            }
            "cutoff" => {
                filter.cutoff_hz = range(1, 96000)? as u32;
                filter_option = Some(pos);
            }
            "resonance" => {
                filter.resonance = range(50, 1000)? as u16;
                filter_option = Some(pos);
            }
            "keytrack" => {
                filter.keytrack = range(0, 200)? as u16;
                filter_option = Some(pos);
            }
            "filter-env" => {
                filter.envelope_cents = range(-9600, 9600)? as i16;
                filter_option = Some(pos);
            }
            "filter-adsr" => {
                let parts: Vec<_> = value.split(',').collect();
                let [a, d, s, r] = parts.as_slice() else {
                    return Err(pos.syntax("filter-adsr needs attack,decay,sustain,release"));
                };
                filter.envelope = Envelope {
                    attack_ms: bounded(a, 0, 60000, pos)? as u32,
                    decay_ms: bounded(d, 0, 60000, pos)? as u32,
                    sustain: bounded(s, 0, 100, pos)? as u8,
                    release_ms: bounded(r, 0, 60000, pos)? as u32,
                };
                filter_option = Some(pos);
            }
            "lfo-pitch" => wide.pitch_lfo = Some(lfo(&value, pos, 2400)?),
            "lfo-cutoff" => {
                wide.cutoff_lfo = Some(lfo(&value, pos, 9600)?);
                filter_option = Some(pos);
            }
            "lfo-amplitude" => wide.amplitude_lfo = Some(lfo(&value, pos, 100)?),
            "lfo-pan" => wide.pan_lfo = Some(lfo(&value, pos, 100)?),
            _ => return Err(pos.syntax("Unknown synthwave key")),
        }
    }
    if let Some(mode) = filter_mode {
        filter.mode = mode;
        wide.filter = Some(filter);
    } else if let Some(pos) = filter_option {
        return Err(pos.invalid("Filter parameters need filter=MODE"));
    }
    wide.validate().map_err(|e| first.wrap(e))?;
    instrument.synthwave = Some(wide);
    Ok(())
}
