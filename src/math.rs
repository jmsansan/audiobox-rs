#![allow(dead_code)] // Each feature uses part of this module.
//! Deterministic replacements for platform math. Only `+`, `-`, `*`, `/` and `floor`
//! are used, which IEEE 754 rounds identically on every target.
use std::f64::consts::{LN_2, TAU};

/// `2^x`, accurate to about 1e-15 relative error. Infinities and results beyond the
/// `f64` range give infinity or 0, as `f64::exp2` does, and NaN stays NaN.
pub(crate) fn exp2(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x >= 1024.0 {
        return f64::INFINITY;
    }
    if x < -1075.0 {
        return 0.0;
    }
    let whole = x.floor();
    let f = (x - whole) * LN_2;
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..=18 {
        term = term * f / k as f64;
        sum += term;
    }
    let power = |e: i64| f64::from_bits(((e + 1023) as u64) << 52);
    let exponent = whole as i64;
    if exponent >= -1022 {
        sum * power(exponent)
    } else {
        // Subnormal results: scale exactly to the smallest normal power first, so the
        // second multiplication rounds once.
        sum * power(-1022) * power(exponent + 1022)
    }
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

/// `10^(db / 20)`: the linear gain of a level in decibels.
pub(crate) fn db_to_gain(db: f64) -> f64 {
    exp2(db * std::f64::consts::LOG2_10 / 20.0)
}

/// `e^x`.
pub(crate) fn exp(x: f64) -> f64 {
    exp2(x * std::f64::consts::LOG2_E)
}

/// Deterministic base-two logarithm. The atanh series converges with |z| <= 1/3.
pub(crate) fn log2(mut x: f64) -> f64 {
    if x <= 0.0 || !x.is_finite() {
        return if x == 0.0 {
            f64::NEG_INFINITY
        } else if x > 0.0 {
            x
        } else {
            f64::NAN
        };
    }
    let mut offset = 0;
    if x < f64::MIN_POSITIVE {
        x *= (1u64 << 52) as f64;
        offset = -52;
    }
    let bits = x.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32 - 1023 + offset;
    let m = f64::from_bits((bits & ((1u64 << 52) - 1)) | (1023u64 << 52));
    let z = (m - 1.0) / (m + 1.0);
    let mut term = z;
    let mut sum = z;
    for k in 1..=17 {
        term *= z * z;
        sum += term / (2 * k + 1) as f64;
    }
    exponent as f64 + 2.0 * sum * std::f64::consts::LOG2_E
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
        for x in [1023.5, 1100.0, -1030.0, -1030.5, -1074.0, -1080.0, -0.0] {
            let (ours, platform) = (exp2(x), x.exp2());
            let close = ours == platform || (ours / platform - 1.0).abs() < 1e-14;
            assert!(close, "exp2({x}): {ours} vs {platform}");
        }
        assert_eq!(exp2(f64::NEG_INFINITY), 0.0);
        assert_eq!(exp2(f64::INFINITY), f64::INFINITY);
        assert!(exp2(f64::NAN).is_nan());
        assert_eq!(db_to_gain(f64::NEG_INFINITY), 0.0);
        assert_eq!(cents_to_hz(0.0), 440.0);
        assert_eq!(cents_to_hz(1200.0), 880.0);
        for x in [1e-320f64, 1e-100, 0.001, 0.5, 1.0, 1.99, 100.0, 1e100] {
            assert!((log2(x) - x.log2()).abs() < 2e-13, "log2({x})");
        }
    }
}
