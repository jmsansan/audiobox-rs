//! The `fm NAME` … `end` block. Kept line-driven so the score parser can adopt it.
use super::{Cursor, Parsed, Pos};
use crate::ErrorCode;
use crate::synth::fm::{
    Fm, FmCurve, FmFrequency, FmLfo, FmLfoWave, FmOperator, FmPatch, FmRouting, MAX_FM_TEXT_BYTES,
    MAX_FM_UNISON_VOICES,
};

struct Pair<'a> {
    key: &'a str,
    pos: Pos,
    value: &'a str,
    value_pos: Pos,
}
impl Pair<'_> {
    fn int(&self, min: i64, max: i64) -> Parsed<i64> {
        int(self.key, self.value, self.value_pos, min, max)
    }
    fn switch(&self) -> Parsed<bool> {
        match self.value {
            "on" => Ok(true),
            "off" => Ok(false),
            other => Err(self
                .value_pos
                .syntax(format!("{} must be on or off, not {other}", self.key))),
        }
    }
    /// Comma-separated parts with the position of each.
    fn parts(&self) -> Vec<(&str, Pos)> {
        let mut column = self.value_pos.column;
        self.value
            .split(',')
            .map(|part| {
                let pos = Pos {
                    column,
                    ..self.value_pos
                };
                column += part.chars().count() + 1;
                (part, pos)
            })
            .collect()
    }
    fn list<const N: usize>(&self, min: i64, max: i64) -> Parsed<[i64; N]> {
        let parts = self.parts();
        if parts.len() != N {
            return Err(self.value_pos.syntax(format!(
                "{} needs {N} comma-separated values, not {}",
                self.key,
                parts.len()
            )));
        }
        let mut out = [0; N];
        for (slot, (part, pos)) in out.iter_mut().zip(parts) {
            *slot = int(self.key, part, pos, min, max)?;
        }
        Ok(out)
    }
}

fn int(key: &str, value: &str, pos: Pos, min: i64, max: i64) -> Parsed<i64> {
    let n: i64 = value
        .parse()
        .map_err(|_| pos.syntax(format!("{key} needs an integer, not {value}")))?;
    if n < min || n > max {
        return Err(pos.invalid(format!("{key} must be {min}..={max}, not {n}")));
    }
    Ok(n)
}

fn pairs<'a>(cursor: &mut Cursor<'a>) -> Parsed<Vec<Pair<'a>>> {
    let mut out: Vec<Pair> = Vec::new();
    loop {
        let (pos, word) = cursor.word();
        if word.is_empty() {
            return Ok(out);
        }
        let (key, value) = word
            .split_once('=')
            .ok_or_else(|| pos.syntax(format!("Expected key=value, found {word}")))?;
        if out.iter().any(|p| p.key == key) {
            return Err(pos.syntax(format!("{key} is set twice")));
        }
        let value_pos = Pos {
            column: pos.column + key.chars().count() + 1,
            ..pos
        };
        out.push(Pair {
            key,
            pos,
            value,
            value_pos,
        });
    }
}

fn envelope(pair: &Pair, target: &mut [u8; 4]) -> Parsed<()> {
    *target = pair.list::<4>(0, 99)?.map(|v| v as u8);
    Ok(())
}

fn frequency(pair: &Pair) -> Parsed<FmFrequency> {
    let parts = pair.parts();
    if parts.len() > 2 {
        return Err(pair
            .value_pos
            .syntax(format!("{} takes a value and an optional fine", pair.key)));
    }
    let (first, first_pos) = parts[0];
    let fine = match parts.get(1) {
        Some((part, pos)) => int("fine", part, *pos, 0, 99)? as u8,
        None => 0,
    };
    if pair.key == "ratio" {
        let coarse = match first {
            "0.5" => 0,
            _ => first
                .parse::<u8>()
                .ok()
                .filter(|c| (1..=31).contains(c))
                .ok_or_else(|| {
                    first_pos.invalid(format!("ratio must be 0.5 or 1..=31, not {first}"))
                })?,
        };
        Ok(FmFrequency::Ratio { coarse, fine })
    } else {
        let decade = ["1", "10", "100", "1000"]
            .iter()
            .position(|d| *d == first)
            .ok_or_else(|| {
                first_pos.invalid(format!("fixed must be 1, 10, 100 or 1000 Hz, not {first}"))
            })?;
        Ok(FmFrequency::Fixed {
            decade: decade as u8,
            fine,
        })
    }
}

fn named<T: Copy>(names: &[(&str, T)], key: &str, value: &str, pos: Pos) -> Parsed<T> {
    names
        .iter()
        .find(|(name, _)| *name == value)
        .map(|(_, v)| *v)
        .ok_or_else(|| {
            let all: Vec<_> = names.iter().map(|(name, _)| *name).collect();
            pos.syntax(format!(
                "{key} must be one of {}, not {value}",
                all.join(", ")
            ))
        })
}

fn unknown(pair: &Pair, line: &str) -> super::ParseError {
    pair.pos.syntax(format!("Unknown {line} key {}", pair.key))
}

