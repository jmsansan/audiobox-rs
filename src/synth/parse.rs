use std::collections::HashMap;
use std::fmt;

use super::score::{
    Arpeggio, ChannelWriter, DutySweep, Envelope, Instrument, Length, Pitch, Score, ScoreLimits,
    Vibrato,
};
use super::wave::{Duty, NoiseMode, Waveform};
use crate::{Error, ErrorCode};

/// An error in score text, with the 1-based line and column (in characters) where it was found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    /// [`ErrorCode::Syntax`] for malformed text, [`ErrorCode::InvalidArgument`] for
    /// out-of-range values, and [`ErrorCode::LimitExceeded`] for exceeded [`ScoreLimits`].
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
        Error::new(e.code, e.to_string())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Pos {
    line: usize,
    column: usize,
}
impl Pos {
    fn error(self, code: ErrorCode, message: impl Into<String>) -> ParseError {
        ParseError {
            line: self.line,
            column: self.column,
            code,
            message: message.into(),
        }
    }
    fn syntax(self, message: impl Into<String>) -> ParseError {
        self.error(ErrorCode::Syntax, message)
    }
    fn invalid(self, message: impl Into<String>) -> ParseError {
        self.error(ErrorCode::InvalidArgument, message)
    }
    fn wrap(self, error: Error) -> ParseError {
        self.error(error.code, error.message)
    }
}

type Parsed<T> = std::result::Result<T, ParseError>;

/// Characters of one line with their positions.
struct Cursor<'a> {
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    text: &'a str,
    line: usize,
    column: usize,
}
impl<'a> Cursor<'a> {
    fn new(text: &'a str, line: usize, column: usize) -> Self {
        Self {
            chars: text.char_indices().peekable(),
            text,
            line,
            column,
        }
    }
    fn pos(&self) -> Pos {
        Pos {
            line: self.line,
            column: self.column,
        }
    }
    fn peek(&mut self) -> Option<char> {
        self.chars.peek().map(|&(_, c)| c)
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.chars.next().map(|(_, c)| c);
        if c.is_some() {
            self.column += 1;
        }
        c
    }
    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }
    fn take_while(&mut self, f: impl Fn(char) -> bool) -> &'a str {
        let start = self.chars.peek().map_or(self.text.len(), |&(i, _)| i);
        while self.peek().is_some_and(&f) {
            self.bump();
        }
        let end = self.chars.peek().map_or(self.text.len(), |&(i, _)| i);
        &self.text[start..end]
    }
    fn word(&mut self) -> (Pos, &'a str) {
        self.skip_space();
        let pos = self.pos();
        (pos, self.take_while(|c| !c.is_whitespace()))
    }
    fn name(&mut self, what: &str) -> Parsed<String> {
        let pos = self.pos();
        let name = self.take_while(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if name.is_empty() {
            return Err(pos.syntax(format!("Expected {what} name")));
        }
        Ok(name.to_owned())
    }
    fn number(&mut self) -> Parsed<Option<u32>> {
        let pos = self.pos();
        let digits = self.take_while(|c| c.is_ascii_digit());
        if digits.is_empty() {
            return Ok(None);
        }
        digits
            .parse()
            .map(Some)
            .map_err(|_| pos.invalid("Number is too large"))
    }
    fn required(&mut self, what: &str) -> Parsed<u32> {
        let pos = self.pos();
        self.number()?
            .ok_or_else(|| pos.syntax(format!("Expected {what}")))
    }
}

fn ranged(pos: Pos, value: u32, range: std::ops::RangeInclusive<u32>, what: &str) -> Parsed<u32> {
    if range.contains(&value) {
        Ok(value)
    } else {
        Err(pos.invalid(format!(
            "{what} must be {}..={}, not {value}",
            range.start(),
            range.end()
        )))
    }
}

#[derive(Clone, Copy, Debug)]
enum LengthKind {
    Default,
    Note(u16),
    Ticks(u32),
}
#[derive(Clone, Copy, Debug)]
struct LengthSpec {
    kind: LengthKind,
    dots: u8,
}

#[derive(Clone, Debug)]
enum Token {
    Note(char, i8, LengthSpec, Option<u8>),
    Rest(LengthSpec),
    Extend(LengthSpec),
    TieNext,
    Octave(u8),
    Up,
    Down,
    DefaultLength(LengthSpec),
    Volume(u8),
    Gate(u8),
    Instrument(String),
    Call(String),
    Loop,
    Open,
    Break,
    Close(u32),
}

