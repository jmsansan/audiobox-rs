//! The offline transforms that render audio compute with the crate's own math, so they
//! give the same samples on every platform. Each matches the platform-math result
//! within a stated tolerance, and its output is pinned by a checksum: a platform that
//! rounds differently fails here.
use audiobox::*;
use std::f64::consts::PI;

/// A stereo test signal built from integers only: a square wave, a slow ramp and
/// linear-congruential noise.
fn signal(frames: usize, rate: u32) -> Audio {
    let mut seed = 0x2545_f491u32;
    let mut noise = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    };
    let left = (0..frames)
        .map(|i| if (i / 50) % 2 == 0 { 0.4 } else { -0.4 } + noise() * 0.1 + (i % 1000) as f32 / 4000.0)
        .collect();
    let right = (0..frames)
        .map(|i| noise() * 0.5 + if (i / 73) % 2 == 0 { 0.2 } else { -0.2 })
        .collect();
    Audio::new(vec![left, right], rate).unwrap()
}

fn fnv1a(audio: &Audio) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for channel in audio.all_channels() {
        for s in channel {
            for byte in s.to_bits().to_le_bytes() {
                hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    hash
}

fn close(a: &[f32], b: &[f64], tolerance: f64) {
    assert_eq!(a.len(), b.len());
    let worst = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| (x as f64 - y).abs())
        .fold(0.0, f64::max);
    assert!(worst <= tolerance, "worst difference {worst}");
}

const TOLERANCE: f64 = 1e-6;

#[test]
fn pan_gain_fades_and_crossfades_match_platform_math() {
    let audio = signal(9000, 48_000);
    let mono = audio.to_mono();
    let m = mono.channel_data(0).unwrap();
    // pan: cos θ and sin θ, θ = (p + 1)·π/4.
    let angle = (0.3f64 + 1.0) * PI / 4.0;
    let panned = mono.pan(0.3).unwrap();
    close(
        panned.channel_data(0).unwrap(),
        &m.iter()
            .map(|&s| s as f64 * angle.cos())
            .collect::<Vec<_>>(),
        TOLERANCE,
    );
    close(
        panned.channel_data(1).unwrap(),
        &m.iter()
            .map(|&s| s as f64 * angle.sin())
            .collect::<Vec<_>>(),
        TOLERANCE,
    );
    // gain_db: 10^(dB / 20).
    let gained = audio.gain_db(-4.5).unwrap();
    let factor = 10f64.powf(-4.5 / 20.0);
    close(
        gained.channel_data(0).unwrap(),
        &audio
            .channel_data(0)
            .unwrap()
            .iter()
            .map(|&s| s as f64 * factor)
            .collect::<Vec<_>>(),
        TOLERANCE,
    );
    assert!((audiobox::analyze::from_db(-4.5) - factor).abs() < 1e-15);
    // An equal-power fade-in of 1000 frames: sin(t·π/2).
    let faded = mono
        .fade(1000.0 / 48_000.0, 0.0, FadeCurve::EqualPower)
        .unwrap();
    let expected: Vec<f64> = m
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            if i < 1000 {
                s as f64 * (i as f64 / 999.0 * PI / 2.0).sin()
            } else {
                s as f64
            }
        })
        .collect();
    close(faded.channel_data(0).unwrap(), &expected, TOLERANCE);
    // An equal-power crossfade of 500 frames.
    let other = audio.reverse().to_mono();
    let mixed = mono
        .crossfade_to(&other, audiobox::TimePosition::Frames(500))
        .unwrap();
    let o = other.channel_data(0).unwrap();
    let n = 500;
    let mut expected: Vec<f64> = m[..m.len() - n].iter().map(|&s| s as f64).collect();
    for i in 0..n {
        let t = i as f64 / (n - 1) as f64 * PI / 2.0;
        expected.push(m[m.len() - n + i] as f64 * t.cos() + o[i] as f64 * t.sin());
    }
    expected.extend(o[n..].iter().map(|&s| s as f64));
    close(mixed.channel_data(0).unwrap(), &expected, TOLERANCE);
}

/// The RBJ cookbook coefficients with platform math, run through the same recursion.
fn reference_filter(
    input: &[f32],
    rate: f64,
    kind: FilterType,
    frequency: f64,
    q: f64,
    gain_db: f64,
) -> Vec<f64> {
    let w = 2.0 * PI * frequency / rate;
    let (s, c) = (w.sin(), w.cos());
    let alpha = s / (2.0 * q);
    let a = 10f64.powf(gain_db / 40.0);
    let beta = 2.0 * a.sqrt() * alpha;
    let (b0, b1, b2, a0, a1, a2) = match kind {
        FilterType::Lowpass => (
            (1.0 - c) / 2.0,
            1.0 - c,
            (1.0 - c) / 2.0,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        ),
        FilterType::Highpass => (
            (1.0 + c) / 2.0,
            -(1.0 + c),
            (1.0 + c) / 2.0,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        ),
        FilterType::Peaking => (
            1.0 + alpha * a,
            -2.0 * c,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * c,
            1.0 - alpha / a,
        ),
        FilterType::Lowshelf => (
            a * ((a + 1.0) - (a - 1.0) * c + beta),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - beta),
            (a + 1.0) + (a - 1.0) * c + beta,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - beta,
        ),
        _ => unreachable!(),
    };
    let (b0, b1, b2, a1, a2) = (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0);
    let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
    input
        .iter()
        .map(|&s| {
            let x = s as f64;
            let y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            (x2, x1, y2, y1) = (x1, x, y1, y);
            y as f32 as f64
        })
        .collect()
}

