//! Independent response, automation and determinism checks for colour effects.
#![cfg(feature = "mixer")]
use audiobox::Audio;
use audiobox::mixer::effects::*;
use audiobox::mixer::*;
use std::f64::consts::{PI, TAU};

const RATE: u32 = 48_000;
fn mixer(rate: u32, channels: usize) -> (Mixer, MixerHandle) {
    Mixer::new(MixerConfig {
        sample_rate: rate,
        channels,
        ..Default::default()
    })
    .unwrap()
}
fn process(data: Vec<Vec<f32>>, effect: Effect, frames: usize, rate: u32) -> Vec<f32> {
    let (mut m, mut h) = mixer(rate, data.len());
    h.set_effects(BusId::MASTER, &[effect]).unwrap();
    h.play(Audio::new(data, rate).unwrap(), &Play::default())
        .unwrap();
    let mut out = vec![0.0; frames * m.channels()];
    m.render(&mut out);
    out
}
fn sine(hz: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| (TAU * hz * n as f64 / RATE as f64).sin() as f32 * 0.2)
        .collect()
}
fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}

#[test]
fn chorus_has_two_taps_and_independent_stereo_phase() {
    let effect = Effect::Chorus(Chorus {
        rate: ModulationRate::Hertz(0.0),
        delay_ms: 10.0,
        depth_ms: 2.0,
        feedback: 0.0,
        stereo_phase: 0.5,
        mix: 1.0,
    });
    let out = process(vec![vec![1.0], vec![0.5]], effect, 800, RATE);
    for (n, pair) in out.chunks_exact(2).enumerate() {
        assert!(
            (pair[0] - if n == 480 || n == 576 { 0.5 } else { 0.0 }).abs() < 1e-7,
            "left {n}"
        );
        assert!(
            (pair[1] - if n == 384 || n == 480 { 0.25 } else { 0.0 }).abs() < 1e-7,
            "right {n}"
        );
    }
    let silent_right = process(vec![vec![1.0], vec![0.0]], effect, 800, RATE);
    assert!(silent_right.chunks_exact(2).all(|p| p[1] == 0.0));
}

#[test]
fn flanger_impulse_feedback_and_comb_response() {
    let flanger = Flanger {
        rate: ModulationRate::Hertz(0.0),
        delay_ms: 2.0,
        depth_ms: 0.0,
        feedback: 0.5,
        stereo_phase: 0.0,
        mix: 0.5,
    };
    let out = process(vec![vec![1.0]], Effect::Flanger(flanger), 481, RATE);
    for (n, &v) in out.iter().enumerate() {
        let expected = if n == 0 {
            0.5
        } else if n % 96 == 0 {
            0.5f32.powi((n / 96) as i32)
        } else {
            0.0
        };
        assert_eq!(v, expected, "echo {n}");
    }
    for (hz, expected) in [(250.0, 0.0), (500.0, 1.0)] {
        let input = sine(hz, RATE as usize);
        let level = rms(&input[24_000..]);
        let out = process(
            vec![input],
            Effect::Flanger(Flanger {
                feedback: 0.0,
                ..flanger
            }),
            RATE as usize,
            RATE,
        );
        assert!((rms(&out[24_000..]) / level - expected).abs() < 1e-6);
    }
}

#[test]
fn phaser_allpass_magnitude_and_two_blended_notches() {
    let phaser = Phaser {
        rate: ModulationRate::Hertz(0.0),
        centre_hz: 1000.0,
        depth_octaves: 0.0,
        feedback: 0.0,
        stereo_phase: 0.0,
        mix: 1.0,
    };
    for hz in [100.0, 1000.0, 6000.0] {
        let input = sine(hz, RATE as usize);
        let level = rms(&input[24_000..]);
        let out = process(vec![input], Effect::Phaser(phaser), RATE as usize, RATE);
        assert!(
            (rms(&out[24_000..]) / level - 1.0).abs() < 1e-6,
            "allpass {hz}"
        );
    }
    // Four identical first-order allpasses have -pi/-3pi total phase at these
    // bilinear frequencies, so a half dry/half wet blend cancels the input.
    let g = (PI * 1000.0 / RATE as f64).tan();
    for angle in [PI / 8.0, 3.0 * PI / 8.0] {
        let hz = RATE as f64 / PI * (g * angle.tan()).atan();
        let input = sine(hz, RATE as usize);
        let level = rms(&input[24_000..]);
        let out = process(
            vec![input],
            Effect::Phaser(Phaser { mix: 0.5, ..phaser }),
            RATE as usize,
            RATE,
        );
        assert!(rms(&out[24_000..]) / level < 1e-6, "notch {hz}");
    }
}

