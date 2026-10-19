use super::{Waveform, wave::Duty};
use crate::{Error, Result};

/// The most channels a score can have.
pub const MAX_CHANNELS: usize = 8;
/// The deepest nesting of repeats and patterns that [`ScoreLimits::max_nesting`] can allow.
pub const MAX_NESTING: usize = 256;
const MAX_MS: u32 = 60_000;

/// Bounds on a score, checked while it is parsed or built.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreLimits {
    /// Length of score text in bytes.
    pub max_text_bytes: usize,
    /// Depth of nested repeats and pattern references, at most [`MAX_NESTING`].
    pub max_nesting: usize,
    /// Number of channels, at most [`MAX_CHANNELS`].
    pub max_channels: usize,
    /// Notes in all channels after repeats and patterns are expanded.
    pub max_notes: usize,
    /// Commands executed while expanding repeats and patterns. Bounds parsing work
    /// even for repeats of commands that produce no notes.
    pub max_events: usize,
    /// Length of the song once through, before loops.
    pub max_duration_seconds: f64,
}
impl Default for ScoreLimits {
    fn default() -> Self {
        Self {
            max_text_bytes: 1024 * 1024,
            max_nesting: 16,
            max_channels: MAX_CHANNELS,
            max_notes: 200_000,
            max_events: 1_000_000,
            max_duration_seconds: 1200.0,
        }
    }
}

/// A MIDI note number, 0 through 127. Middle C (`o4 c`) is 60 and A4 (440 Hz) is 69.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Pitch(u8);
impl Pitch {
    pub const A4: Pitch = Pitch(69);
    pub const C4: Pitch = Pitch(60);
    pub fn new(midi: u8) -> Result<Self> {
        if midi > 127 {
            return Err(Error::invalid("Pitch must be a MIDI note 0..=127"));
        }
        Ok(Self(midi))
    }
    /// A note letter `a`..`g`, a number of semitones to raise (`1` for sharp, `-1`
    /// for flat) and an octave, where octave 4 starts at middle C.
    pub fn from_name(letter: char, accidental: i8, octave: i8) -> Result<Self> {
        let semitone = match letter.to_ascii_lowercase() {
            'c' => 0,
            'd' => 2,
            'e' => 4,
            'f' => 5,
            'g' => 7,
            'a' => 9,
            'b' => 11,
            _ => return Err(Error::invalid(format!("Unknown note letter {letter}"))),
        };
        let midi = (octave as i32 + 1) * 12 + semitone + accidental as i32;
        if !(0..=127).contains(&midi) {
            return Err(Error::invalid("Note is outside MIDI range 0..=127"));
        }
        Ok(Self(midi as u8))
    }
    pub fn midi(self) -> u8 {
        self.0
    }
    /// Equal-tempered frequency with A4 at 440 Hz.
    pub fn frequency(self) -> f64 {
        super::math::cents_to_hz((self.0 as f64 - 69.0) * 100.0)
    }
}

/// A note length: a fraction of a whole note with dots, or an exact number of ticks.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Length {
    /// `denominator` 4 is a quarter note. Each dot adds half of the previous value.
    Note {
        denominator: u16,
        dots: u8,
    },
    Ticks(u32),
}
impl Length {
    pub const WHOLE: Length = Length::note(1);
    pub const HALF: Length = Length::note(2);
    pub const QUARTER: Length = Length::note(4);
    pub const EIGHTH: Length = Length::note(8);
    pub const SIXTEENTH: Length = Length::note(16);
    pub const fn note(denominator: u16) -> Self {
        Self::Note {
            denominator,
            dots: 0,
        }
    }
    /// Adds a dot. A tick length is returned unchanged.
    pub const fn dotted(self) -> Self {
        match self {
            Self::Note { denominator, dots } => Self::Note {
                denominator,
                dots: dots.saturating_add(1),
            },
            ticks => ticks,
        }
    }
    /// Length in ticks. Fails unless it is a positive whole number of ticks.
    pub fn ticks(self, ticks_per_quarter: u16) -> Result<u64> {
        let (denominator, dots) = match self {
            Self::Ticks(0) => return Err(Error::invalid("Length must be positive")),
            Self::Ticks(t) => return Ok(t as u64),
            Self::Note { denominator, dots } => (denominator as u64, dots),
        };
        let whole = 4 * ticks_per_quarter as u64;
        let unrepresentable = || {
            Error::invalid(format!(
                "Length 1/{denominator}{} is not a whole number of ticks at {ticks_per_quarter} ticks per quarter",
                ".".repeat(dots as usize)
            ))
        };
        if denominator == 0 || whole % denominator != 0 {
            return Err(unrepresentable());
        }
        let mut part = whole / denominator;
        let mut total = part;
        for _ in 0..dots {
            if part % 2 != 0 {
                return Err(unrepresentable());
            }
            part /= 2;
            total += part;
        }
        Ok(total)
    }
}

