#![cfg(feature = "sfx")]
use audiobox::ErrorCode;
use audiobox::sfx::{Effect, MAX_DURATION, Preset};
use audiobox::synth::{Aliasing, Duty, NoiseMode, Waveform};

const RATE: u32 = 48_000;

fn render(effect: &Effect) -> Vec<f32> {
    effect
        .render_with(RATE, Aliasing::Naive)
        .unwrap()
        .channel_data(0)
        .unwrap()
        .to_vec()
}

/// A full-volume pulse lasting `seconds`, with no decay.
fn tone(frequency: f64, seconds: f64) -> Effect {
    Effect {
        frequency,
        sustain: seconds,
        decay: 0.0,
        volume: 1.0,
        ..Effect::default()
    }
}

/// Mean frequency over `from..to` seconds, from the times of upward zero crossings.
fn frequency_between(samples: &[f32], from: f64, to: f64) -> f64 {
    let range = (from * RATE as f64) as usize..(to * RATE as f64) as usize;
    let crossings: Vec<f64> = range
        .filter(|&i| i > 0 && samples[i - 1] < 0.0 && samples[i] >= 0.0)
        .map(|i| {
            let (a, b) = (samples[i - 1] as f64, samples[i] as f64);
            i as f64 - 1.0 + a / (a - b)
        })
        .collect();
    let span = crossings[crossings.len() - 1] - crossings[0];
    (crossings.len() - 1) as f64 * RATE as f64 / span
}

/// Magnitude of the component at `hz` over `from..to` seconds.
fn level(samples: &[f32], hz: f64, from: f64, to: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    let start = (from * RATE as f64) as usize;
    for (n, &s) in samples
        .iter()
        .enumerate()
        .take((to * RATE as f64) as usize)
        .skip(start)
    {
        let w = 2.0 * std::f64::consts::PI * hz * n as f64 / RATE as f64;
        re += s as f64 * w.cos();
        im += s as f64 * w.sin();
    }
    (re * re + im * im).sqrt()
}

