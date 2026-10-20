use super::{Effect, FIELDS, ParseError};
use crate::ErrorCode;
use crate::synth::{Duty, NoiseMode, Waveform};

/// The longest text an effect may be written in, in bytes.
const MAX_TEXT_BYTES: usize = 64 * 1024;

fn error(line: usize, column: usize, code: ErrorCode, message: impl Into<String>) -> ParseError {
    ParseError {
        line,
        column,
        code,
        message: message.into(),
    }
}

/// A `key=value` word with the positions of its key and its value.
struct Pair<'a> {
    key: &'a str,
    value: &'a str,
    line: usize,
    column: usize,
    value_column: usize,
}

fn pairs(text: &str) -> Result<Vec<Pair<'_>>, ParseError> {
    let mut out = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let content = raw.split(';').next().unwrap_or("");
        let mut start = None;
        let mut column = 0;
        // A trailing space flushes the last word.
        for (offset, c) in content.char_indices().chain([(content.len(), ' ')]) {
            column += 1;
            match (c.is_whitespace(), start) {
                (false, None) => start = Some((offset, column)),
                (true, Some((from, word_column))) => {
                    start = None;
                    let word = &content[from..offset];
                    let (key, value) = word.split_once('=').ok_or_else(|| {
                        error(
                            line,
                            word_column,
                            ErrorCode::Syntax,
                            format!("Expected key=value, found {word}"),
                        )
                    })?;
                    out.push(Pair {
                        key,
                        value,
                        line,
                        column: word_column,
                        value_column: word_column + key.chars().count() + 1,
                    });
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

pub(super) fn parse(text: &str) -> Result<Effect, ParseError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(error(
            1,
            1,
            ErrorCode::LimitExceeded,
            "Effect text exceeds 64 KiB",
        ));
    }
    let mut effect = Effect::default();
    let mut seen: Vec<&str> = Vec::new();
    let (mut wave, mut duty, mut noise, mut duty_sweep) = (None, None, None, None);
    let mut envelope = (1, 1);
    for pair in pairs(text)? {
        let at = |code, message: String| error(pair.line, pair.value_column, code, message);
        if seen.contains(&pair.key) {
            return Err(error(
                pair.line,
                pair.column,
                ErrorCode::Syntax,
                format!("{} is set twice", pair.key),
            ));
        }
        match pair.key {
            "wave" => match pair.value {
                "pulse" | "triangle" | "saw" | "noise" => wave = Some(pair.value),
                other => {
                    return Err(at(
                        ErrorCode::Syntax,
                        format!("wave must be pulse, triangle, saw or noise, not {other}"),
                    ));
                }
            },
            "duty" => {
                let value = pair
                    .value
                    .parse::<Duty>()
                    .map_err(|e| at(e.code, e.message))?;
                duty = Some((value, pair.line, pair.column));
            }
            "noise" => {
                let mode = match pair.value {
                    "long" => NoiseMode::Long,
                    "short" => NoiseMode::Short,
                    other => {
                        return Err(at(
                            ErrorCode::Syntax,
                            format!("noise must be long or short, not {other}"),
                        ));
                    }
                };
                noise = Some((mode, pair.line, pair.column));
            }
            key => {
                let field = FIELDS.iter().find(|f| f.key == key).ok_or_else(|| {
                    error(
                        pair.line,
                        pair.column,
                        ErrorCode::Syntax,
                        format!("Unknown key {key}"),
                    )
                })?;
                let value = pair.value.parse::<f64>().map_err(|_| {
                    at(
                        ErrorCode::Syntax,
                        format!("{key} needs a number, not {}", pair.value),
                    )
                })?;
                field.check(value).map_err(|e| at(e.code, e.message))?;
                (field.set)(&mut effect, value);
                if matches!(key, "attack" | "sustain" | "decay") {
                    envelope = (pair.line, pair.column);
                }
                if key == "duty-sweep" {
                    duty_sweep = Some((pair.line, pair.column));
                }
            }
        }
        seen.push(pair.key);
    }
    let wave = wave.unwrap_or("pulse");
    let misplaced = |what: &str, needs: &str, (line, column): (usize, usize)| {
        error(
            line,
            column,
            ErrorCode::InvalidArgument,
            format!("{what} needs wave={needs}"),
        )
    };
    if let Some((_, line, column)) = duty.filter(|_| wave != "pulse") {
        return Err(misplaced("duty", "pulse", (line, column)));
    }
    if let Some((_, line, column)) = noise.filter(|_| wave != "noise") {
        return Err(misplaced("noise", "noise", (line, column)));
    }
    if let Some(position) = duty_sweep.filter(|_| wave != "pulse" && effect.duty_sweep != 0.0) {
        return Err(misplaced("duty-sweep", "pulse", position));
    }
    effect.waveform = match wave {
        "pulse" => Waveform::Pulse(duty.map_or(Duty::Half, |(d, ..)| d)),
        "triangle" => Waveform::Triangle,
        "saw" => Waveform::Sawtooth,
        _ => Waveform::Noise(noise.map_or(NoiseMode::Long, |(m, ..)| m)),
    };
    effect
        .validate()
        .map_err(|e| error(envelope.0, envelope.1, e.code, e.message))?;
    Ok(effect)
}