fn length_spec(cursor: &mut Cursor) -> Parsed<LengthSpec> {
    let pos = cursor.pos();
    let kind = if cursor.eat('%') {
        LengthKind::Ticks(ranged(
            pos,
            cursor.required("ticks after %")?,
            1..=u32::MAX,
            "Ticks",
        )?)
    } else {
        match cursor.number()? {
            Some(n) => LengthKind::Note(ranged(pos, n, 1..=3840, "Note length")? as u16),
            None => LengthKind::Default,
        }
    };
    let mut dots = 0u8;
    while cursor.eat('.') {
        dots = dots.saturating_add(1);
    }
    if dots > 0 && matches!(kind, LengthKind::Ticks(_)) {
        return Err(pos.syntax("A tick length cannot be dotted"));
    }
    Ok(LengthSpec { kind, dots })
}

fn lex_body(cursor: &mut Cursor, out: &mut Vec<(Token, Pos)>) -> Parsed<()> {
    loop {
        cursor.skip_space();
        let pos = cursor.pos();
        let Some(c) = cursor.bump() else {
            return Ok(());
        };
        let token = match c {
            'a'..='g' => {
                let mut accidental = 0i8;
                loop {
                    if cursor.eat('+') || cursor.eat('#') {
                        accidental = accidental.saturating_add(1);
                    } else if cursor.eat('-') {
                        accidental = accidental.saturating_sub(1);
                    } else {
                        break;
                    }
                }
                let length = length_spec(cursor)?;
                let velocity = if cursor.eat('!') {
                    let velocity_pos = cursor.pos();
                    Some(ranged(
                        velocity_pos,
                        cursor.required("velocity after !")?,
                        0..=127,
                        "Velocity",
                    )? as u8)
                } else {
                    None
                };
                Token::Note(c, accidental, length, velocity)
            }
            'r' => Token::Rest(length_spec(cursor)?),
            '^' => Token::Extend(length_spec(cursor)?),
            '&' => Token::TieNext,
            'o' => Token::Octave(
                ranged(pos, cursor.required("octave after o")?, 0..=9, "Octave")? as u8,
            ),
            '>' => Token::Up,
            '<' => Token::Down,
            'l' => {
                let spec = length_spec(cursor)?;
                if matches!(spec.kind, LengthKind::Default) {
                    return Err(pos.syntax("Expected a length after l"));
                }
                Token::DefaultLength(spec)
            }
            'v' => Token::Volume(
                ranged(pos, cursor.required("volume after v")?, 0..=15, "Volume")? as u8,
            ),
            'q' => Token::Gate(ranged(pos, cursor.required("gate after q")?, 1..=8, "Gate")? as u8),
            '@' => Token::Instrument(cursor.name("an instrument")?),
            '$' => Token::Call(cursor.name("a pattern")?),
            'L' => Token::Loop,
            '[' => Token::Open,
            '|' => Token::Break,
            ']' => {
                let count_pos = cursor.pos();
                let count = cursor.number()?.unwrap_or(2);
                Token::Close(ranged(count_pos, count, 1..=65535, "Repeat count")?)
            }
            other => return Err(pos.syntax(format!("Unexpected character '{other}'"))),
        };
        out.push((token, pos));
    }
}

#[derive(Clone, Debug)]
enum Node {
    Token(Token, Pos),
    Repeat {
        body: Vec<Node>,
        tail: Vec<Node>,
        count: u32,
        pos: Pos,
    },
}

fn build_tree(tokens: Vec<(Token, Pos)>) -> Parsed<Vec<Node>> {
    // Each open repeat holds its start, the nodes before it, and its body if a `|` was seen.
    let mut stack: Vec<(Pos, Vec<Node>, Option<Vec<Node>>)> = Vec::new();
    let mut current = Vec::new();
    for (token, pos) in tokens {
        match token {
            Token::Open => stack.push((pos, std::mem::take(&mut current), None)),
            Token::Break => match stack.last_mut() {
                Some((_, _, body @ None)) => *body = Some(std::mem::take(&mut current)),
                Some(_) => return Err(pos.syntax("A repeat has only one '|'")),
                None => return Err(pos.syntax("'|' outside a repeat")),
            },
            Token::Close(count) => {
                let (open, outer, body) = stack
                    .pop()
                    .ok_or_else(|| pos.syntax("']' without a matching '['"))?;
                let inner = std::mem::replace(&mut current, outer);
                let (body, tail) = match body {
                    Some(body) => (body, inner),
                    None => (inner, Vec::new()),
                };
                current.push(Node::Repeat {
                    body,
                    tail,
                    count,
                    pos: open,
                });
            }
            token => current.push(Node::Token(token, pos)),
        }
    }
    match stack.last() {
        Some((open, ..)) => Err(open.syntax("'[' is never closed")),
        None => Ok(current),
    }
}