fn fnv1a(samples: &[f32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for s in samples {
        for byte in s.to_bits().to_le_bytes() {
            hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

#[test]
fn slide_changes_pitch_as_stated() {
    let close = |measured: f64, expected: f64, what: &str| {
        assert!(
            (measured / expected - 1.0).abs() < 0.003,
            "{what}: {measured} Hz instead of {expected} Hz"
        );
    };
    // One octave per second: 220 Hz doubles every second.
    let slide = render(&Effect {
        slide: 1.0,
        ..tone(220.0, 2.0)
    });
    for t in [0.25, 0.5, 1.0, 1.5] {
        close(
            frequency_between(&slide, t - 0.01, t + 0.01),
            220.0 * 2f64.powf(t),
            "slide",
        );
    }
    // A change of the slide of 2 octaves per second²: 2^(t²).
    let change = render(&Effect {
        slide_change: 2.0,
        ..tone(220.0, 1.5)
    });
    for t in [0.5, 1.0, 1.25] {
        close(
            frequency_between(&change, t - 0.01, t + 0.01),
            220.0 * 2f64.powf(t * t),
            "slide change",
        );
    }
    // An arpeggio of 7 semitones at 0.3 s, restarted by a repeat every 0.5 s.
    let arpeggio = render(&Effect {
        arpeggio: 7.0,
        arpeggio_time: 0.3,
        repeat: 0.5,
        ..tone(440.0, 1.0)
    });
    let fifth = 440.0 * 2f64.powf(7.0 / 12.0);
    close(
        frequency_between(&arpeggio, 0.05, 0.28),
        440.0,
        "before the arpeggio",
    );
    close(
        frequency_between(&arpeggio, 0.32, 0.48),
        fifth,
        "after the arpeggio",
    );
    close(
        frequency_between(&arpeggio, 0.55, 0.78),
        440.0,
        "after the repeat",
    );
    close(
        frequency_between(&arpeggio, 0.82, 0.98),
        fifth,
        "arpeggio after the repeat",
    );
}

#[test]
fn envelope_stages_give_the_length() {
    let effect = Effect {
        attack: 0.01,
        sustain: 0.02,
        punch: 0.5,
        decay: 0.03,
        volume: 1.0,
        ..Effect::default()
    };
    let s: Vec<f32> = render(&effect).iter().map(|v| v.abs()).collect();
    assert_eq!(s.len(), 480 + 960 + 1440);
    let close = |i: usize, expected: f32| {
        assert!(
            (s[i] - expected).abs() < 1e-6,
            "sample {i}: {} != {expected}",
            s[i]
        )
    };
    close(0, 0.0);
    close(240, 0.5);
    close(480, 1.5); // Sustain starts at 1 + punch...
    close(960, 1.25);
    close(1440, 1.0); // ...and falls to 1 when the decay starts.
    close(2160, 0.5);
    close(2879, 1.0 / 1440.0);
    let rounded = Effect {
        attack: 0.0,
        sustain: 0.000_01,
        decay: 0.000_031,
        ..effect
    };
    assert_eq!(render(&rounded).len(), 1); // 0.48 rounds to 0 and 1.488 to 1.
}

#[test]
fn filters_attenuate_as_stated() {
    // A 480 Hz pulse has exactly 100 samples per period, so a whole number of
    // periods fits each analysis window and the harmonics do not leak.
    let f = 480.0;
    let reference = render(&tone(f, 1.2));
    let fundamental = level(&reference, f, 0.5, 1.0);
    // Bilinear-transform Butterworth response, the filters' stated design.
    let warp = |hz: f64| (std::f64::consts::PI * hz / RATE as f64).tan();
    let low_pass = |cutoff: f64| 1.0 / (1.0 + (warp(f) / warp(cutoff)).powi(4)).sqrt();
    let high_pass = |cutoff: f64| {
        let r = (warp(f) / warp(cutoff)).powi(2);
        r / (1.0 + r * r).sqrt()
    };
    let cases = [
        (
            Effect {
                low_pass: f,
                ..tone(f, 1.2)
            },
            std::f64::consts::FRAC_1_SQRT_2,
        ),
        (
            Effect {
                low_pass: f / 4.0,
                ..tone(f, 1.2)
            },
            low_pass(f / 4.0),
        ),
        (
            Effect {
                low_pass: 4.0 * f,
                ..tone(f, 1.2)
            },
            low_pass(4.0 * f),
        ),
        (
            Effect {
                high_pass: f,
                ..tone(f, 1.2)
            },
            std::f64::consts::FRAC_1_SQRT_2,
        ),
        (
            Effect {
                high_pass: 4.0 * f,
                ..tone(f, 1.2)
            },
            high_pass(4.0 * f),
        ),
        (
            Effect {
                high_pass: f / 4.0,
                ..tone(f, 1.2)
            },
            high_pass(f / 4.0),
        ),
    ];
    for (effect, expected) in cases {
        let gain = level(&render(&effect), f, 0.5, 1.0) / fundamental;
        assert!(
            (gain / expected - 1.0).abs() < 0.01,
            "{effect}: gain {gain}, expected {expected}"
        );
    }
    assert!(
        low_pass(f / 4.0) < 0.07,
        "two octaves above the cutoff is about -24 dB"
    );
    // A sweep of 2 octaves per second moves a 120 Hz cutoff to 480 Hz after one second.
    let swept = render(&Effect {
        low_pass: f / 4.0,
        low_pass_sweep: 2.0,
        ..tone(f, 1.2)
    });
    let early = level(&swept, f, 0.1, 0.15) / level(&reference, f, 0.1, 0.15);
    let late = level(&swept, f, 0.975, 1.025) / level(&reference, f, 0.975, 1.025);
    assert!(
        (early / low_pass(f / 4.0 * 2f64.powf(0.25)) - 1.0).abs() < 0.05,
        "{early}"
    );
    assert!(
        (late / std::f64::consts::FRAC_1_SQRT_2 - 1.0).abs() < 0.03,
        "{late}"
    );
}

#[test]
fn duty_sweep_changes_the_duty() {
    let swept = render(&Effect {
        waveform: Waveform::Pulse(Duty::Eighth),
        duty_sweep: 0.5,
        ..tone(480.0, 1.0)
    });
    let high = |from: f64, to: f64| {
        let part = &swept[(from * RATE as f64) as usize..(to * RATE as f64) as usize];
        part.iter().filter(|&&v| v > 0.0).count() as f64 / part.len() as f64
    };
    assert!((high(0.0, 0.05) - 0.1375).abs() < 0.01);
    assert!((high(0.5, 0.55) - 0.3875).abs() < 0.01);
    assert!((high(0.9, 1.0) - 0.6).abs() < 0.01);
}

#[test]
fn text_round_trips() {
    let text = "
        ; A laser, keys in any order.
        slide=-4.5 wave=saw frequency=1200
        sustain=0.1 decay=0.15   high-pass=300.25
        volume=0.75 vibrato-depth=0.5 vibrato-speed=12
    ";
    let effect = Effect::parse(text).unwrap();
    assert_eq!(
        effect,
        Effect {
            waveform: Waveform::Sawtooth,
            frequency: 1200.0,
            slide: -4.5,
            sustain: 0.1,
            decay: 0.15,
            high_pass: 300.25,
            volume: 0.75,
            vibrato_depth: 0.5,
            vibrato_speed: 12.0,
            ..Effect::default()
        }
    );
    assert_eq!(Effect::parse(&effect.to_string()).unwrap(), effect);
    assert_eq!(Effect::parse("").unwrap(), Effect::default());

    let awkward = Effect {
        waveform: Waveform::Noise(NoiseMode::Short),
        frequency: 0.1 + 0.2 + 99.0,
        slide: -1e-7,
        slide_change: 255.999_999_999,
        attack: 1.0 / 3.0,
        punch: 0.000_000_1,
        low_pass: 12_345.678_9,
        low_pass_sweep: -0.0,
        repeat: 0.01,
        ..Effect::default()
    };
    let written = awkward.to_string();
    let read = Effect::parse(&written).unwrap();
    assert_eq!(read, awkward, "{written}");
    assert_eq!(read.to_string(), written);
    for kind in Preset::ALL {
        for seed in 0..50 {
            let effect = Effect::preset(kind, seed);
            assert_eq!(effect.to_string().parse::<Effect>().unwrap(), effect);
        }
    }
}

#[test]
fn out_of_range_values_are_rejected() {
    let cases: &[(&str, usize, usize, ErrorCode)] = &[
        ("frequency=5", 1, 11, ErrorCode::InvalidArgument),
        ("frequency=20001", 1, 11, ErrorCode::InvalidArgument),
        ("volume=1.5", 1, 8, ErrorCode::InvalidArgument),
        ("punch=-0.1", 1, 7, ErrorCode::InvalidArgument),
        ("low-pass=5", 1, 10, ErrorCode::InvalidArgument),
        ("repeat=0.001", 1, 8, ErrorCode::InvalidArgument),
        ("slide=NaN", 1, 7, ErrorCode::InvalidArgument),
        ("slide=inf", 1, 7, ErrorCode::InvalidArgument),
        ("arpeggio=49", 1, 10, ErrorCode::InvalidArgument),
        ("duty=33", 1, 6, ErrorCode::InvalidArgument),
        ("\n  wave=saw duty=25", 2, 12, ErrorCode::InvalidArgument),
        ("noise=short", 1, 1, ErrorCode::InvalidArgument),
        (
            "wave=triangle duty-sweep=1",
            1,
            15,
            ErrorCode::InvalidArgument,
        ),
        ("sustain=0 decay=0", 1, 11, ErrorCode::InvalidArgument),
        (
            "attack=4 sustain=4\ndecay=4",
            2,
            1,
            ErrorCode::InvalidArgument,
        ),
        ("frequency=fast", 1, 11, ErrorCode::Syntax),
        ("wave=sine", 1, 6, ErrorCode::Syntax),
        ("noise=pink wave=noise", 1, 7, ErrorCode::Syntax),
        ("frequency=100 colour=red", 1, 15, ErrorCode::Syntax),
        ("frequency 100", 1, 1, ErrorCode::Syntax),
        ("volume=0.5 volume=0.6", 1, 12, ErrorCode::Syntax),
        ("; é\n  é=1", 2, 3, ErrorCode::Syntax),
    ];
    for &(text, line, column, code) in cases {
        let e = Effect::parse(text).expect_err(text);
        assert_eq!(
            (e.line, e.column, e.code),
            (line, column, code),
            "{text:?}: {e}"
        );
    }
    let long = "volume=0.5 ".repeat(7000);
    assert_eq!(
        Effect::parse(&long).unwrap_err().code,
        ErrorCode::LimitExceeded
    );

    let invalid = [
        Effect {
            frequency: f64::NAN,
            ..Effect::default()
        },
        Effect {
            volume: -1.0,
            ..Effect::default()
        },
        Effect {
            decay: MAX_DURATION,
            ..Effect::default()
        },
        Effect {
            sustain: 0.0,
            decay: 0.0,
            ..Effect::default()
        },
        Effect {
            waveform: Waveform::Sawtooth,
            duty_sweep: 0.1,
            ..Effect::default()
        },
    ];
    for effect in invalid {
        assert_eq!(
            effect.validate().unwrap_err().code,
            ErrorCode::InvalidArgument,
            "{effect:?}"
        );
        assert_eq!(
            effect.render(RATE).unwrap_err().code,
            ErrorCode::InvalidArgument
        );
    }
    assert!(Effect::default().render(0).is_err());
}

#[test]
fn presets_are_deterministic_and_valid() {
    for kind in Preset::ALL {
        let a = Effect::preset(kind, 42);
        let b = Effect::preset(kind, 42);
        assert_eq!(a, b);
        let (sa, sb) = (a.render(22_050).unwrap(), b.render(22_050).unwrap());
        let (sa, sb) = (sa.channel_data(0).unwrap(), sb.channel_data(0).unwrap());
        assert!(sa.iter().zip(sb).all(|(x, y)| x.to_bits() == y.to_bits()));
        assert_ne!(Effect::preset(kind, 43), a, "{kind:?} seeds differ");
        for seed in 0..200 {
            let effect = Effect::preset(kind, seed);
            effect
                .validate()
                .unwrap_or_else(|e| panic!("{kind:?} {seed}: {e}"));
            let audio = effect.render(8_000).unwrap();
            assert!(audio.peak() > 0.0, "{kind:?} {seed} is silent");
        }
    }
    assert_ne!(
        Effect::preset(Preset::Jump, 1),
        Effect::preset(Preset::Blip, 1)
    );
}

#[test]
fn reference_effect_checksum() {
    let reference: Effect = "
        wave=pulse duty=25 frequency=523.25 slide=0.75 slide-change=-1.5
        vibrato-depth=0.4 vibrato-speed=9 attack=0.01 sustain=0.12 punch=0.35 decay=0.2
        duty-sweep=0.8 low-pass=5000 low-pass-sweep=-1.5 high-pass=150 high-pass-sweep=1
        arpeggio=12 arpeggio-time=0.08 repeat=0.15 volume=0.6
    "
    .parse()
    .unwrap();
    let audio = reference.render(44_100).unwrap();
    let samples = audio.channel_data(0).unwrap();
    assert_eq!(samples.len(), 441 + 5292 + 8820);
    let explosion = Effect::preset(Preset::Explosion, 1).render(44_100).unwrap();
    let explosion = explosion.channel_data(0).unwrap();
    assert_eq!(fnv1a(samples), 0xfe47_7734_44ac_5af6);
    // Also pins the preset generator's parameters for this seed.
    assert_eq!(fnv1a(explosion), 0x0f49_e023_a95b_4086);
}

#[test]
fn malformed_text_never_panics() {
    let alphabet: Vec<char> = "wavepulsnoiz=.-0123456789e ;\n_é".chars().collect();
    let base: Vec<char> = Effect::preset(Preset::Laser, 9)
        .to_string()
        .chars()
        .collect();
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut parsed = 0;
    for _ in 0..2000 {
        let mut text = base.clone();
        for _ in 0..1 + next() % 3 {
            let at = (next() % text.len() as u64) as usize;
            text[at] = alphabet[(next() % alphabet.len() as u64) as usize];
        }
        let text: String = text.into_iter().collect();
        if let Ok(effect) = Effect::parse(&text) {
            effect.render(8_000).unwrap();
            parsed += 1;
        }
    }
    assert!(parsed > 50, "{parsed}");
}