#[test]
fn beat_periods_match_hertz_and_tempo_updates_preserve_phase() {
    let rate = 1000;
    let input: Vec<f32> = (0..3000).map(|n| n as f32 / 10_000.0).collect();
    let base = Flanger {
        rate: ModulationRate::Hertz(1.0),
        delay_ms: 10.0,
        depth_ms: 3.0,
        feedback: 0.0,
        stereo_phase: 0.0,
        mix: 1.0,
    };
    let beats = Flanger {
        rate: ModulationRate::Beats {
            beats: 2.0,
            bpm: 120.0,
        },
        ..base
    };
    assert_eq!(
        process(vec![input.clone()], Effect::Flanger(base), 3000, rate),
        process(vec![input.clone()], Effect::Flanger(beats), 3000, rate)
    );
    let (mut m, mut h) = mixer(rate, 1);
    h.set_effects(BusId::MASTER, &[Effect::Flanger(beats)])
        .unwrap();
    h.play(Audio::new(vec![input], rate).unwrap(), &Play::default())
        .unwrap();
    h.at(1234)
        .set_effect(
            BusId::MASTER,
            0,
            Effect::Flanger(Flanger {
                rate: ModulationRate::Beats {
                    beats: 2.0,
                    bpm: 240.0,
                },
                ..base
            }),
            Fade::NONE,
        )
        .unwrap();
    let mut out = vec![0.0; 3000];
    m.render(&mut out);
    for n in [500, 1233, 1234, 1500, 2500] {
        let phase = if n < 1234 {
            n as f64 / 1000.0
        } else {
            1.234 + (n - 1234) as f64 / 500.0
        };
        let delay = 10.0 + 3.0 * (TAU * phase).sin();
        let expected = (n as f64 - delay) / 10_000.0;
        assert!((out[n] as f64 - expected).abs() < 3e-8, "phase at {n}");
    }
}

#[test]
fn bitcrusher_has_exact_signed_levels_and_blends_fractional_bits() {
    for bits in [2u32, 3, 8, 12] {
        let scale = 1 << (bits - 1);
        let input: Vec<f32> = (-scale..scale).map(|n| n as f32 / scale as f32).collect();
        let n = input.len();
        let out = process(
            vec![input.clone()],
            Effect::Bitcrusher(Bitcrusher {
                bits: bits as f32,
                rate_ratio: 1.0,
                mix: 1.0,
            }),
            n,
            RATE,
        );
        assert_eq!(out, input);
        let levels: std::collections::BTreeSet<_> = out.iter().map(|v| v.to_bits()).collect();
        assert_eq!(levels.len(), 1 << bits);
    }
    let input = vec![
        -3.0, -1.0, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0, 3.0,
    ];
    assert_eq!(
        process(
            vec![input.clone()],
            Effect::Bitcrusher(Bitcrusher {
                bits: 2.0,
                rate_ratio: 1.0,
                mix: 1.0
            }),
            input.len(),
            RATE
        ),
        [-1.0, -1.0, -1.0, -0.5, -0.5, 0.0, 0.5, 0.5, 0.5, 0.5, 0.5]
    );
    let input: Vec<_> = (0..1000).map(|n| n as f32 / 500.0 - 1.0).collect();
    let crush = |bits| {
        process(
            vec![input.clone()],
            Effect::Bitcrusher(Bitcrusher {
                bits,
                rate_ratio: 1.0,
                mix: 1.0,
            }),
            input.len(),
            RATE,
        )
    };
    for ((low, high), blended) in crush(4.0).iter().zip(crush(5.0)).zip(crush(4.5)) {
        assert_eq!((*low + high) * 0.5, blended);
    }
}