fn parse_instrument(cursor: &mut Cursor) -> Parsed<(String, Instrument, Pos)> {
    cursor.skip_space();
    let name_pos = cursor.pos();
    let name = cursor.name("an instrument")?;
    let mut instrument = Instrument::default();
    let (mut wave, mut duties, mut noise) = ("pulse", None, None);
    let (mut arpeggio, mut arpeggio_step, mut duty_step) = (None, 16, 16);
    let (mut drum_kind, mut drum_kit) = (None, None);
    let mut drum_options = Vec::new();
    let mut melodic_key = None;
    loop {
        let (pos, word) = cursor.word();
        if word.is_empty() {
            break;
        }
        let (key, value) = word
            .split_once('=')
            .ok_or_else(|| pos.syntax(format!("Expected key=value, found {word}")))?;
        let value_pos = Pos {
            column: pos.column + key.chars().count() + 1,
            ..pos
        };
        let numbers = |count: std::ops::RangeInclusive<usize>| -> Parsed<Vec<i64>> {
            let list: Vec<_> = value.split(',').map(|v| v.trim().parse::<i64>()).collect();
            match list.into_iter().collect::<std::result::Result<Vec<_>, _>>() {
                Ok(list) if count.contains(&list.len()) => Ok(list),
                _ => Err(value_pos.syntax(format!(
                    "{key} needs {} to {} comma-separated integers",
                    count.start(),
                    count.end()
                ))),
            }
        };
        let to_u32 = |v: i64| -> Parsed<u32> {
            u32::try_from(v).map_err(|_| value_pos.invalid(format!("{key} is out of range")))
        };
        if !matches!(key, "volume" | "drum" | "kit") && !key.starts_with("drum-") {
            melodic_key = Some(pos);
        }
        match key {
            "drum" => {
                drum_kind = Some(match value {
                    "kick" => super::DrumKind::Kick,
                    "snare" => super::DrumKind::Snare,
                    "closed-hat" => super::DrumKind::ClosedHat,
                    "open-hat" => super::DrumKind::OpenHat,
                    "tom" => super::DrumKind::Tom,
                    "clap" => super::DrumKind::Clap,
                    "crash" => super::DrumKind::Crash,
                    _ => return Err(value_pos.syntax("Unknown drum kind")),
                })
            }
            "kit" => {
                drum_kit = Some(match value {
                    "chiptune" => super::DrumKit::Chiptune,
                    "electronic" => super::DrumKit::Electronic,
                    _ => return Err(value_pos.syntax("Kit must be chiptune or electronic")),
                })
            }
            "drum-pitch" | "drum-tone" | "drum-noise" | "drum-mix" | "drum-click" | "drum-seed"
            | "drum-sweep" => {
                let count = if key == "drum-sweep" { 2 } else { 1 };
                let values = numbers(count..=count)?
                    .into_iter()
                    .map(to_u32)
                    .collect::<Parsed<Vec<_>>>()?;
                drum_options.push((key.to_owned(), values, value_pos));
            }
            "wave" => match value {
                "pulse" | "triangle" | "saw" | "noise" => wave = value,
                _ => {
                    return Err(value_pos.syntax(format!(
                        "wave must be pulse, triangle, saw or noise, not {value}"
                    )));
                }
            },
            "duty" => {
                let list = value
                    .split(',')
                    .map(|d| d.trim().parse::<Duty>())
                    .collect::<crate::Result<Vec<_>>>()
                    .map_err(|e| value_pos.wrap(e))?;
                duties = Some((list, pos));
            }
            "duty-step" => duty_step = to_u32(numbers(1..=1)?[0])?,
            "noise" => match value {
                "long" => noise = Some((NoiseMode::Long, pos)),
                "short" => noise = Some((NoiseMode::Short, pos)),
                _ => {
                    return Err(
                        value_pos.syntax(format!("noise must be long or short, not {value}"))
                    );
                }
            },
            "adsr" => {
                let v = numbers(4..=4)?;
                instrument.envelope = Envelope {
                    attack_ms: to_u32(v[0])?,
                    decay_ms: to_u32(v[1])?,
                    sustain: ranged(value_pos, to_u32(v[2])?, 0..=100, "Sustain")? as u8,
                    release_ms: to_u32(v[3])?,
                };
            }
            "volume" => {
                instrument.volume =
                    ranged(value_pos, to_u32(numbers(1..=1)?[0])?, 0..=15, "Volume")? as u8;
            }
            "slide" => {
                instrument.slide = i32::try_from(numbers(1..=1)?[0])
                    .map_err(|_| value_pos.invalid("slide is out of range"))?;
            }
            "vibrato" => {
                let v = numbers(2..=3)?;
                instrument.vibrato = Some(Vibrato {
                    depth_cents: u16::try_from(v[0])
                        .map_err(|_| value_pos.invalid("Vibrato depth is out of range"))?,
                    period_ms: to_u32(v[1])?,
                    delay_ms: to_u32(v.get(2).copied().unwrap_or(0))?,
                });
            }
            "arpeggio" => {
                let offsets = numbers(1..=16)?
                    .into_iter()
                    .map(|v| {
                        i8::try_from(v)
                            .map_err(|_| value_pos.invalid("Arpeggio offset is out of range"))
                    })
                    .collect::<Parsed<Vec<_>>>()?;
                arpeggio = Some(offsets);
            }
            "arpeggio-step" => arpeggio_step = to_u32(numbers(1..=1)?[0])?,
            _ => return Err(pos.syntax(format!("Unknown instrument key {key}"))),
        }
    }
    if let Some(kind) = drum_kind {
        if let Some(pos) = melodic_key {
            return Err(pos.invalid("Drums cannot use melodic oscillator or modulation keys"));
        }
        let mut drum = super::Drum::new(kind, drum_kit.unwrap_or(super::DrumKit::Electronic));
        for (key, values, pos) in drum_options {
            let as_u8 = |v| u8::try_from(v).map_err(|_| pos.invalid("Drum value is out of range"));
            let as_u16 =
                |v| u16::try_from(v).map_err(|_| pos.invalid("Drum value is out of range"));
            match key.as_str() {
                "drum-pitch" => drum.pitch_hz = as_u16(values[0])?,
                "drum-tone" => drum.tone_ms = values[0],
                "drum-noise" => drum.noise_ms = values[0],
                "drum-mix" => drum.noise_percent = as_u8(values[0])?,
                "drum-click" => drum.click_percent = as_u8(values[0])?,
                "drum-seed" => drum.seed = values[0],
                "drum-sweep" => {
                    drum.sweep_cents = as_u16(values[0])?;
                    drum.sweep_ms = values[1];
                }
                _ => unreachable!(),
            }
        }
        drum.validate().map_err(|e| name_pos.wrap(e))?;
        instrument.drum = Some(drum);
    } else if drum_kit.is_some() || !drum_options.is_empty() {
        return Err(name_pos.invalid("kit and drum parameters require drum=KIND"));
    }
    instrument.waveform = match wave {
        "pulse" => Waveform::Pulse(match &duties {
            Some((list, _)) => list[0],
            None => Duty::Half,
        }),
        "triangle" => Waveform::Triangle,
        "saw" => Waveform::Sawtooth,
        _ => Waveform::Noise(noise.map_or(NoiseMode::Long, |(m, _)| m)),
    };
    if let Some((_, pos)) = noise.filter(|_| wave != "noise") {
        return Err(pos.invalid("noise needs wave=noise"));
    }
    if let Some((list, pos)) = duties {
        if wave != "pulse" {
            return Err(pos.invalid("duty needs wave=pulse"));
        }
        if list.len() > 1 {
            instrument.duty_sweep = Some(DutySweep {
                duties: list[1..].to_vec(),
                step_ms: duty_step,
            });
        }
    }
    instrument.arpeggio = arpeggio.map(|offsets| Arpeggio {
        offsets,
        step_ms: arpeggio_step,
    });
    instrument.validate().map_err(|e| name_pos.wrap(e))?;
    Ok((name, instrument, name_pos))
}