/// Linear attack, decay and release in milliseconds, and a sustain level in percent.
/// A note's attack starts from the channel's current level, so retriggering does not click.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Envelope {
    pub attack_ms: u32,
    pub decay_ms: u32,
    /// 0 through 100.
    pub sustain: u8,
    pub release_ms: u32,
}
impl Default for Envelope {
    fn default() -> Self {
        Self {
            attack_ms: 0,
            decay_ms: 0,
            sustain: 100,
            release_ms: 0,
        }
    }
}

/// Sinusoidal pitch modulation that starts `delay_ms` after the note starts.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Vibrato {
    /// Peak deviation, up to 1200 cents.
    pub depth_cents: u16,
    pub period_ms: u32,
    pub delay_ms: u32,
}

/// Semitone offsets cycled every `step_ms` from the start of each note.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Arpeggio {
    /// One to 16 offsets of -48..=48 semitones.
    pub offsets: Vec<i8>,
    pub step_ms: u32,
}

/// Duties that replace a pulse's duty one after another every `step_ms` from the
/// start of each note. The last one holds.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DutySweep {
    /// One to 16 duties.
    pub duties: Vec<Duty>,
    pub step_ms: u32,
}

/// The sound of a channel: a waveform, an envelope, a volume and pitch effects.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Instrument {
    pub waveform: Waveform,
    pub envelope: Envelope,
    /// 0 through 15. The default is 15.
    pub volume: u8,
    /// Constant pitch slide from the start of each note, in cents per second.
    pub slide: i32,
    pub vibrato: Option<Vibrato>,
    pub arpeggio: Option<Arpeggio>,
    /// Only for [`Waveform::Pulse`].
    pub duty_sweep: Option<DutySweep>,
}
impl Default for Instrument {
    fn default() -> Self {
        Self::new(Waveform::default())
    }
}
impl Instrument {
    /// The waveform at full volume with the default envelope and no effects.
    pub fn new(waveform: Waveform) -> Self {
        Self {
            waveform,
            envelope: Envelope::default(),
            volume: 15,
            slide: 0,
            vibrato: None,
            arpeggio: None,
            duty_sweep: None,
        }
    }
    pub fn validate(&self) -> Result<()> {
        let e = &self.envelope;
        if self.volume > 15 {
            return Err(Error::invalid("Instrument volume must be 0..=15"));
        }
        if e.sustain > 100 {
            return Err(Error::invalid("Sustain must be 0..=100 percent"));
        }
        if [e.attack_ms, e.decay_ms, e.release_ms]
            .iter()
            .any(|&ms| ms > MAX_MS)
        {
            return Err(Error::invalid("Envelope times must be at most 60000 ms"));
        }
        if !(-1_200_000..=1_200_000).contains(&self.slide) {
            return Err(Error::invalid(
                "Slide must be within ±1200000 cents per second",
            ));
        }
        if let Some(v) = self.vibrato {
            if v.depth_cents > 1200
                || v.period_ms == 0
                || v.period_ms > MAX_MS
                || v.delay_ms > MAX_MS
            {
                return Err(Error::invalid(
                    "Vibrato needs a depth of at most 1200 cents and a period and delay of at most 60000 ms, with a positive period",
                ));
            }
        }
        if let Some(a) = &self.arpeggio {
            if a.offsets.is_empty()
                || a.offsets.len() > 16
                || a.offsets.iter().any(|o| !(-48..=48).contains(o))
                || a.step_ms == 0
                || a.step_ms > MAX_MS
            {
                return Err(Error::invalid(
                    "Arpeggio needs 1 to 16 offsets of -48..=48 semitones and a step of 1..=60000 ms",
                ));
            }
        }
        if let Some(s) = &self.duty_sweep {
            if !matches!(self.waveform, Waveform::Pulse(_)) {
                return Err(Error::invalid("A duty sweep needs a pulse waveform"));
            }
            if s.duties.is_empty() || s.duties.len() > 16 || s.step_ms == 0 || s.step_ms > MAX_MS {
                return Err(Error::invalid(
                    "Duty sweep needs 1 to 16 duties and a step of 1..=60000 ms",
                ));
            }
        }
        Ok(())
    }
}

/// A note with its position and length in ticks and the channel settings it was played with.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Note {
    pub start: u64,
    pub length: u64,
    pub pitch: Pitch,
    /// Index into [`Score::instruments`].
    pub instrument: usize,
    /// Channel volume, 0 through 15.
    pub volume: u8,
    /// Fraction of the length in eighths, 1 through 8, before the release starts.
    pub gate: u8,
}