/// Builds one patch from its lines. `header` takes the text after `fm`; `line` takes
/// each following line and returns the finished patch at `end`.
pub(in crate::synth) struct Block {
    patch: FmPatch,
    start: Pos,
    operators: [Option<Pos>; 6],
    pitch: bool,
    lfo: bool,
    voices: Option<Pos>,
    unison: Option<Pos>,
    mono: Option<Pos>,
}
impl Block {
    pub(in crate::synth) fn header(start: Pos, cursor: &mut Cursor) -> Parsed<Self> {
        cursor.skip_space();
        let name = cursor.name("an fm patch")?;
        if cursor.peek().is_some_and(|c| !c.is_whitespace()) {
            return Err(cursor
                .pos()
                .syntax("A name takes ASCII letters, digits, _ and -"));
        }
        let mut voice = Fm {
            operators: [None; 6],
            ..Fm::default()
        };
        let (mut volume, mut voices, mut unison, mut mono) = (15, None, None, None);
        for pair in pairs(cursor)? {
            match pair.key {
                "algorithm" => voice.algorithm = pair.int(1, 32)? as u8,
                "feedback" => voice.feedback = pair.int(0, 7)? as u8,
                "key-sync" => voice.key_sync = pair.switch()?,
                "transpose" => voice.transpose = pair.int(-24, 24)? as i8,
                "volume" => volume = pair.int(0, 15)? as u8,
                "voices" => {
                    voice.voices = pair.int(1, 16)? as u8;
                    voices = Some(pair.value_pos);
                }
                "unison" => {
                    voice.unison = pair.int(1, 8)? as u8;
                    unison = Some(pair.value_pos);
                }
                "unison-detune" => voice.unison_detune = pair.int(0, 100)? as u16,
                "oversample" => voice.oversample = pair.int(1, 2)? as u8,
                "spread" => voice.spread = pair.int(0, 100)? as u8,
                "pan" => voice.pan = pair.int(-100, 100)? as i8,
                "glide" => {
                    voice.glide_ms = pair.int(0, 60000)? as u32;
                    if voice.glide_ms > 0 {
                        mono = Some(pair.pos);
                    }
                }
                "legato" => {
                    voice.legato = pair.switch()?;
                    if voice.legato {
                        mono = Some(pair.pos);
                    }
                }
                _ => return Err(unknown(&pair, "fm")),
            }
        }
        Ok(Self {
            patch: FmPatch {
                name,
                volume,
                voice,
            },
            start,
            operators: [None; 6],
            pitch: false,
            lfo: false,
            voices,
            unison,
            mono,
        })
    }

    pub(in crate::synth) fn line(
        &mut self,
        keyword: &str,
        pos: Pos,
        cursor: &mut Cursor,
    ) -> Parsed<bool> {
        match keyword {
            "op" => self.operator(cursor)?,
            "pitch" => {
                if std::mem::replace(&mut self.pitch, true) {
                    return Err(pos.syntax("pitch is set twice"));
                }
                let envelope_ref = &mut self.patch.voice.pitch_envelope;
                for pair in pairs(cursor)? {
                    match pair.key {
                        "rates" => envelope(&pair, &mut envelope_ref.rates)?,
                        "levels" => envelope(&pair, &mut envelope_ref.levels)?,
                        _ => return Err(unknown(&pair, "pitch")),
                    }
                }
            }
            "lfo" => {
                if std::mem::replace(&mut self.lfo, true) {
                    return Err(pos.syntax("lfo is set twice"));
                }
                self.patch.voice.lfo = lfo(cursor)?;
            }
            "end" => {
                cursor.skip_space();
                if cursor.peek().is_some() {
                    return Err(cursor.pos().syntax("Unexpected text after end"));
                }
                return Ok(true);
            }
            other => {
                return Err(pos.syntax(format!(
                    "Expected op, pitch, lfo or end inside an fm block, found {other}"
                )));
            }
        }
        Ok(false)
    }

    fn operator(&mut self, cursor: &mut Cursor) -> Parsed<()> {
        let (number_pos, word) = cursor.word();
        let number = int("op", word, number_pos, 1, 6)? as usize;
        if self.operators[number - 1].is_some() {
            return Err(number_pos.syntax(format!("op {number} is set twice")));
        }
        self.operators[number - 1] = Some(number_pos);
        let mut o = FmOperator::default();
        let mut source: Option<&str> = None;
        for pair in pairs(cursor)? {
            match pair.key {
                "ratio" | "fixed" => {
                    if let Some(other) = source.replace(pair.key) {
                        return Err(pair.pos.syntax(format!(
                            "An operator takes ratio or fixed, not both ({other} is already set)"
                        )));
                    }
                    o.frequency = frequency(&pair)?;
                }
                "detune" => o.detune = pair.int(-7, 7)? as i8,
                "level" => o.level = pair.int(0, 99)? as u8,
                "rates" => envelope(&pair, &mut o.envelope.rates)?,
                "levels" => envelope(&pair, &mut o.envelope.levels)?,
                "scaling" => {
                    let [b, l, r] = pair.list::<3>(0, 99)?;
                    o.scaling.break_point = b as u8;
                    o.scaling.left_depth = l as u8;
                    o.scaling.right_depth = r as u8;
                }
                "curves" => {
                    let parts = pair.parts();
                    if parts.len() != 2 {
                        return Err(pair.value_pos.syntax("curves needs LEFT,RIGHT"));
                    }
                    o.scaling.left_curve =
                        named(&FmCurve::NAMES, "curves", parts[0].0, parts[0].1)?;
                    o.scaling.right_curve =
                        named(&FmCurve::NAMES, "curves", parts[1].0, parts[1].1)?;
                }
                "rate-scaling" => o.rate_scaling = pair.int(0, 7)? as u8,
                "velocity" => o.velocity = pair.int(0, 7)? as u8,
                "amp-sensitivity" => o.amplitude_sensitivity = pair.int(0, 3)? as u8,
                _ => return Err(unknown(&pair, "op")),
            }
        }
        self.patch.voice.operators[number - 1] = Some(o);
        Ok(())
    }