/// The channel state that only exists in text: octave, default length and a pending `&`.
struct TextState {
    octave: i8,
    length: Length,
    last_pitch: Option<Pitch>,
    tie: Option<Pos>,
}

struct Expander<'a> {
    patterns: &'a HashMap<String, Vec<Node>>,
    limits: &'a ScoreLimits,
    events: usize,
    calls: Vec<&'a str>,
}
impl<'a> Expander<'a> {
    fn run(
        &mut self,
        nodes: &'a [Node],
        writer: &mut ChannelWriter,
        state: &mut TextState,
        depth: usize,
    ) -> Parsed<()> {
        for node in nodes {
            let pos = match node {
                Node::Token(_, pos) | Node::Repeat { pos, .. } => *pos,
            };
            self.spend(pos)?;
            match node {
                Node::Repeat {
                    body,
                    tail,
                    count,
                    pos,
                } => {
                    if depth >= self.limits.max_nesting {
                        return Err(pos.error(
                            ErrorCode::LimitExceeded,
                            "Repeats and patterns are nested too deeply",
                        ));
                    }
                    for i in 0..*count {
                        self.spend(*pos)?;
                        self.run(body, writer, state, depth + 1)?;
                        if i + 1 < *count {
                            self.run(tail, writer, state, depth + 1)?;
                        }
                    }
                }
                Node::Token(token, pos) => self.token(token, *pos, writer, state, depth)?,
            }
        }
        Ok(())
    }
    fn spend(&mut self, pos: Pos) -> Parsed<()> {
        if self.events >= self.limits.max_events {
            return Err(pos.error(ErrorCode::LimitExceeded, "Score exceeds its event limit"));
        }
        self.events += 1;
        Ok(())
    }
    fn token(
        &mut self,
        token: &'a Token,
        pos: Pos,
        writer: &mut ChannelWriter,
        state: &mut TextState,
        depth: usize,
    ) -> Parsed<()> {
        if let Some(tie) = state.tie {
            if !matches!(
                token,
                Token::Note(..) | Token::Octave(_) | Token::Up | Token::Down
            ) {
                return Err(tie.syntax("'&' must be followed by a note"));
            }
        }
        let length = |spec: &LengthSpec, state: &TextState| -> Length {
            match spec.kind {
                LengthKind::Ticks(t) => Length::Ticks(t),
                LengthKind::Note(denominator) => Length::Note {
                    denominator,
                    dots: spec.dots,
                },
                LengthKind::Default => (0..spec.dots).fold(state.length, |l, _| l.dotted()),
            }
        };
        let wrap = |e| pos.wrap(e);
        match token {
            Token::Note(letter, accidental, spec, velocity) => {
                let pitch = Pitch::from_name(*letter, *accidental, state.octave).map_err(wrap)?;
                let len = length(spec, state);
                if let Some(tie) = state.tie.take() {
                    if velocity.is_some() {
                        return Err(pos.invalid("A tied continuation cannot set velocity"));
                    }
                    if state.last_pitch != Some(pitch) {
                        return Err(tie.syntax("'&' joins notes of different pitch"));
                    }
                    writer.tie(len).map_err(wrap)?;
                } else {
                    writer
                        .note_with_velocity(pitch, len, velocity.unwrap_or(127))
                        .map_err(wrap)?;
                }
                state.last_pitch = Some(pitch);
            }
            Token::Rest(spec) => {
                writer.rest(length(spec, state)).map_err(wrap)?;
                state.last_pitch = None;
            }
            Token::Extend(spec) => {
                writer.tie(length(spec, state)).map_err(wrap)?;
            }
            Token::TieNext => {
                if state.last_pitch.is_none() {
                    return Err(pos.syntax("'&' must follow a note"));
                }
                state.tie = Some(pos);
            }
            Token::Octave(o) => state.octave = *o as i8,
            Token::Up | Token::Down => {
                let octave = state.octave + if matches!(token, Token::Up) { 1 } else { -1 };
                if !(0..=9).contains(&octave) {
                    return Err(pos.invalid("Octave must stay within 0..=9"));
                }
                state.octave = octave;
            }
            Token::DefaultLength(spec) => {
                let len = length(spec, state);
                len.ticks(writer.score().ticks_per_quarter())
                    .map_err(wrap)?;
                state.length = len;
            }
            Token::Volume(v) => {
                writer.volume(*v).map_err(wrap)?;
            }
            Token::Gate(g) => {
                writer.gate(*g).map_err(wrap)?;
            }
            Token::Instrument(name) => {
                let index = writer
                    .score()
                    .instrument_index(name)
                    .ok_or_else(|| pos.syntax(format!("Unknown instrument {name}")))?;
                writer.instrument(index).map_err(wrap)?;
            }
            Token::Loop => {
                writer.loop_point().map_err(wrap)?;
            }
            Token::Call(name) => {
                let (key, body) = self
                    .patterns
                    .get_key_value(name)
                    .ok_or_else(|| pos.syntax(format!("Unknown pattern {name}")))?;
                if self.calls.contains(&key.as_str()) {
                    return Err(pos.syntax(format!("Pattern {name} refers to itself")));
                }
                if depth >= self.limits.max_nesting {
                    return Err(pos.error(
                        ErrorCode::LimitExceeded,
                        "Repeats and patterns are nested too deeply",
                    ));
                }
                self.calls.push(key);
                self.run(body, writer, state, depth + 1)?;
                self.calls.pop();
            }
            Token::Open | Token::Break | Token::Close(_) => unreachable!(),
        }
        Ok(())
    }
}