#[test]
fn reducer_captures_first_frame_and_holds_stereo_together() {
    let input: Vec<f32> = (0..20).map(|n| n as f32 / 32.0).collect();
    let out = process(
        vec![input.clone(), input.iter().map(|v| -v).collect()],
        Effect::Bitcrusher(Bitcrusher {
            bits: 8.0,
            rate_ratio: 0.25,
            mix: 1.0,
        }),
        20,
        RATE,
    );
    for (n, pair) in out.chunks_exact(2).enumerate() {
        assert_eq!(pair[0], input[n / 4 * 4]);
        assert_eq!(pair[1], -pair[0]);
    }
    let out = process(
        vec![input.clone()],
        Effect::Bitcrusher(Bitcrusher {
            bits: 8.0,
            rate_ratio: 0.3,
            mix: 1.0,
        }),
        20,
        RATE,
    );
    // First capture at zero, then ceil(k / 0.3), retaining the residual phase.
    let captures = [
        0, 0, 0, 0, 4, 4, 4, 7, 7, 7, 10, 10, 10, 10, 14, 14, 14, 17, 17, 17,
    ];
    for (n, &capture) in captures.iter().enumerate() {
        assert_eq!(out[n], input[capture]);
    }
}

#[test]
fn saturation_curve_symmetry_and_third_harmonic_match_the_polynomial() {
    let input: Vec<f32> = (-100..=100).map(|n| n as f32 / 20.0).collect();
    let settings = Saturation {
        drive_db: 12.0,
        output_db: -6.0,
        mix: 0.6,
    };
    let out = process(
        vec![input.clone()],
        Effect::Saturation(settings),
        input.len(),
        RATE,
    );
    for (&x, &y) in input.iter().zip(&out) {
        let v = x as f64 * 10f64.powf(12.0 / 20.0);
        let wet = if v.abs() >= 1.0 {
            v.signum() * 2.0 / 3.0
        } else {
            v - v.powi(3) / 3.0
        } * 10f64.powf(-6.0 / 20.0);
        let expected = x as f64 * (1.0 - 0.6f32 as f64) + wet * 0.6f32 as f64;
        assert!((y as f64 - expected).abs() < 2e-7);
    }
    for (&positive, &negative) in out.iter().zip(out.iter().rev()) {
        assert_eq!(positive, -negative);
    }
    let input: Vec<f32> = (0..RATE)
        .map(|n| (TAU * 512.0 * n as f64 / RATE as f64).sin() as f32 * 0.6)
        .collect();
    let out = process(
        vec![input],
        Effect::Saturation(Saturation {
            drive_db: 0.0,
            output_db: 0.0,
            mix: 1.0,
        }),
        RATE as usize,
        RATE,
    );
    let amplitude = |hz: f64| {
        2.0 / RATE as f64
            * out
                .iter()
                .enumerate()
                .map(|(n, &v)| v as f64 * (TAU * hz * n as f64 / RATE as f64).sin())
                .sum::<f64>()
    };
    assert!((amplitude(512.0) - (0.6 - 0.6f64.powi(3) / 4.0)).abs() < 1e-7);
    assert!((amplitude(1536.0) - 0.6f64.powi(3) / 12.0).abs() < 1e-7);
}

#[test]
fn parameter_ramps_preserve_state_and_do_not_add_abrupt_jumps() {
    let chorus = Chorus {
        delay_ms: 10.0,
        depth_ms: 2.0,
        feedback: 0.0,
        mix: 0.5,
        ..Default::default()
    };
    let flanger = Flanger {
        feedback: 0.0,
        ..Default::default()
    };
    let phaser = Phaser {
        feedback: 0.0,
        ..Default::default()
    };
    for (initial, target) in [
        (
            Effect::Chorus(chorus),
            Effect::Chorus(Chorus {
                delay_ms: 20.0,
                depth_ms: 4.0,
                ..chorus
            }),
        ),
        (
            Effect::Flanger(flanger),
            Effect::Flanger(Flanger {
                delay_ms: 8.0,
                depth_ms: 4.0,
                ..flanger
            }),
        ),
        (
            Effect::Phaser(phaser),
            Effect::Phaser(Phaser {
                centre_hz: 4000.0,
                ..phaser
            }),
        ),
        (
            Effect::Bitcrusher(Bitcrusher {
                bits: 8.0,
                rate_ratio: 1.0,
                mix: 1.0,
            }),
            Effect::Bitcrusher(Bitcrusher {
                bits: 2.0,
                rate_ratio: 1.0,
                mix: 1.0,
            }),
        ),
        (
            Effect::Saturation(Saturation {
                drive_db: 0.0,
                output_db: 0.0,
                mix: 1.0,
            }),
            Effect::Saturation(Saturation {
                drive_db: 24.0,
                output_db: -6.0,
                mix: 1.0,
            }),
        ),
    ] {
        // Constant raw input isolates automation from intentional quantization and
        // driven waveform slopes. Stateful modulation uses varying input instead.
        let data = if matches!(initial, Effect::Bitcrusher(_) | Effect::Saturation(_)) {
            vec![0.31; 16_000]
        } else {
            sine(137.0, 16_000)
        };
        let baseline = process(vec![data.clone()], initial, data.len(), RATE);
        let (mut m, mut h) = mixer(RATE, 1);
        h.set_effects(BusId::MASTER, &[initial]).unwrap();
        h.play(Audio::new(vec![data], RATE).unwrap(), &Play::default())
            .unwrap();
        h.at(6000)
            .set_effect(BusId::MASTER, 0, target, Fade::linear(0.1))
            .unwrap();
        let mut out = vec![0.0; 16_000];
        m.render(&mut out);
        assert_eq!(out[..=6000], baseline[..=6000], "ramp start {initial:?}");
        let max_step = out[5990..10_900]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_step < 0.02,
            "automation discontinuity {initial:?}: {max_step}"
        );
    }
}

