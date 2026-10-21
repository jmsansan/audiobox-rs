//! Voice states published by the audio side for the game to read without blocking.
//!
//! Each voice slot has a sequence counter (a seqlock): the mixer makes it odd, writes
//! the slot, and makes it even again; a reader retries if the counter was odd or
//! changed while it read. Only atomics are used, so neither side ever waits on the
//! other for long, and nothing is allocated.
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering, fence};

use super::VoiceId;

/// What a voice is doing, as of the mixer's last render.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceState {
    Playing,
    /// Paused by itself or by its bus; its position does not move.
    Paused,
    /// Fading out after a stop or a steal.
    Stopping,
    Finished,
}

/// A voice's state and its position in its sound, in source frames. A looping voice's
/// position wraps back with the loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoiceStatus {
    pub state: VoiceState,
    pub position: u64,
}

const EMPTY: u64 = u64::MAX;

struct Slot {
    sequence: AtomicU64,
    id: AtomicU64,
    state: AtomicU8,
    position: AtomicU64,
}

pub(super) struct Table {
    slots: Box<[Slot]>,
}

impl Table {
    pub fn new(slots: usize) -> Self {
        Self {
            slots: (0..slots)
                .map(|_| Slot {
                    sequence: AtomicU64::new(0),
                    id: AtomicU64::new(EMPTY),
                    state: AtomicU8::new(0),
                    position: AtomicU64::new(0),
                })
                .collect(),
        }
    }
    /// Writes a slot. Only the mixer writes.
    pub fn publish(&self, slot: usize, id: VoiceId, status: VoiceStatus) {
        let s = &self.slots[slot];
        let sequence = s.sequence.load(Ordering::Relaxed);
        s.sequence
            .store(sequence.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        s.id.store(id.0, Ordering::Relaxed);
        s.state.store(status.state as u8, Ordering::Relaxed);
        s.position.store(status.position, Ordering::Relaxed);
        s.sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
    }
    /// The last status published for `id`, if a slot still holds it.
    pub fn read(&self, id: VoiceId) -> Option<VoiceStatus> {
        for s in self.slots.iter() {
            loop {
                let before = s.sequence.load(Ordering::Acquire);
                if before % 2 == 1 {
                    std::hint::spin_loop();
                    continue;
                }
                let slot_id = s.id.load(Ordering::Relaxed);
                let state = s.state.load(Ordering::Relaxed);
                let position = s.position.load(Ordering::Relaxed);
                fence(Ordering::Acquire);
                if s.sequence.load(Ordering::Relaxed) != before {
                    continue;
                }
                if slot_id == id.0 {
                    let state = match state {
                        0 => VoiceState::Playing,
                        1 => VoiceState::Paused,
                        2 => VoiceState::Stopping,
                        _ => VoiceState::Finished,
                    };
                    return Some(VoiceStatus { state, position });
                }
                break;
            }
        }
        None
    }
}