struct Body {
    name: String,
    pos: Pos,
    tokens: Vec<(Token, Pos)>,
}

fn body_entry(bodies: &mut Vec<Body>, name: String, pos: Pos) -> &mut Body {
    let index = match bodies.iter().position(|b| b.name == name) {
        Some(i) => i,
        None => {
            bodies.push(Body {
                name,
                pos,
                tokens: Vec::new(),
            });
            bodies.len() - 1
        }
    };
    &mut bodies[index]
}

pub(super) fn parse(text: &str, limits: &ScoreLimits) -> Parsed<Score> {
    let start = Pos { line: 1, column: 1 };
    if text.len() > limits.max_text_bytes {
        return Err(start.error(
            ErrorCode::LimitExceeded,
            "Score text exceeds its length limit",
        ));
    }
    let (mut tempo, mut ticks) = (None, None);
    let mut instruments = Vec::new();
    let mut patterns: Vec<Body> = Vec::new();
    let mut channels: Vec<Body> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let content = raw.split(';').next().unwrap_or("");
        let mut cursor = Cursor::new(content, index + 1, 1);
        let (pos, keyword) = cursor.word();
        match keyword {
            "" => continue,
            "tempo" | "ticks" => {
                cursor.skip_space();
                let value_pos = cursor.pos();
                let value = cursor.required(&format!("a number after {keyword}"))?;
                let slot = if keyword == "tempo" {
                    &mut tempo
                } else {
                    &mut ticks
                };
                if slot.is_some() {
                    return Err(pos.syntax(format!("{keyword} is set twice")));
                }
                let range = if keyword == "tempo" { 1..=999 } else { 1..=960 };
                *slot = Some(ranged(value_pos, value, range, keyword)? as u16);
            }
            "instrument" => instruments.push(parse_instrument(&mut cursor)?),
            "pattern" | "channel" => {
                cursor.skip_space();
                let name_pos = cursor.pos();
                let name = cursor.name(keyword)?;
                cursor.skip_space();
                let eq = cursor.pos();
                if !cursor.eat('=') {
                    return Err(eq.syntax(format!("Expected '=' after the {keyword} name")));
                }
                let list = if keyword == "pattern" {
                    &mut patterns
                } else {
                    &mut channels
                };
                lex_body(&mut cursor, &mut body_entry(list, name, name_pos).tokens)?;
                continue;
            }
            other => return Err(pos.syntax(format!("Unknown directive {other}"))),
        }
        cursor.skip_space();
        if cursor.peek().is_some() {
            return Err(cursor
                .pos()
                .syntax("Unexpected text at the end of the line"));
        }
    }
    let mut score = Score::with_limits(tempo.unwrap_or(120), ticks.unwrap_or(48), limits.clone())
        .map_err(|e| start.wrap(e))?;
    for (name, instrument, pos) in instruments {
        score
            .add_instrument(name, instrument)
            .map_err(|e| pos.wrap(e))?;
    }
    let mut trees = HashMap::new();
    for body in patterns {
        trees.insert(body.name, build_tree(body.tokens)?);
    }
    let channel_trees = channels
        .into_iter()
        .map(|body| Ok((body.name, body.pos, build_tree(body.tokens)?)))
        .collect::<Parsed<Vec<_>>>()?;
    let mut expander = Expander {
        patterns: &trees,
        limits,
        events: 0,
        calls: Vec::new(),
    };
    for (name, pos, nodes) in &channel_trees {
        let mut writer = score.channel(name).map_err(|e| pos.wrap(e))?;
        let mut state = TextState {
            octave: 4,
            length: Length::QUARTER,
            last_pitch: None,
            tie: None,
        };
        expander.run(nodes, &mut writer, &mut state, 0)?;
        if let Some(tie) = state.tie {
            return Err(tie.syntax("'&' must be followed by a note"));
        }
    }
    Ok(score)
}
