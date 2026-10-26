use crate::{Error, Result};

/// Shared fixed-tempo control precision; both prepared synth and music transport.
pub(crate) fn milli_bpm(bpm: f64) -> Result<u32> {
    if !bpm.is_finite() || !(1.0..=1000.0).contains(&bpm) {
        return Err(Error::invalid("Tempo must be 1..=1000 BPM"));
    }
    Ok((bpm * 1000.0).round() as u32)
}

/// Nearest frame, halves up, without overflowing numerator + denominator/2.
pub(crate) fn rounded_ratio(numerator: u128, denominator: u128) -> Option<u64> {
    let whole = numerator / denominator;
    let up = numerator % denominator >= denominator.div_ceil(2);
    u64::try_from(whole.checked_add(up as u128)?).ok()
}