#[derive(Clone, Debug)]
pub struct Channel {
    name: String,
    notes: Vec<Note>,
    length: u64,
    instrument: Option<usize>,
    volume: u8,
    gate: u8,
    last_is_note: bool,
}
impl Channel {
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Non-overlapping notes in order of start.
    pub fn notes(&self) -> &[Note] {
        &self.notes
    }
    /// Length in ticks including trailing rests.
    pub fn length(&self) -> u64 {
        self.length
    }
}
impl PartialEq for Channel {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.notes == other.notes && self.length == other.length
    }
}

/// A song of up to eight channels of notes, at one tempo.
///
/// Build it in code with [`Score::channel`], or parse the text format described in
/// the [module documentation](super) with [`Score::parse`]. Both give equal scores.
#[derive(Clone, Debug)]
pub struct Score {
    tempo: u16,
    ticks_per_quarter: u16,
    instruments: Vec<(String, Instrument)>,
    channels: Vec<Channel>,
    loop_tick: Option<u64>,
    notes: usize,
    limits: ScoreLimits,
}
impl PartialEq for Score {
    fn eq(&self, other: &Self) -> bool {
        self.tempo == other.tempo
            && self.ticks_per_quarter == other.ticks_per_quarter
            && self.instruments == other.instruments
            && self.channels == other.channels
            && self.loop_tick == other.loop_tick
    }
}
impl Score {
    /// `tempo` in quarter notes per minute, 1..=999, and a time base of 1..=960 ticks per quarter.
    pub fn new(tempo: u16, ticks_per_quarter: u16) -> Result<Self> {
        Self::with_limits(tempo, ticks_per_quarter, ScoreLimits::default())
    }
    pub fn with_limits(tempo: u16, ticks_per_quarter: u16, limits: ScoreLimits) -> Result<Self> {
        if !(1..=999).contains(&tempo) {
            return Err(Error::invalid("Tempo must be 1..=999"));
        }
        if !(1..=960).contains(&ticks_per_quarter) {
            return Err(Error::invalid("Ticks per quarter must be 1..=960"));
        }
        if limits.max_channels > MAX_CHANNELS
            || limits.max_nesting > MAX_NESTING
            || !limits.max_duration_seconds.is_finite()
            || limits.max_duration_seconds < 0.0
        {
            return Err(Error::invalid(
                "Score limits allow at most 8 channels, a nesting of 256 and a finite, non-negative duration",
            ));
        }
        Ok(Self {
            tempo,
            ticks_per_quarter,
            instruments: Vec::new(),
            channels: Vec::new(),
            loop_tick: None,
            notes: 0,
            limits,
        })
    }
    pub fn tempo(&self) -> u16 {
        self.tempo
    }
    pub fn ticks_per_quarter(&self) -> u16 {
        self.ticks_per_quarter
    }
    pub fn limits(&self) -> &ScoreLimits {
        &self.limits
    }
    pub fn instruments(&self) -> &[(String, Instrument)] {
        &self.instruments
    }
    pub fn instrument_index(&self, name: &str) -> Option<usize> {
        self.instruments.iter().position(|(n, _)| n == name)
    }
    pub fn channels(&self) -> &[Channel] {
        &self.channels
    }
    /// The tick where the song restarts after its end. Without one, it restarts at 0.
    pub fn loop_tick(&self) -> Option<u64> {
        self.loop_tick
    }
    /// The song's length in ticks: the longest channel.
    pub fn length_ticks(&self) -> u64 {
        self.channels.iter().map(|c| c.length).max().unwrap_or(0)
    }
    pub fn duration_seconds(&self) -> f64 {
        self.ticks_to_seconds(self.length_ticks())
    }
    fn ticks_to_seconds(&self, ticks: u64) -> f64 {
        ticks as f64 * 60.0 / (self.tempo as f64 * self.ticks_per_quarter as f64)
    }
    /// Adds a named instrument and returns its index.
    pub fn add_instrument(
        &mut self,
        name: impl Into<String>,
        instrument: Instrument,
    ) -> Result<usize> {
        let name = name.into();
        if self.instrument_index(&name).is_some() {
            return Err(Error::invalid(format!(
                "Instrument {name} is already defined"
            )));
        }
        if self.instruments.len() >= 256 {
            return Err(Error::limit("A score has at most 256 instruments"));
        }
        instrument.validate()?;
        self.instruments.push((name, instrument));
        Ok(self.instruments.len() - 1)
    }
    /// Writes to the named channel, creating it after the existing ones. Writing to
    /// it again continues where it ended, with the same instrument, volume and gate.
    pub fn channel(&mut self, name: &str) -> Result<ChannelWriter<'_>> {
        let index = match self.channels.iter().position(|c| c.name == name) {
            Some(i) => i,
            None => {
                if self.channels.len() >= self.limits.max_channels {
                    return Err(Error::limit(format!(
                        "A score has at most {} channels",
                        self.limits.max_channels
                    )));
                }
                self.channels.push(Channel {
                    name: name.to_owned(),
                    notes: Vec::new(),
                    length: 0,
                    instrument: None,
                    volume: 15,
                    gate: 8,
                    last_is_note: false,
                });
                self.channels.len() - 1
            }
        };
        Ok(ChannelWriter { score: self, index })
    }
}

