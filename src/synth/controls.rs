//! Bounded source-frame control plans. No shared live handle or callback allocation.
use crate::{Error, Result};
use std::collections::BTreeMap;
use std::str::FromStr;

/// Maximum retained controls/results in one source's replayable plan.
pub const MAX_SYNTH_CONTROLS: usize = 65536;
pub(super) const PARAMETERS: usize = 9;

/// Supported continuous quantities. Structural instrument edits are not controls.
/// `FromStr` accepts the documented kebab-case names; unsupported names return
/// `InvalidArgument`, including `voices`, `unison`, `waveform` and envelope edits.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(usize)]
pub enum ControlParameter {
    /// Linear multiplier after the voice envelope, 0..=1. Default one.
    Expression,
    /// Whole-voice linear pan, -1..=1; overrides the base pan, before its LFO.
    Pan,
    /// Synthwave base cutoff in Hz, 1..=96000, before existing modulation/clamp.
    CutoffHz,
    /// Synthwave filter Q, 0.5..=10.
    ResonanceQ,
    /// Synthwave additive oscillator pitch offset, -2400..=2400 cents.
    PitchCents,
    /// Existing pitch LFO depth, 0..=2400 cents; a configured LFO is required.
    PitchLfoDepth,
    /// Existing cutoff LFO depth, 0..=9600 cents; a configured LFO is required.
    CutoffLfoDepth,
    /// Existing amplitude LFO depth, 0..=100 percent.
    AmplitudeLfoDepth,
    /// Existing pan LFO depth, 0..=100 percent.
    PanLfoDepth,
}
impl ControlParameter {
    pub(super) fn validate(self, value: f64) -> Result<()> {
        let (min, max) = match self {
            Self::Expression => (0.0, 1.0),
            Self::Pan => (-1.0, 1.0),
            Self::CutoffHz => (1.0, 96000.0),
            Self::ResonanceQ => (0.5, 10.0),
            Self::PitchCents => (-2400.0, 2400.0),
            Self::PitchLfoDepth => (0.0, 2400.0),
            Self::CutoffLfoDepth => (0.0, 9600.0),
            Self::AmplitudeLfoDepth | Self::PanLfoDepth => (0.0, 100.0),
        };
        if !value.is_finite() || !(min..=max).contains(&value) {
            return Err(Error::invalid(
                "Synth control value exceeds its parameter range",
            ));
        }
        Ok(())
    }
}
impl FromStr for ControlParameter {
    type Err = Error;
    fn from_str(name: &str) -> Result<Self> {
        Ok(match name {
            "expression" => Self::Expression,
            "pan" => Self::Pan,
            "cutoff-hz" => Self::CutoffHz,
            "resonance-q" => Self::ResonanceQ,
            "pitch-cents" => Self::PitchCents,
            "pitch-lfo-depth" => Self::PitchLfoDepth,
            "cutoff-lfo-depth" => Self::CutoffLfoDepth,
            "amplitude-lfo-depth" => Self::AmplitudeLfoDepth,
            "pan-lfo-depth" => Self::PanLfoDepth,
            _ => return Err(Error::invalid("Unsupported synth control parameter")),
        })
    }
}

/// Persistent track/instrument control, or an individual authored note index.
/// Note indices are positions in the track's Score notes / FrameScore notes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ControlTarget {
    /// Affects existing/releasing and future matching voices, independently per track.
    Instrument { track: usize, instrument: usize },
    /// Overrides that note's instrument lane. It must start within the authored
    /// gate. A note already stolen/ended at execution reports `TargetInactive`.
    Note { track: usize, note: usize },
}

/// Ramp interpolation, separate from the mixer's fade-easing curves.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ControlCurve {
    #[default]
    Linear,
    /// Geometric interpolation in log2 space, requiring strictly positive
    /// source/target endpoints. Uses deterministic engine log2/exp2.
    Exponential,
}