#[test]
fn filters_and_the_limiter_match_platform_math() {
    let audio = signal(9000, 44_100).to_mono();
    let input = audio.channel_data(0).unwrap();
    for (kind, frequency, gain) in [
        (FilterType::Lowpass, 1200.0, 0.0),
        (FilterType::Highpass, 300.0, 0.0),
        (FilterType::Peaking, 2500.0, 6.0),
        (FilterType::Lowshelf, 200.0, -4.0),
    ] {
        let mut options = FilterOptions::new(kind, frequency);
        options.gain_db = gain;
        let out = audio.filter(options).unwrap();
        let expected = reference_filter(input, 44_100.0, kind, frequency, options.q, gain);
        close(out.channel_data(0).unwrap(), &expected, TOLERANCE);
    }
    // The limiter: a look-ahead window maximum, instant attack and exponential release.
    let loud = audio.gain(3.0).unwrap();
    let options = LimiterOptions::default();
    let limited = loud.limit(options).unwrap();
    let ceiling = 10f64.powf(options.ceiling_db / 20.0);
    let look = (options.lookahead_ms * 44.1).round() as usize;
    let release = (-1.0 / (options.release_ms * 44.1)).exp();
    let x = loud.channel_data(0).unwrap();
    let mut gain = 1.0f64;
    let expected: Vec<f64> = (0..x.len())
        .map(|i| {
            let peak = x[i..(i + look + 1).min(x.len())]
                .iter()
                .map(|s| s.abs() as f64)
                .fold(0.0, f64::max);
            let target = if peak > ceiling { ceiling / peak } else { 1.0 };
            gain = if target < gain {
                target
            } else {
                target + (gain - target) * release
            };
            (x[i] * gain as f32) as f64
        })
        .collect();
    close(limited.channel_data(0).unwrap(), &expected, TOLERANCE);
}

#[test]
fn each_transform_is_pinned_by_a_checksum() {
    let audio = signal(24_000, 48_000);
    let mut other = signal(12_000, 48_000).reverse();
    other = other.gain(0.5).unwrap();
    let silence_padded = audio.pad(0.2, 0.3).unwrap();
    let mut filters = Vec::new();
    for (kind, frequency) in [
        (FilterType::Lowpass, 1000.0),
        (FilterType::Highpass, 400.0),
        (FilterType::Bandpass, 1500.0),
        (FilterType::Notch, 3000.0),
        (FilterType::Peaking, 2500.0),
        (FilterType::Lowshelf, 200.0),
        (FilterType::Highshelf, 6000.0),
        (FilterType::Allpass, 800.0),
    ] {
        let mut options = FilterOptions::new(kind, frequency);
        options.gain_db = 5.0;
        filters.push(audio.filter(options).unwrap());
    }
    let results = [
        ("pan", audio.to_mono().pan(-0.4).unwrap()),
        ("gain_db", audio.gain_db(-7.25).unwrap()),
        (
            "fade",
            audio.fade(0.05, 0.1, FadeCurve::EqualPower).unwrap(),
        ),
        ("crossfade_to", audio.crossfade_to(&other, 0.05).unwrap()),
        ("filters", Audio::merge(&filters).unwrap()),
        (
            "limit",
            audio
                .gain(4.0)
                .unwrap()
                .limit(LimiterOptions::default())
                .unwrap(),
        ),
        ("resample up", audio.resample(48_000 * 3 / 2).unwrap()),
        ("resample down", audio.resample(22_050).unwrap()),
        ("tempo", audio.tempo(1.25).unwrap()),
        ("pitch", audio.pitch(3.0).unwrap()),
        (
            "trim_silence",
            silence_padded
                .trim_silence(SilenceOptions::default())
                .unwrap(),
        ),
        (
            "normalize",
            audio.normalize(NormalizeOptions::default()).unwrap(),
        ),
    ];
    let got: Vec<(&str, u64)> = results
        .iter()
        .map(|(name, audio)| (*name, fnv1a(audio)))
        .collect();
    let expected: &[(&str, u64)] = &[
        ("pan", 0x7b29349d6e3991cb),
        ("gain_db", 0x64a25ceb112b2bcc),
        ("fade", 0xf910d23b508db6c4),
        ("crossfade_to", 0x60bedad87a6f7b4f),
        ("filters", 0x4f25f0deb7d3290e),
        ("limit", 0x49f531c6ff0074c8),
        ("resample up", 0xb69a6f00728985fe),
        ("resample down", 0xbe895d11c6324da2),
        ("tempo", 0x0ef8acde31695b36),
        ("pitch", 0xb3c79a8192334521),
        ("trim_silence", 0xba54a8e873bcc0ad),
        ("normalize", 0x1137c445ac833b9c),
    ];
    for ((name, hash), (_, want)) in got.iter().zip(expected) {
        assert_eq!(hash, want, "{name} changed: {hash:#018x}");
    }
    assert_eq!(got.len(), expected.len());
}