/// Appends notes and rests to one channel of a [`Score`].
pub struct ChannelWriter<'a> {
    score: &'a mut Score,
    index: usize,
}
impl ChannelWriter<'_> {
    pub(super) fn score(&self) -> &Score {
        self.score
    }
    fn state(&mut self) -> &mut Channel {
        &mut self.score.channels[self.index]
    }
    /// The position in ticks where the next note or rest starts.
    pub fn position(&self) -> u64 {
        self.score.channels[self.index].length
    }
    /// Selects the instrument for the following notes, by its index.
    pub fn instrument(&mut self, index: usize) -> Result<&mut Self> {
        if index >= self.score.instruments.len() {
            return Err(Error::invalid("Instrument index out of range"));
        }
        self.state().instrument = Some(index);
        Ok(self)
    }
    /// Channel volume for the following notes, 0 through 15. The default is 15.
    pub fn volume(&mut self, volume: u8) -> Result<&mut Self> {
        if volume > 15 {
            return Err(Error::invalid("Volume must be 0..=15"));
        }
        self.state().volume = volume;
        Ok(self)
    }
    /// The part of each following note, in eighths, played before its release. The default is 8.
    pub fn gate(&mut self, eighths: u8) -> Result<&mut Self> {
        if !(1..=8).contains(&eighths) {
            return Err(Error::invalid("Gate must be 1..=8 eighths"));
        }
        self.state().gate = eighths;
        Ok(self)
    }
    pub fn note(&mut self, pitch: Pitch, length: Length) -> Result<&mut Self> {
        let ticks = length.ticks(self.score.ticks_per_quarter)?;
        let channel = &self.score.channels[self.index];
        let instrument = channel
            .instrument
            .ok_or_else(|| Error::invalid("Select an instrument before the first note"))?;
        if self.score.notes >= self.score.limits.max_notes {
            return Err(Error::limit("Score exceeds its note limit"));
        }
        let note = Note {
            start: channel.length,
            length: ticks,
            pitch,
            instrument,
            volume: channel.volume,
            gate: channel.gate,
        };
        self.advance(ticks)?;
        self.score.notes += 1;
        let channel = self.state();
        channel.notes.push(note);
        channel.last_is_note = true;
        Ok(self)
    }
    pub fn rest(&mut self, length: Length) -> Result<&mut Self> {
        let ticks = length.ticks(self.score.ticks_per_quarter)?;
        self.advance(ticks)?;
        self.state().last_is_note = false;
        Ok(self)
    }
    /// Lengthens the previous note, or the previous rest, by `length`.
    pub fn tie(&mut self, length: Length) -> Result<&mut Self> {
        let ticks = length.ticks(self.score.ticks_per_quarter)?;
        if self.position() == 0 {
            return Err(Error::invalid("A tie needs a preceding note or rest"));
        }
        self.advance(ticks)?;
        let channel = self.state();
        if channel.last_is_note {
            if let Some(note) = channel.notes.last_mut() {
                note.length += ticks;
            }
        }
        Ok(self)
    }
    /// Marks the current position as the song's loop point. All channels that mark
    /// one must mark the same tick.
    pub fn loop_point(&mut self) -> Result<&mut Self> {
        let tick = self.position();
        match self.score.loop_tick {
            Some(t) if t != tick => {
                return Err(Error::invalid(format!(
                    "Loop point at tick {tick} differs from the loop point at tick {t}"
                )));
            }
            _ => self.score.loop_tick = Some(tick),
        }
        Ok(self)
    }
    fn advance(&mut self, ticks: u64) -> Result<()> {
        let end = self
            .position()
            .checked_add(ticks)
            .filter(|&end| {
                self.score.ticks_to_seconds(end) <= self.score.limits.max_duration_seconds
            })
            .ok_or_else(|| Error::limit("Score exceeds its duration limit"))?;
        self.state().length = end;
        Ok(())
    }
}
