#![allow(dead_code)] // Each feature uses part of this module.
//! Deterministic replacements for platform math. Only `+`, `-`, `*`, `/` and `floor`
//! are used, which IEEE 754 rounds identically on every target.
use std::f64::consts::{LN_2, TAU};

/// `2^x`, accurate to about 1e-15 relative error.
pub(crate) fn exp2(x: f64) -> f64 {
    let whole = x.floor();
    let f = (x - whole) * LN_2;
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..=18 {
        term = term * f / k as f64;
        sum += term;
    }
    let exponent = (whole as i64).clamp(-1022, 1023);
    sum * f64::from_bits(((exponent + 1023) as u64) << 52)
}

/// `sin(2πx)`, accurate to about 1e-11.
pub(crate) fn sin_turns(x: f64) -> f64 {
    let mut q = x - x.floor();
    if q > 0.75 {
        q -= 1.0;
    } else if q > 0.25 {
        q = 0.5 - q;
    }
    let r = q * TAU;
    let r2 = r * r;
    let (mut sum, mut term) = (r, r);
    for k in 1..=8 {
        term = -term * r2 / ((2 * k) * (2 * k + 1)) as f64;
        sum += term;
    }
    sum
}

/// `cos(2πx)`.
pub(crate) fn cos_turns(x: f64) -> f64 {
    sin_turns(x + 0.25)
}

/// Frequency in Hz of a pitch `cents` above or below A4 (440 Hz).
pub(crate) fn cents_to_hz(cents: f64) -> f64 {
    440.0 * exp2(cents / 1200.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn match_platform_math() {
        for i in -400..400 {
            let x = i as f64 / 37.0;
            assert!((exp2(x) / x.exp2() - 1.0).abs() < 1e-13, "exp2({x})");
            assert!((sin_turns(x) - (TAU * x).sin()).abs() < 1e-10, "sin({x})");
            assert!((cos_turns(x) - (TAU * x).cos()).abs() < 1e-10, "cos({x})");
        }
        assert_eq!(cents_to_hz(0.0), 440.0);
        assert_eq!(cents_to_hz(1200.0), 880.0);
    }
}
