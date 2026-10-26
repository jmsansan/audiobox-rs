//! Typed preparation for absolute note frames; arrangement syntax belongs to callers.
use super::{Instrument, MAX_CHANNELS, Pitch, ScoreLimits};
use crate::{Error, Limits, Result};

/// Maximum authored tempo segments in a prepared map.
pub const MAX_TEMPO_CHANGES: usize = 1024;

/// A tempo beginning at an absolute tick. BPM counts quarter notes per minute.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoChange {
    pub tick: u64,
    /// Finite 1..=1000 BPM; rounded to 0.001 BPM during preparation.
    pub bpm: f64,
}

#[derive(Clone, Copy, Debug)]
struct Rational {
    num: u128,
    den: u128,
}
fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
fn timing_overflow() -> Error {
    Error::limit("Exact prepared timing exceeds checked u128/u64 bounds")
}
impl Rational {
    fn new(num: u128, den: u128) -> Self {
        let g = gcd(num, den);
        Self {
            num: num / g,
            den: den / g,
        }
    }
    fn duration(ticks: u64, rate: u32, bpm: u32, tpq: u16) -> Self {
        // The bounded factors (u64 ticks, <=768000 Hz) fit in u128.
        Self::new(
            ticks as u128 * rate as u128 * 60000,
            bpm as u128 * tpq as u128,
        )
    }
    fn add(self, other: Self) -> Result<Self> {
        let g = gcd(self.den, other.den);
        let a = other.den / g;
        let b = self.den / g;
        let num = self
            .num
            .checked_mul(a)
            .and_then(|n| n.checked_add(other.num.checked_mul(b)?))
            .ok_or_else(timing_overflow)?;
        let den = self.den.checked_mul(a).ok_or_else(timing_overflow)?;
        Ok(Self::new(num, den))
    }
    fn frame(self) -> Result<u64> {
        crate::timing::rounded_ratio(self.num, self.den).ok_or_else(timing_overflow)
    }
}

#[derive(Clone, Debug)]
struct Segment {
    tick: u64,
    bpm: u32,
    start: Rational,
    frame: u64,
    beat_numerator: u128,
}

/// An immutable, prepared tick-to-frame map with 0.001-BPM tempo precision.
///
/// Changes must be strictly tick-ordered, start at tick zero, and contain
/// 1..=1024 segments. TPQ is 1..=960; sample rate follows default `Limits`.
/// Prefix durations are exact reduced u128 rationals. Every absolute position
/// is rounded once, nearest frame with halves up, using Music::Transport's rule.
/// Invalid tempo/order/TPQ is `InvalidArgument`; sample-rate limits, segment
/// count, rational intermediate overflow or frames beyond u64 are `LimitExceeded`.
/// This deliberately differs from legacy Score's integer-tempo floor rounding.
#[derive(Clone, Debug)]
pub struct TempoMap {
    rate: u32,
    tpq: u16,
    segments: Vec<Segment>,
}
impl TempoMap {
    pub fn new(sample_rate: u32, ticks_per_quarter: u16, changes: &[TempoChange]) -> Result<Self> {
        Limits::default().check(0, 1, sample_rate)?;
        if changes.len() > MAX_TEMPO_CHANGES {
            return Err(Error::limit("Too many prepared tempo changes"));
        }
        if changes.is_empty()
            || changes[0].tick != 0
            || !(1..=960).contains(&ticks_per_quarter)
            || changes.windows(2).any(|p| p[0].tick >= p[1].tick)
        {
            return Err(Error::invalid(
                "Tempo changes must start at zero and increase; TPQ must be 1..=960",
            ));
        }
        let mut segments: Vec<Segment> = Vec::new();
        segments
            .try_reserve_exact(changes.len())
            .map_err(|_| Error::limit("Cannot reserve tempo map"))?;
        for change in changes {
            let bpm = crate::timing::milli_bpm(change.bpm)?;
            let (start, beat_numerator) = if let Some(previous) = segments.last() {
                let start = previous.start.add(Rational::duration(
                    change.tick - previous.tick,
                    sample_rate,
                    previous.bpm,
                    ticks_per_quarter,
                ))?;
                let frame = start.frame()?;
                (
                    start,
                    previous.beat_numerator
                        + (frame - previous.frame) as u128 * previous.bpm as u128,
                )
            } else {
                (Rational::new(0, 1), 0)
            };
            segments.push(Segment {
                tick: change.tick,
                bpm,
                start,
                frame: start.frame()?,
                beat_numerator,
            });
        }
        Ok(Self {
            rate: sample_rate,
            tpq: ticks_per_quarter,
            segments,
        })
    }
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
    pub fn ticks_per_quarter(&self) -> u16 {
        self.tpq
    }
    /// Map an absolute tick once, including note-offs and the exclusive body end.
    /// Map both boundaries of a duration and subtract; never add rounded lengths.
    /// Exact rational intermediate overflow or frames beyond u64 are rejected.
    pub fn frame_at(&self, tick: u64) -> Result<u64> {
        let s = &self.segments[self.segments.partition_point(|s| s.tick <= tick) - 1];
        s.start
            .add(Rational::duration(
                tick - s.tick,
                self.rate,
                s.bpm,
                self.tpq,
            ))?
            .frame()
    }
    /// The normalized tempo in effect at an absolute tick, in BPM.
    pub fn bpm_at(&self, tick: u64) -> f64 {
        self.segments[self.segments.partition_point(|s| s.tick <= tick) - 1].bpm as f64 / 1000.0
    }
    pub(super) fn last_change_frame(&self) -> u64 {
        self.segments.last().unwrap().frame
    }
    pub(super) fn clock(&self, frame: u64) -> BeatClock {
        // Last authored change wins when several ticks round to the same frame.
        let s = &self.segments[self.segments.partition_point(|s| s.frame <= frame) - 1];
        BeatClock {
            numerator: s.beat_numerator + (frame - s.frame) as u128 * s.bpm as u128,
            denominator: self.rate as u128 * 60000,
        }
    }
}