    /// Cross-field checks, each at the text that caused it.
    pub(in crate::synth) fn finish(self) -> Parsed<FmPatch> {
        let voice = &self.patch.voice;
        let product = voice.voices as u16 * voice.unison as u16;
        if product > MAX_FM_UNISON_VOICES {
            let pos = self.unison.or(self.voices).unwrap_or(self.start);
            return Err(pos.error(
                ErrorCode::LimitExceeded,
                format!("voices * unison must be at most {MAX_FM_UNISON_VOICES}, not {product}"),
            ));
        }
        if let Some(pos) = self.mono.filter(|_| voice.voices != 1) {
            return Err(pos.invalid("Glide and legato require voices=1"));
        }
        let routing = FmRouting::algorithm(voice.algorithm).map_err(|e| self.start.wrap(e))?;
        if !(1..=6u8).any(|n| routing.is_carrier(n) && voice.operators[n as usize - 1].is_some()) {
            return Err(self.start.invalid(format!(
                "Algorithm {} needs an op line for at least one of its carriers",
                voice.algorithm
            )));
        }
        self.patch.validate().map_err(|e| self.start.wrap(e))?;
        Ok(self.patch)
    }

    pub(in crate::synth) fn unclosed(&self) -> super::ParseError {
        self.start.syntax(format!(
            "fm block {} is not closed with end",
            self.patch.name
        ))
    }
}

fn lfo(cursor: &mut Cursor) -> Parsed<FmLfo> {
    let mut l = FmLfo::default();
    for pair in pairs(cursor)? {
        match pair.key {
            "wave" => l.wave = named(&FmLfoWave::NAMES, "wave", pair.value, pair.value_pos)?,
            "speed" => l.speed = pair.int(0, 99)? as u8,
            "delay" => l.delay = pair.int(0, 99)? as u8,
            "pitch" => l.pitch_depth = pair.int(0, 99)? as u8,
            "amp" => l.amplitude_depth = pair.int(0, 99)? as u8,
            "sync" => l.key_sync = pair.switch()?,
            "pitch-sensitivity" => l.pitch_sensitivity = pair.int(0, 7)? as u8,
            "beats" => {
                let [n, d] = pair.list::<2>(1, 64)?;
                l.beats = Some((n as u16, d as u16));
            }
            "seed" => l.seed = pair.int(0, u32::MAX as i64)? as u32,
            _ => return Err(unknown(&pair, "lfo")),
        }
    }
    Ok(l)
}

pub(in crate::synth) fn parse_patch(text: &str) -> Parsed<FmPatch> {
    let first = Pos { line: 1, column: 1 };
    if text.len() > MAX_FM_TEXT_BYTES {
        return Err(first.error(ErrorCode::LimitExceeded, "FM patch text exceeds 64 KiB"));
    }
    let mut block: Option<Block> = None;
    let mut done: Option<FmPatch> = None;
    for (index, raw) in text.lines().enumerate() {
        let content = raw.split(';').next().unwrap_or("");
        let mut cursor = Cursor::new(content, index + 1, 1);
        let (pos, keyword) = cursor.word();
        if keyword.is_empty() {
            continue;
        }
        if done.is_some() {
            return Err(pos.syntax("Unexpected text after end"));
        }
        match &mut block {
            None if keyword == "fm" => block = Some(Block::header(pos, &mut cursor)?),
            None => return Err(pos.syntax(format!("Expected fm NAME, found {keyword}"))),
            Some(open) => {
                if open.line(keyword, pos, &mut cursor)? {
                    done = Some(block.take().expect("open block").finish()?);
                }
            }
        }
    }
    match (done, block) {
        (Some(patch), _) => Ok(patch),
        (None, Some(open)) => Err(open.unclosed()),
        (None, None) => Err(first.syntax("Expected fm NAME")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::fm::FmEnvelope;

    #[test]
    fn envelope_defaults_match_the_type() {
        let patch = parse_patch("fm a\nop 1\nend").unwrap();
        assert_eq!(patch.voice, Fm::default());
        assert_eq!(patch.voice.pitch_envelope, FmEnvelope::PITCH);
    }
}
