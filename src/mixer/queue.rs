//! A bounded single-producer, single-consumer queue between the game and the audio side.
//!
//! The producer and the consumer each own one counter: the producer writes slot
//! `tail % capacity` and then publishes `tail + 1` with release ordering; the consumer
//! reads `tail` with acquire ordering, takes the slot, and publishes `head + 1` the same
//! way. A slot is therefore only ever touched by one side at a time. Each slot is a
//! `Mutex<Option<T>>` taken with `try_lock`, which the standard library documents never
//! to block; under this protocol it is never contended, so it succeeds at once, as a
//! single atomic operation without a system call. That is the safe-Rust way to move
//! values such as boxed voices between threads, since the crate forbids `unsafe`.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, TryLockError};

struct Ring<T> {
    slots: Box<[Mutex<Option<T>>]>,
    /// Next slot to read. Written only by the consumer.
    head: AtomicUsize,
    /// Next slot to write. Written only by the producer.
    tail: AtomicUsize,
}

impl<T> Ring<T> {
    fn with_slot<R>(&self, index: usize, f: impl FnOnce(&mut Option<T>) -> R) -> Option<R> {
        let slot = &self.slots[index % self.slots.len()];
        match slot.try_lock() {
            Ok(mut guard) => Some(f(&mut guard)),
            // Assigning or taking an `Option` cannot panic, so a poisoned slot still
            // holds a consistent value.
            Err(TryLockError::Poisoned(poisoned)) => Some(f(&mut poisoned.into_inner())),
            // Not reachable under the protocol; treated as "not ready" rather than waiting.
            Err(TryLockError::WouldBlock) => None,
        }
    }
}

/// The sending half. Never blocks: a full queue gives the value back.
pub(super) struct Producer<T> {
    ring: Arc<Ring<T>>,
}

/// The receiving half. Never blocks and never allocates.
pub(super) struct Consumer<T> {
    ring: Arc<Ring<T>>,
}

/// Why a value was not sent.
pub(super) enum PushError<T> {
    Full(T),
    Disconnected(T),
}

/// Makes a queue of `capacity` values. All of its memory is allocated here.
pub(super) fn queue<T>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let capacity = capacity.max(1);
    let slots: Box<[Mutex<Option<T>>]> = (0..capacity).map(|_| Mutex::new(None)).collect();
    // Some platforms allocate a mutex the first time it is locked: lock each slot once
    // now, on the creating thread, so the audio thread never does.
    for slot in slots.iter() {
        drop(slot.lock());
    }
    let ring = Arc::new(Ring {
        slots,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
    });
    (Producer { ring: ring.clone() }, Consumer { ring })
}

impl<T> Producer<T> {
    pub fn push(&mut self, value: T) -> Result<(), PushError<T>> {
        if Arc::strong_count(&self.ring) < 2 {
            return Err(PushError::Disconnected(value));
        }
        let ring = &self.ring;
        let tail = ring.tail.load(Ordering::Relaxed);
        let head = ring.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) >= ring.slots.len() {
            return Err(PushError::Full(value));
        }
        let mut value = Some(value);
        if ring.with_slot(tail, |slot| *slot = value.take()).is_none() {
            return Err(PushError::Full(
                value.take().expect("the value was not stored"),
            ));
        }
        ring.tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }
}

impl<T> Consumer<T> {
    pub fn pop(&mut self) -> Option<T> {
        let ring = &self.ring;
        let head = ring.head.load(Ordering::Relaxed);
        let tail = ring.tail.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        let value = ring.with_slot(head, Option::take)?;
        ring.head.store(head.wrapping_add(1), Ordering::Release);
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_arrive_in_order_and_fullness_is_reported() {
        let (mut tx, mut rx) = queue::<u32>(3);
        for i in 0..3 {
            assert!(tx.push(i).is_ok());
        }
        assert!(matches!(tx.push(3), Err(PushError::Full(3))));
        assert_eq!(rx.pop(), Some(0));
        assert!(tx.push(3).is_ok());
        assert_eq!(
            [rx.pop(), rx.pop(), rx.pop(), rx.pop()],
            [Some(1), Some(2), Some(3), None]
        );
        drop(rx);
        assert!(matches!(tx.push(4), Err(PushError::Disconnected(4))));
    }

    #[test]
    fn two_threads_lose_and_reorder_nothing() {
        const COUNT: u64 = 300_000;
        let (mut tx, mut rx) = queue::<Box<u64>>(64);
        let producer = std::thread::spawn(move || {
            for i in 0..COUNT {
                let mut value = Box::new(i);
                loop {
                    match tx.push(value) {
                        Ok(()) => break,
                        Err(PushError::Full(back)) => {
                            value = back;
                            std::thread::yield_now();
                        }
                        Err(PushError::Disconnected(_)) => panic!("consumer dropped"),
                    }
                }
            }
        });
        let mut expected = 0;
        while expected < COUNT {
            match rx.pop() {
                Some(value) => {
                    assert_eq!(*value, expected);
                    expected += 1;
                }
                None => std::thread::yield_now(),
            }
        }
        producer.join().unwrap();
        assert_eq!(rx.pop(), None);
    }

    #[test]
    fn values_left_in_the_queue_are_dropped() {
        let counter = Arc::new(());
        let (mut tx, rx) = queue(4);
        tx.push(counter.clone()).ok();
        tx.push(counter.clone()).ok();
        drop((tx, rx));
        assert_eq!(Arc::strong_count(&counter), 1);
    }
}
