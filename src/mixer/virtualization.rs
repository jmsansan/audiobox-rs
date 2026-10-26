//! Opt-in bounded physical-voice selection; legacy admission stays the default.
use super::{Mixer, TAILS};

/// Fixed global output-frame cadence for physical selection, plus command frames.
pub const VIRTUAL_VOICE_QUANTUM: u64 = 32;
/// Maximum retained logical voices, excluding the existing bounded fade-out slots.
pub const MAX_LOGICAL_VOICES: usize = 4096;

/// Opt-in logical voice pool. `MixerConfig::max_voices` remains the physical budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VirtualVoices {
    /// Total logical live voices, max_voices..=4096. Includes virtual/paused voices.
    pub capacity: usize,
    /// Estimated peak-amplitude floor in linear units, 0..=1. Memory uses its
    /// prepared sample peak; streams use a unit reference. Combined with a
    /// gain/fade/distance/pause bound through the quantum, not RMS/perceived
    /// loudness or bus gain (pre-fader sends may bypass the latter).
    pub threshold: f32,
}
impl Default for VirtualVoices {
    fn default() -> Self {
        Self {
            capacity: 256,
            threshold: 0.0001,
        }
    }
}

struct Candidate {
    slot: usize,
    priority: u8,
    level: f64,
    order: u64,
}
pub(super) struct Scheduler {
    pub settings: VirtualVoices,
    pub dirty: bool,
    quantum: Option<u64>,
    candidates: Vec<Candidate>,
}
impl Scheduler {
    pub fn new(settings: VirtualVoices, slots: usize) -> Self {
        Self {
            settings,
            dirty: true,
            quantum: None,
            candidates: Vec::with_capacity(slots),
        }
    }
}
impl Mixer {
    /// Latest snapshot of selected, unpaused physical voices, including audible tails.
    pub fn physical_voices(&self) -> usize {
        if self.buses[0].paused {
            return 0;
        }
        self.voices
            .iter()
            .flatten()
            .filter(|v| !v.virtualized && !v.paused() && !self.buses[v.bus].paused)
            .count()
    }
    /// Latest snapshot of advancing virtual voices; paused voices are separate.
    pub fn virtual_voice_count(&self) -> usize {
        if self.buses[0].paused {
            return 0;
        }
        self.voices
            .iter()
            .flatten()
            .filter(|v| v.virtualized && !v.paused() && !self.buses[v.bus].paused)
            .count()
    }
    pub(super) fn select_physical(&mut self, frame: u64) {
        let quantum = frame / VIRTUAL_VOICE_QUANTUM;
        let Some(scheduler) = &self.virtualization else {
            return;
        };
        if !scheduler.dirty && scheduler.quantum == Some(quantum) {
            return;
        }
        // Identical eligibility at a fixed decision frame regardless of caller
        // chunking: finish a source that reached EOF at the preceding sample.
        for voice in self.voices.iter_mut().flatten() {
            voice.settle();
        }
        self.retire_done();
        let scheduler = self.virtualization.as_mut().unwrap();
        scheduler.dirty = false;
        scheduler.quantum = Some(quantum);
        scheduler.candidates.clear();
        let end = (frame - frame % VIRTUAL_VOICE_QUANTUM) + VIRTUAL_VOICE_QUANTUM - 1;
        for (slot, voice) in self.voices.iter_mut().enumerate() {
            let Some(voice) = voice else { continue };
            if !voice.live() {
                continue;
            }
            voice.virtualized = true;
            if self.buses[0].paused || self.buses[voice.bus].paused || voice.paused() {
                continue;
            }
            let level = voice.source_peak * voice.level_bound(frame, end);
            if level > scheduler.settings.threshold as f64 {
                scheduler.candidates.push(Candidate {
                    slot,
                    priority: voice.priority,
                    level,
                    order: voice.order,
                });
            }
        }
        scheduler.candidates.sort_unstable_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(b.level.total_cmp(&a.level))
                .then(a.order.cmp(&b.order))
        });
        let physical = scheduler.candidates.len().min(self.max_voices);
        for candidate in &scheduler.candidates[..physical] {
            self.voices[candidate.slot].as_mut().unwrap().virtualized = false;
        }
        // Preserve already physical stop fades while bounding their additional
        // cost to the legacy physical budget + TAILS. A culled tail never returns.
        scheduler.candidates.clear();
        for (slot, voice) in self.voices.iter().enumerate() {
            if let Some(voice) = voice.as_deref().filter(|v| !v.live() && !v.virtualized) {
                scheduler.candidates.push(Candidate {
                    slot,
                    priority: voice.priority,
                    level: 0.0,
                    order: voice.order,
                });
            }
        }
        scheduler
            .candidates
            .sort_unstable_by_key(|v| std::cmp::Reverse(v.order));
        for candidate in scheduler
            .candidates
            .iter()
            .skip(self.max_voices + TAILS - physical)
        {
            self.voices[candidate.slot].as_mut().unwrap().virtualized = true;
        }
    }
}

pub(super) fn placement(
    spatial: &super::SpatialConfig,
    listener: &super::Listener,
    emitter: &super::Emitter,
    layout: super::OutputLayout,
    cull: bool,
) -> super::Placement {
    let mut p = spatial.place_validated(listener, emitter, layout);
    if cull {
        let squared = (0..3)
            .map(|i| {
                let d = emitter.position[i] as f64 - listener.position[i] as f64;
                d * d
            })
            .sum::<f64>();
        let max = emitter.attenuation.max_distance as f64;
        if squared >= max * max {
            p.distance_gain = 0.0;
        }
    }
    p
}
