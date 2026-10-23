use super::{Choice, Definition, EventBank};
use crate::mixer::{Attenuation, DistanceModel, Emitter, Fade, Instances, Loop, OverLimit, Play};
use crate::{Error, ErrorCode};
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

/// Stable error code and one-based line/character column of an offending token.
/// Syntax errors use `Syntax`, out-of-range values/unresolved references use
/// `InvalidArgument`, and text/event/source-entry bounds use `LimitExceeded`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub code: ErrorCode,
    pub message: String,
}
impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}
impl std::error::Error for ParseError {}
impl From<ParseError> for Error {
    fn from(e: ParseError) -> Self {
        Self::new(e.code, e.to_string())
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Location {
    line: usize,
    column: usize,
}
impl Location {
    fn error(self, code: ErrorCode, message: impl Into<String>) -> ParseError {
        ParseError {
            line: self.line,
            column: self.column,
            code,
            message: message.into(),
        }
    }
    pub(super) fn invalid(self, message: impl Into<String>) -> ParseError {
        self.error(ErrorCode::InvalidArgument, message)
    }
    fn syntax(self, message: impl Into<String>) -> ParseError {
        self.error(ErrorCode::Syntax, message)
    }
    fn number<T: FromStr>(self, s: &str) -> Result<T, ParseError> {
        s.parse().map_err(|_| self.syntax("Invalid number"))
    }
    fn unsigned(self, s: &str) -> Result<u64, ParseError> {
        use std::num::IntErrorKind;
        s.parse::<u64>().map_err(|e| {
            let negative = s.strip_prefix('-').is_some_and(|digits| {
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            });
            if negative
                || matches!(
                    e.kind(),
                    IntErrorKind::PosOverflow | IntErrorKind::NegOverflow
                )
            {
                self.invalid("Integer outside the supported range")
            } else {
                self.syntax("Invalid integer")
            }
        })
    }
    fn float(self, s: &str, min: f32, max: f32) -> Result<f32, ParseError> {
        let f: f32 = self.number(s)?;
        if !f.is_finite() || f < min || f > max {
            return Err(self.invalid("Number outside the supported range"));
        }
        Ok(f)
    }
    fn vector<const N: usize>(self, s: &str) -> Result<[f32; N], ParseError> {
        let mut parts = s.split(',');
        let mut result = [0.0; N];
        for value in &mut result {
            *value = self.float(
                parts
                    .next()
                    .ok_or_else(|| self.syntax("Missing vector coordinate"))?,
                -f32::MAX,
                f32::MAX,
            )?;
        }
        if parts.next().is_some() {
            return Err(self.syntax("Too many vector coordinates"));
        }
        Ok(result)
    }
    fn range(self, s: &str, min: f32, max: f32) -> Result<[f32; 2], ParseError> {
        let range = if s.contains(',') {
            self.vector(s)?
        } else {
            [self.float(s, min, max)?; 2]
        };
        if range[0] < min || range[1] > max || range[0] > range[1] {
            return Err(self.invalid("Range needs min <= max within supported bounds"));
        }
        Ok(range)
    }
}
pub(super) fn name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
}
pub(super) fn bank(text: &str) -> Result<EventBank, ParseError> {
    let origin = Location { line: 1, column: 1 };
    if text.len() > 1024 * 1024 {
        return Err(origin.error(ErrorCode::LimitExceeded, "Event text exceeds 1 MiB"));
    }
    let mut definitions = Vec::new();
    let mut names = BTreeSet::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("");
        // Advancing the suffix keeps both Unicode whitespace and character columns correct.
        let mut suffix = line;
        let mut tokens = Vec::new();
        let mut column = 1;
        for token in line.split_whitespace() {
            let offset = suffix.find(token).expect("token in suffix");
            let byte = line.len() - suffix.len() + offset;
            column += suffix[..offset].chars().count();
            tokens.push((
                token,
                Location {
                    line: i + 1,
                    column,
                },
            ));
            suffix = &line[byte + token.len()..];
            column += token.chars().count();
        }
        if tokens.is_empty() {
            continue;
        }
        let (head, location) = tokens[0];
        if head != "event" {
            return Err(location.syntax("Expected event"));
        }
        let &(name, name_location) = tokens
            .get(1)
            .ok_or_else(|| location.syntax("Missing event name"))?;
        if !name_ok(name) || !names.insert(name) {
            return Err(name_location.syntax("Invalid or duplicate event name"));
        }
        if definitions.len() == 1024 {
            return Err(location.error(
                ErrorCode::LimitExceeded,
                "At most 1024 events are supported",
            ));
        }
        let mut d = Definition {
            name: name.to_owned(),
            sources: Vec::new(),
            choice: Choice::Random,
            gain: [1.0; 2],
            pitch: [0.0; 2],
            bus: "master".into(),
            play: Play {
                instances: Some(Instances::new(0)),
                ..Play::default()
            },
            source_location: name_location,
            bus_location: name_location,
            loop_location: name_location,
        };
        let mut seen = BTreeSet::new();
        let mut emitter = Emitter::default();
        let mut emitter_location = None;
        let mut spatial_detail = None;
        let mut position_location = None;
        for &(token, at) in &tokens[2..] {
            let (key, value) = token
                .split_once('=')
                .ok_or_else(|| at.syntax("Expected key=value"))?;
            if value.is_empty() || !seen.insert(key) {
                return Err(at.syntax("Empty value or repeated property"));
            }
            match key {
                "sources" => {
                    for source in value.split(',') {
                        if !name_ok(source) {
                            return Err(at.syntax("Invalid source name"));
                        }
                        if d.sources.len() == 64 {
                            return Err(at.error(
                                ErrorCode::LimitExceeded,
                                "At most 64 source entries per event",
                            ));
                        }
                        d.sources.push(source.to_owned());
                    }
                    d.source_location = at;
                }
                "choice" => {
                    d.choice = match value {
                        "random" => Choice::Random,
                        "shuffle" => Choice::Shuffle,
                        "sequence" => Choice::Sequence,
                        _ => return Err(at.syntax("Expected random, shuffle or sequence")),
                    }
                }
                "gain" => d.gain = at.range(value, 0.0, f32::MAX)?,
                "pitch" => d.pitch = at.range(value, -48.0, 48.0)?,
                "bus" => {
                    if !name_ok(value) {
                        return Err(at.syntax("Invalid bus name"));
                    }
                    d.bus = value.into();
                    d.bus_location = at;
                }
                "priority" => {
                    let n = at.unsigned(value)?;
                    d.play.priority =
                        u8::try_from(n).map_err(|_| at.invalid("Priority must be 0..=255"))?;
                }
                "max-instances" => {
                    let n = at.unsigned(value)?;
                    d.play.instances.as_mut().unwrap().max = u16::try_from(n)
                        .map_err(|_| at.invalid("Instance maximum must be 0..=65535"))?;
                }
                "min-gap" => {
                    let gap: f64 = at.number(value)?;
                    if !gap.is_finite() || !(0.0..=60.0).contains(&gap) {
                        return Err(at.invalid("Minimum gap must be 0..=60 seconds"));
                    }
                    d.play.instances.as_mut().unwrap().min_gap = gap;
                }
                "policy" => {
                    d.play.instances.as_mut().unwrap().policy = match value {
                        "refuse" => OverLimit::Refuse,
                        "oldest" => OverLimit::StopOldest,
                        "quietest" => OverLimit::StopQuietest,
                        _ => return Err(at.syntax("Expected refuse, oldest or quietest")),
                    }
                }
                "loop" => {
                    d.play.looping = match value {
                        "off" => Loop::Off,
                        "whole" => Loop::Whole,
                        _ => {
                            let (a, b) = value
                                .split_once(',')
                                .ok_or_else(|| at.syntax("Expected off, whole or start,end"))?;
                            let start = at.unsigned(a)?;
                            let end = at.unsigned(b)?;
                            if start >= end {
                                return Err(at.invalid("Loop needs start < end"));
                            }
                            Loop::Region { start, end }
                        }
                    };
                    d.loop_location = at;
                }
                "pan" => d.play.pan = at.float(value, -1.0, 1.0)?,
                "rate" => d.play.rate = at.float(value, 1.0 / 16.0, 8.0)?,
                "fade-in" => d.play.fade_in = Fade::linear(at.float(value, 0.0, 60.0)? as f64),
                "position" => {
                    d.play.position = Some(at.vector(value)?);
                    position_location = Some(at);
                }
                "emitter" => {
                    emitter.position = at.vector(value)?;
                    emitter_location = Some(at);
                }
                "velocity" => {
                    emitter.velocity = at.vector(value)?;
                    spatial_detail = Some(at);
                }
                "attenuation" => {
                    let (model, rest) = value
                        .split_once(',')
                        .ok_or_else(|| at.syntax("Expected model,min,max,rolloff"))?;
                    let model = match model {
                        "linear" => DistanceModel::Linear,
                        "inverse" => DistanceModel::Inverse,
                        "exponential" => DistanceModel::Exponential,
                        _ => return Err(at.syntax("Unknown distance model")),
                    };
                    let [min_distance, max_distance, rolloff] = at.vector(rest)?;
                    emitter.attenuation = Attenuation {
                        model,
                        min_distance,
                        max_distance,
                        rolloff,
                    };
                    emitter
                        .attenuation
                        .validate()
                        .map_err(|e| at.invalid(e.message))?;
                    spatial_detail = Some(at);
                }
                _ => return Err(at.syntax(format!("Unknown event property: {key}"))),
            }
        }
        if d.sources.is_empty() {
            return Err(name_location.syntax("Event needs sources"));
        }
        if let Some(at) = emitter_location {
            if let Some(pos_at) = position_location {
                return Err(pos_at.invalid("Choose 2D position or a 3D emitter"));
            }
            emitter.validate().map_err(|e| at.invalid(e.message))?;
            d.play.emitter = Some(emitter);
        } else if let Some(at) = spatial_detail {
            return Err(at.invalid("Velocity and attenuation require emitter"));
        }
        definitions.push(d);
    }
    Ok(EventBank { definitions })
}
