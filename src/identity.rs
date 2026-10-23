/// Internal sound/group identities share the upper half of the key space.
/// Caller-selected mixer instance keys occupy the lower half.
pub(crate) fn next() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1 << 63);
    // Never wrap into caller keys, even in the theoretical exhaustion case.
    let mut current = NEXT.load(Ordering::Relaxed);
    loop {
        let next = current.checked_add(1).expect("sound identities exhausted");
        match NEXT.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return current,
            Err(actual) => current = actual,
        }
    }
}