/// One source-frame event. Zero ramp frames steps immediately and then holds.
/// A positive ramp starts at the current value on `frame` and reaches `value`
/// exactly on `frame + ramp_frames`; an overlapping event retargets from there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlEvent {
    pub frame: u64,
    pub target: ControlTarget,
    pub parameter: ControlParameter,
    pub value: f64,
    pub ramp_frames: u64,
    pub curve: ControlCurve,
}
impl ControlEvent {
    pub fn new(frame: u64, target: ControlTarget, parameter: ControlParameter, value: f64) -> Self {
        Self {
            frame,
            target,
            parameter,
            value,
            ramp_frames: 0,
            curve: ControlCurve::Linear,
        }
    }
}

/// Submission index in the current plan. Reset replays the same IDs; replacing
/// a plan starts a new ID space. Later scheduling does not renumber earlier IDs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlId(pub u32);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlStatus {
    Applied,
    TargetInactive,
}
/// One bounded result per executed event. Polling does not allocate or free.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlReport {
    pub id: ControlId,
    pub frame: u64,
    pub status: ControlStatus,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Ramp {
    start: u64,
    end: u64,
    from: f64,
    to: f64,
    curve: ControlCurve,
    log_from: f64,
    log_to: f64,
}
impl Ramp {
    fn new(from: f64, event: ControlEvent) -> Result<Self> {
        let (log_from, log_to) = if event.curve == ControlCurve::Exponential {
            if from <= 0.0 || event.value <= 0.0 {
                return Err(Error::invalid(
                    "Exponential synth controls require positive endpoints",
                ));
            }
            (crate::math::log2(from), crate::math::log2(event.value))
        } else {
            (0.0, 0.0)
        };
        Ok(Self {
            start: event.frame,
            end: event
                .frame
                .checked_add(event.ramp_frames)
                .ok_or_else(|| Error::limit("Synth ramp frame overflow"))?,
            from,
            to: event.value,
            curve: event.curve,
            log_from,
            log_to,
        })
    }
    pub(super) fn value(self, frame: u64) -> f64 {
        if frame >= self.end {
            return self.to;
        }
        if frame <= self.start {
            return self.from;
        }
        let t = (frame - self.start) as f64 / (self.end - self.start) as f64;
        let value = match self.curve {
            ControlCurve::Linear => self.from + (self.to - self.from) * t,
            ControlCurve::Exponential => {
                crate::math::exp2(self.log_from + (self.log_to - self.log_from) * t)
            }
        };
        value.clamp(self.from.min(self.to), self.from.max(self.to))
    }
}
pub(super) type Lanes = [Option<Ramp>; PARAMETERS];
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Values {
    mask: u16,
    values: [f64; PARAMETERS],
}
impl Values {
    pub(super) fn get(self, p: ControlParameter) -> Option<f64> {
        (self.mask & (1 << p as usize) != 0).then_some(self.values[p as usize])
    }
    fn set(&mut self, p: usize, value: f64) {
        self.mask |= 1 << p;
        self.values[p] = value;
    }
    pub(super) fn some(self) -> Option<Self> {
        (self.mask != 0).then_some(self)
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Resolved {
    pub group: usize,
    pub note: Option<(usize, usize)>,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Scheduled {
    pub id: ControlId,
    pub event: ControlEvent,
    target: Resolved,
    ramp: Ramp,
}
#[derive(Clone, Debug)]
pub(super) struct VoiceLanes {
    note: Option<usize>,
    lanes: Lanes,
}
#[derive(Debug)]
pub(super) struct ControlState {
    pub queue: Vec<Scheduled>,
    pub next: usize,
    pub capacity: usize,
    pub used: Vec<bool>,
    groups: Vec<Lanes>,
    voices: Vec<Vec<VoiceLanes>>,
    reports: Vec<ControlReport>,
    poll: usize,
}
impl Clone for ControlState {
    fn clone(&self) -> Self {
        // Vec::clone retains only length, losing reserved callback capacity.
        let mut queue = Vec::with_capacity(self.capacity);
        queue.extend_from_slice(&self.queue);
        let mut reports = Vec::with_capacity(self.capacity);
        reports.extend_from_slice(&self.reports);
        Self {
            queue,
            reports,
            next: self.next,
            capacity: self.capacity,
            used: self.used.clone(),
            groups: self.groups.clone(),
            voices: self.voices.clone(),
            poll: self.poll,
        }
    }
}
pub(super) fn compile(
    entries: &[(ControlId, ControlEvent)],
    capacity: usize,
    resolve: impl Fn(ControlEvent) -> Result<(Resolved, f64)>,
) -> Result<Vec<Scheduled>> {
    if entries.windows(2).any(|p| p[0].1.frame > p[1].1.frame) {
        return Err(Error::invalid("Synth controls must be frame-ordered"));
    }
    let mut queue = Vec::new();
    queue
        .try_reserve_exact(capacity)
        .map_err(|_| Error::limit("Cannot reserve synth controls"))?;
    let mut history: BTreeMap<(usize, Option<usize>, ControlParameter), Ramp> = BTreeMap::new();
    for &(id, event) in entries {
        event.parameter.validate(event.value)?;
        let (target, base) = resolve(event)?;
        let key = (
            target.group,
            target.note.map(|(_, note)| note),
            event.parameter,
        );
        let from = history
            .get(&key)
            .or_else(|| history.get(&(target.group, None, event.parameter)))
            .map_or(base, |r| r.value(event.frame));
        let ramp = Ramp::new(from, event)?;
        history.insert(key, ramp);
        queue.push(Scheduled {
            id,
            event,
            target,
            ramp,
        });
    }
    Ok(queue)
}
impl ControlState {
    pub fn new(
        used: Vec<bool>,
        voice_counts: impl Iterator<Item = usize>,
        capacity: usize,
    ) -> Result<Self> {
        let groups = vec![[None; PARAMETERS]; used.len()];
        let voices = voice_counts
            .map(|n| {
                vec![
                    VoiceLanes {
                        note: None,
                        lanes: [None; PARAMETERS]
                    };
                    n
                ]
            })
            .collect();
        let mut reports = Vec::new();
        reports
            .try_reserve_exact(capacity)
            .map_err(|_| Error::limit("Cannot reserve synth control reports"))?;
        Ok(Self {
            queue: Vec::new(),
            next: 0,
            capacity,
            used,
            groups,
            voices,
            reports,
            poll: 0,
        })
    }
    pub fn sync(&mut self, track: usize, slot: usize, note: Option<usize>) {
        let state = &mut self.voices[track][slot];
        if state.note != note {
            state.note = note;
            state.lanes.fill(None);
        }
    }
    pub fn advance(&mut self, frame: u64) {
        while let Some(c) = self.queue.get(self.next).copied() {
            if c.event.frame > frame {
                break;
            }
            let lanes = match c.target.note {
                None => Some(&mut self.groups[c.target.group]),
                Some((track, note)) => self.voices[track]
                    .iter_mut()
                    .find(|v| v.note == Some(note))
                    .map(|v| &mut v.lanes),
            };
            let status = if let Some(lanes) = lanes {
                lanes[c.event.parameter as usize] = Some(c.ramp);
                ControlStatus::Applied
            } else {
                ControlStatus::TargetInactive
            };
            self.reports.push(ControlReport {
                id: c.id,
                frame,
                status,
            });
            self.next += 1;
        }
    }
    pub fn values(&self, group: usize, track: usize, slot: usize, frame: u64) -> Values {
        let mut out = Values::default();
        for (p, (note, instrument)) in self.voices[track][slot]
            .lanes
            .iter()
            .zip(self.groups[group].iter())
            .enumerate()
        {
            if let Some(ramp) = note.or(*instrument) {
                out.set(p, ramp.value(frame));
            }
        }
        out
    }
    pub fn group_value(&self, group: usize, p: ControlParameter, frame: u64) -> Option<f64> {
        self.groups[group][p as usize].map(|r| r.value(frame))
    }
    pub fn poll(&mut self) -> Option<ControlReport> {
        let report = self.reports.get(self.poll).copied()?;
        self.poll += 1;
        Some(report)
    }
    pub fn reset(&mut self) {
        self.next = 0;
        self.poll = 0;
        self.reports.clear();
        self.groups.fill([None; PARAMETERS]);
        for track in &mut self.voices {
            for voice in track {
                voice.note = None;
                voice.lanes.fill(None);
            }
        }
    }
}