/// Exact integrated quarter-note phase on the scheduled output-frame clock.
#[derive(Clone, Copy, Debug)]
pub(super) struct BeatClock {
    pub numerator: u128,
    pub denominator: u128,
}

/// One note's absolute onset and exclusive gate-off, in source output frames.
/// A tie across a section/tempo change is one note with a later `off`, not a
/// retrigger. Sorted equal-onset notes follow input order within each track.
#[derive(Clone, Copy, Debug)]
pub struct FrameNote {
    pub on: u64,
    pub off: u64,
    pub pitch: Pitch,
    pub instrument: usize,
    /// 0..=15 channel volume, independent of the instrument's own volume.
    pub volume: u8,
    /// 0..=127 velocity, linear in amplitude.
    pub velocity: u8,
}
impl FrameNote {
    pub fn new(on: u64, off: u64, pitch: Pitch, instrument: usize) -> Self {
        Self {
            on,
            off,
            pitch,
            instrument,
            volume: 15,
            velocity: 127,
        }
    }
}

/// Typed input for [`super::Synth::from_frames`] and its stereo constructor.
/// Up to eight tracks and 256 existing instruments per source. Use multiple
/// aligned sources for larger arrangements. Authored repeats are expanded to
/// absolute events; implicit RenderOptions loops are rejected on this path.
#[derive(Clone, Debug, Default)]
pub struct FrameScore {
    pub instruments: Vec<Instrument>,
    pub tracks: Vec<Vec<FrameNote>>,
    /// Exclusive musical body end, including authored trailing rests. Gates must
    /// end by this frame. Active releases/drum tails drain after it.
    pub end_frame: u64,
}
impl FrameScore {
    pub(super) fn validate(&self, map: &TempoMap) -> Result<()> {
        let limits = ScoreLimits::default();
        if self.tracks.len() > MAX_CHANNELS || self.instruments.len() > 256 {
            return Err(Error::limit(
                "Prepared synth needs at most eight tracks and 256 instruments",
            ));
        }
        if self.end_frame as u128 > map.rate as u128 * 1200 {
            return Err(Error::limit("Prepared synth body exceeds 1200 seconds"));
        }
        if map.last_change_frame() > self.end_frame {
            return Err(Error::invalid(
                "Tempo changes extend beyond the prepared body",
            ));
        }
        let mut notes = 0usize;
        for track in &self.tracks {
            notes = notes
                .checked_add(track.len())
                .ok_or_else(|| Error::limit("Prepared note count overflow"))?;
            if notes > limits.max_notes {
                return Err(Error::limit("Too many prepared notes"));
            }
            if track.windows(2).any(|p| p[0].on > p[1].on) {
                return Err(Error::invalid("Prepared notes must be onset-ordered"));
            }
            for note in track {
                if note.on >= note.off
                    || note.off > self.end_frame
                    || note.instrument >= self.instruments.len()
                    || note.volume > 15
                    || note.velocity > 127
                {
                    return Err(Error::invalid(
                        "Invalid prepared note bounds, instrument, volume or velocity",
                    ));
                }
            }
        }
        for instrument in &self.instruments {
            instrument.validate()?;
        }
        Ok(())
    }
}