#[test]
fn nonlinear_ramps_stay_inside_prepared_delay_and_frequency_bounds() {
    for (initial, target) in [
        (
            Effect::Chorus(Chorus {
                delay_ms: 70.0,
                depth_ms: 9.0,
                feedback: 0.95,
                ..Default::default()
            }),
            Effect::Chorus(Chorus {
                delay_ms: 40.0,
                depth_ms: 39.0,
                feedback: -0.95,
                ..Default::default()
            }),
        ),
        (
            Effect::Flanger(Flanger {
                delay_ms: 17.0,
                depth_ms: 2.0,
                feedback: 0.95,
                ..Default::default()
            }),
            Effect::Flanger(Flanger {
                delay_ms: 10.0,
                depth_ms: 9.0,
                feedback: -0.95,
                ..Default::default()
            }),
        ),
        (
            Effect::Phaser(Phaser {
                centre_hz: 1000.0,
                depth_octaves: 4.0,
                feedback: 0.95,
                ..Default::default()
            }),
            Effect::Phaser(Phaser {
                centre_hz: 20_000.0,
                depth_octaves: 4.0,
                feedback: -0.95,
                ..Default::default()
            }),
        ),
    ] {
        let (mut m, mut h) = mixer(RATE, 1);
        h.set_effects(BusId::MASTER, &[initial]).unwrap();
        h.play(
            Audio::new(vec![sine(211.0, 20_000)], RATE).unwrap(),
            &Play::default(),
        )
        .unwrap();
        h.at(100)
            .set_effect(
                BusId::MASTER,
                0,
                target,
                Fade::new(0.2, audiobox::FadeCurve::EqualPower),
            )
            .unwrap();
        let mut out = vec![0.0; 20_000];
        m.render(&mut out);
        assert!(
            out.iter().all(|v| v.is_finite() && v.abs() < 10.0),
            "unstable {initial:?}"
        );
    }
}

fn timeline(chunked: bool, paused: bool) -> Vec<f32> {
    let (mut m, mut h) = mixer(RATE, 2);
    h.set_effects(
        BusId::MASTER,
        &[
            Effect::Chorus(Chorus::default()),
            Effect::Flanger(Flanger::default()),
            Effect::Phaser(Phaser::default()),
            Effect::Bitcrusher(Bitcrusher::default()),
            Effect::Saturation(Saturation::default()),
        ],
    )
    .unwrap();
    let data: Vec<f32> = (0..12_000)
        .map(|i| ((i * 127 % 251) as f32 / 251.0 - 0.5) * 0.3)
        .collect();
    h.play(
        Audio::new(
            vec![data.clone(), data.iter().map(|v| -v * 0.5).collect()],
            RATE,
        )
        .unwrap(),
        &Play::default(),
    )
    .unwrap();
    h.at(777)
        .set_effect(
            BusId::MASTER,
            0,
            Effect::Chorus(Chorus {
                rate: ModulationRate::Beats {
                    beats: 2.0,
                    bpm: 123.0,
                },
                ..Default::default()
            }),
            Fade::linear(0.02),
        )
        .unwrap();
    h.at(1001)
        .set_effect(
            BusId::MASTER,
            3,
            Effect::Bitcrusher(Bitcrusher {
                bits: 4.5,
                rate_ratio: 0.3,
                mix: 0.7,
            }),
            Fade::linear(0.03),
        )
        .unwrap();
    if paused {
        h.at(4000).pause_bus(BusId::MASTER, Fade::NONE).unwrap();
        h.at(5000).resume_bus(BusId::MASTER, Fade::NONE).unwrap();
    }
    let mut out = vec![0.0; if paused { 28_000 } else { 26_000 }];
    if chunked {
        let sizes = [1, 17, 255, 2, 511, 37];
        let mut start = 0;
        let mut n = 0;
        while start < out.len() {
            let end = (start + sizes[n % sizes.len()] * 2).min(out.len());
            m.render(&mut out[start..end]);
            start = end;
            n += 1;
        }
    } else {
        m.render(&mut out);
    }
    if paused {
        assert!(out.drain(8000..10_000).all(|v| v == 0.0));
    }
    out
}

#[test]
fn phase_holds_and_sample_bits_are_chunk_independent_and_cross_platform_pinned() {
    let out = timeline(false, false);
    assert_eq!(out, timeline(true, false));
    assert_eq!(out, timeline(true, true));
    let hash = out
        .iter()
        .flat_map(|s| s.to_bits().to_le_bytes())
        .fold(0xcbf29ce484222325u64, |h, byte| {
            (h ^ byte as u64).wrapping_mul(0x100000001b3)
        });
    assert_eq!(hash, 8_455_824_914_445_203_916); // Checked on native targets and WASI.
}

#[test]
fn invalid_colour_settings_are_errors() {
    let (_m, mut h) = mixer(RATE, 1);
    for effect in [
        Effect::Chorus(Chorus {
            delay_ms: 79.0,
            depth_ms: 2.0,
            ..Default::default()
        }),
        Effect::Flanger(Flanger {
            rate: ModulationRate::Hertz(f64::INFINITY),
            ..Default::default()
        }),
        Effect::Chorus(Chorus {
            rate: ModulationRate::Beats {
                beats: 0.0,
                bpm: 120.0,
            },
            ..Default::default()
        }),
        Effect::Phaser(Phaser {
            centre_hz: f32::NAN,
            ..Default::default()
        }),
        Effect::Bitcrusher(Bitcrusher {
            bits: 1.0,
            ..Default::default()
        }),
        Effect::Bitcrusher(Bitcrusher {
            rate_ratio: 0.0,
            ..Default::default()
        }),
        Effect::Saturation(Saturation {
            drive_db: 37.0,
            ..Default::default()
        }),
    ] {
        assert!(h.set_effects(BusId::MASTER, &[effect]).is_err());
    }
}

#[test]
#[cfg(feature = "music")]
fn a_whole_music_track_can_use_a_dedicated_effect_bus() {
    use audiobox::music::{Music, TimeSignature, Timing, Track, Transport};
    let (mut m, mut h) = Mixer::new(MixerConfig {
        channels: 1,
        buses: vec!["track".into()],
        ..Default::default()
    })
    .unwrap();
    let bus = h.bus("track").unwrap();
    h.set_effects(
        bus,
        &[Effect::Saturation(Saturation {
            drive_db: 0.0,
            output_db: 0.0,
            mix: 1.0,
        })],
    )
    .unwrap();
    let music = Music::new(
        &mut h,
        Transport::new(RATE, 120.0, TimeSignature::default(), 0).unwrap(),
        bus,
    )
    .unwrap();
    music
        .transition(
            &mut h,
            Track::new(Audio::new(vec![vec![0.4; 2000]], RATE).unwrap()),
            1.0,
            Timing::NOW,
            Fade::NONE,
        )
        .unwrap();
    h.play(
        Audio::new(vec![vec![0.2; 2000]], RATE).unwrap(),
        &Play::default(),
    )
    .unwrap();
    let mut out = [0.0; 100];
    m.render(&mut out);
    assert!(
        out.iter()
            .all(|&v| (v as f64 - (0.4 - 0.4f64.powi(3) / 3.0 + 0.2)).abs() < 1e-7)
    );
}
