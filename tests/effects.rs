//! Measured DSP and routing checks for prepared bus effects.
#![cfg(feature = "mixer")]
use audiobox::mixer::effects::*;
use audiobox::mixer::*;
use audiobox::{Audio, ErrorCode};
use std::sync::Arc;

const RATE: u32 = 48_000;
fn mono(rate: u32) -> (Mixer, MixerHandle) {
    Mixer::new(MixerConfig {
        sample_rate: rate,
        channels: 1,
        ..Default::default()
    })
    .unwrap()
}
fn play(handle: &mut MixerHandle, data: Vec<f32>, bus: BusId) {
    let audio = Audio::new(vec![data], handle.sample_rate()).unwrap();
    handle
        .play(
            audio,
            &Play {
                bus,
                ..Default::default()
            },
        )
        .unwrap();
}
fn process(data: Vec<f32>, effects: &[Effect], frames: usize, rate: u32) -> Vec<f32> {
    let (mut m, mut h) = mono(rate);
    h.set_effects(BusId::MASTER, effects).unwrap();
    play(&mut h, data, BusId::MASTER);
    let mut out = vec![0.0; frames];
    m.render(&mut out);
    out
}
fn impulse() -> Vec<f32> {
    vec![1.0]
}
fn rms(data: &[f32]) -> f64 {
    (data.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / data.len() as f64).sqrt()
}
fn compressor() -> Compressor {
    Compressor {
        attack_ms: 0.0,
        release_ms: 0.0,
        ..Default::default()
    }
}

#[test]
fn filters_match_their_bilinear_response() {
    for high in [false, true] {
        for hz in [100.0f64, 1000.0, 10_000.0] {
            let data: Vec<f32> = (0..RATE)
                .map(|i| (std::f64::consts::TAU * hz * i as f64 / RATE as f64).sin() as f32)
                .collect();
            let effect = if high {
                Effect::HighPass { cutoff_hz: 1000.0 }
            } else {
                Effect::LowPass { cutoff_hz: 1000.0 }
            };
            let input = rms(&data[RATE as usize / 2..]);
            let out = process(data, &[effect], RATE as usize, RATE);
            let ratio = (std::f64::consts::PI * hz / RATE as f64).tan()
                / (std::f64::consts::PI * 1000.0 / RATE as f64).tan();
            let expected = if high { ratio } else { 1.0 } / (1.0 + ratio * ratio).sqrt();
            let measured = rms(&out[RATE as usize / 2..]) / input;
            assert!(
                (measured - expected).abs() < 1e-5,
                "high={high}, hz={hz}: {measured} vs {expected}"
            );
        }
    }
}

#[test]
fn compressor_static_curve_depth_and_stereo_link() {
    for input_db in [-36.0f64, -18.0, -12.0, 0.0] {
        let c = compressor();
        let input = 10f64.powf(input_db / 20.0) as f32;
        let out = process(vec![input; 8], &[Effect::Compressor(c)], 8, RATE);
        let expected_db = if input_db > -18.0 {
            -18.0 + (input_db + 18.0) / 4.0
        } else {
            input_db
        };
        assert!((out[0] as f64 - 10f64.powf(expected_db / 20.0)).abs() < 1e-7);
        let half = process(
            vec![input; 8],
            &[Effect::Compressor(Compressor { depth: 0.5, ..c })],
            8,
            RATE,
        );
        assert!((half[0] - (input + out[0]) * 0.5).abs() < 1e-7);
    }
    let (mut m, mut h) = Mixer::new(MixerConfig::default()).unwrap();
    h.set_effects(BusId::MASTER, &[Effect::Compressor(compressor())])
        .unwrap();
    h.play(
        Audio::new(vec![vec![1.0; 20], vec![0.1; 20]], RATE).unwrap(),
        &Play::default(),
    )
    .unwrap();
    let mut out = [0.0; 40];
    m.render(&mut out);
    for pair in out.chunks_exact(2) {
        assert!((pair[1] / pair[0] - 0.1).abs() < 1e-7);
    }
}

#[test]
fn sidechain_is_current_frame_post_fader_and_creation_order_independent() {
    // Target is deliberately declared before the key; its key must still run first.
    let (mut m, mut h) = Mixer::new(MixerConfig {
        channels: 1,
        buses: vec!["music".into(), "dialogue".into()],
        ..Default::default()
    })
    .unwrap();
    let target = h.bus("music").unwrap();
    let key = h.bus("dialogue").unwrap();
    let c = Compressor {
        threshold_db: -20.0,
        ratio: 100.0,
        attack_ms: 10.0,
        release_ms: 20.0,
        sidechain: Some(key),
        ..compressor()
    };
    h.set_effects(target, &[Effect::Compressor(c)]).unwrap();
    // A key compressor + fader turns 1 into 0.2 before it reaches the detector.
    h.set_effects(
        key,
        &[Effect::Compressor(Compressor {
            threshold_db: -6.0206,
            ratio: 100.0,
            ..compressor()
        })],
    )
    .unwrap();
    h.set_bus_gain(key, 0.4, Fade::NONE).unwrap();
    play(&mut h, vec![1.0; 4800], target);
    play(&mut h, vec![1.0; 960], key);
    let mut out = vec![0.0; 3000];
    m.render(&mut out);
    // Static key compressor (using its actual f32 settings), followed by key gain.
    let key_level = 10f64.powf((-6.0206f32 as f64 * 0.99) / 20.0) * 0.4f32 as f64;
    let target_gain = (0.1 / key_level).powf(0.99);
    let attack = (-1.0f64 / 480.0).exp();
    for n in [0usize, 479, 959] {
        let gain = target_gain + (1.0 - target_gain) * attack.powi(n as i32 + 1);
        assert!((out[n] as f64 - key_level - gain).abs() < 2e-6, "frame {n}");
    }
    let last = target_gain + (1.0 - target_gain) * attack.powi(960);
    for n in [960usize, 1919, 2999] {
        let expected = 1.0 + (last - 1.0) * (-((n - 959) as f64) / 960.0).exp();
        assert!((out[n] as f64 - expected).abs() < 2e-6, "release {n}");
    }
}

#[test]
fn delay_time_feedback_fractional_interpolation_and_beats() {
    for time in [
        EffectTime::Seconds(0.01),
        EffectTime::Beats {
            beats: 0.02,
            bpm: 120.0,
        },
    ] {
        let d = Delay {
            time,
            max_seconds: 0.1,
            feedback: 0.5,
            mix: 1.0,
        };
        let out = process(impulse(), &[Effect::Delay(d)], 2401, RATE);
        for (i, &sample) in out.iter().enumerate() {
            let expected = if i > 0 && i % 480 == 0 {
                0.5f32.powi((i / 480 - 1) as i32)
            } else {
                0.0
            };
            assert_eq!(sample, expected, "delay frame {i}");
        }
    }
    let out = process(
        impulse(),
        &[Effect::Delay(Delay {
            time: EffectTime::Seconds(1.5 / RATE as f64),
            feedback: 0.0,
            mix: 1.0,
            ..Default::default()
        })],
        5,
        RATE,
    );
    assert_eq!(out, [0.0, 0.5, 0.5, 0.0, 0.0]);
}

#[test]
fn reverb_decay_is_within_ten_percent_of_nominal_t60() {
    // Schroeder backward integrated impulse energy; a linear fit from -5 to -35 dB
    // excludes the initial sparse onset and extrapolates the -60 dB decay time.
    for rate in [44_100, 48_000] {
        for decay in [0.5f32, 1.5] {
            let out = process(
                impulse(),
                &[Effect::Reverb(Reverb {
                    decay_seconds: decay,
                    damping: 0.0,
                    mix: 1.0,
                })],
                (rate as f32 * decay * 2.0) as usize,
                rate,
            );
            let mut energy = vec![0.0f64; out.len()];
            let mut sum = 0.0;
            for i in (0..out.len()).rev() {
                sum += (out[i] as f64).powi(2);
                energy[i] = sum;
            }
            let crossing = |db| {
                energy
                    .iter()
                    .position(|e| 10.0 * (e / energy[0]).log10() <= db)
                    .unwrap() as f64
                    / rate as f64
            };
            let measured = 2.0 * (crossing(-35.0) - crossing(-5.0));
            assert!(
                (measured / decay as f64 - 1.0).abs() < 0.1,
                "rate {rate}, T60 {decay}: {measured}"
            );
            assert!(out.iter().all(|x| x.is_finite()));
        }
    }
}

#[test]
fn ramps_are_continuous_and_updates_preserve_delay_history() {
    let (mut m, mut h) = mono(RATE);
    let dry = Delay {
        time: EffectTime::Seconds(0.01),
        feedback: 0.5,
        mix: 0.0,
        ..Default::default()
    };
    h.set_effects(BusId::MASTER, &[Effect::Delay(dry)]).unwrap();
    // Silence after impulse tests that changing mix does not clear retained echoes.
    play(&mut h, impulse(), BusId::MASTER);
    h.at(240)
        .set_effect(
            BusId::MASTER,
            0,
            Effect::Delay(Delay { mix: 1.0, ..dry }),
            Fade::linear(0.005),
        )
        .unwrap();
    let mut out = vec![0.0; 1441];
    m.render(&mut out);
    assert_eq!(out[480], 1.0);
    assert_eq!(out[960], 0.5);

    let (mut m, mut h) = mono(RATE);
    h.set_effects(BusId::MASTER, &[Effect::Compressor(compressor())])
        .unwrap();
    play(&mut h, vec![1.0; 3000], BusId::MASTER);
    h.at(1000)
        .set_effect(
            BusId::MASTER,
            0,
            Effect::Compressor(Compressor {
                depth: 0.0,
                ..compressor()
            }),
            Fade::linear(0.02),
        )
        .unwrap();
    let mut out = vec![0.0; 3000];
    m.render(&mut out);
    let max_step = out[990..2000]
        .windows(2)
        .map(|p| (p[1] - p[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(max_step < 0.001, "parameter ramp step: {max_step}");
    assert_eq!(out[1000], out[999]);
    assert_eq!(out[1960], 1.0);
}

#[test]
fn topology_edits_are_atomic_and_use_application_order() {
    let (mut m, mut h) = Mixer::new(MixerConfig {
        channels: 1,
        buses: vec!["a".into(), "b".into()],
        ..Default::default()
    })
    .unwrap();
    let a = h.bus("a").unwrap();
    let b = h.bus("b").unwrap();
    let keyed = |key| {
        [Effect::Compressor(Compressor {
            sidechain: Some(key),
            ..compressor()
        })]
    };
    // Submission order is the opposite of application order. Only b->a survives.
    h.at(100).set_effects(b, &keyed(a)).unwrap();
    h.at(50).set_effects(a, &keyed(b)).unwrap();
    m.render(&mut [0.0; 150]);
    let events: Vec<_> = std::iter::from_fn(|| h.poll_event()).collect();
    assert_eq!(
        events,
        [
            MixerEvent::EffectsApplied(a, 50),
            MixerEvent::EffectsRejected(b, ErrorCode::InvalidArgument)
        ]
    );
    // A parameter update can also create a cycle and must be refused unchanged.
    h.set_effects(b, &[Effect::Compressor(compressor())])
        .unwrap();
    m.render(&mut []);
    assert_eq!(h.poll_event(), Some(MixerEvent::EffectsApplied(b, 150)));
    h.set_effect(b, 0, keyed(a)[0], Fade::NONE).unwrap();
    m.render(&mut []);
    assert_eq!(
        h.poll_event(),
        Some(MixerEvent::EffectsRejected(b, ErrorCode::InvalidArgument))
    );
    h.set_effects(a, &[]).unwrap();
    m.render(&mut []);
    h.poll_event();
    h.set_effect(b, 0, keyed(a)[0], Fade::NONE).unwrap();
    m.render(&mut []);
    assert_eq!(h.poll_event(), None);
}

#[test]
fn cutoff_and_delay_time_ramps_have_no_abrupt_sample_jump() {
    let signal: Vec<f32> = (0..12_000)
        .map(|i| (std::f64::consts::TAU * 137.0 * i as f64 / RATE as f64).sin() as f32 * 0.2)
        .collect();
    let delay = Delay {
        time: EffectTime::Seconds(0.01),
        max_seconds: 0.1,
        feedback: 0.0,
        mix: 1.0,
    };
    for (initial, target) in [
        (
            Effect::LowPass { cutoff_hz: 80.0 },
            Effect::LowPass {
                cutoff_hz: 10_000.0,
            },
        ),
        (
            Effect::HighPass { cutoff_hz: 80.0 },
            Effect::HighPass {
                cutoff_hz: 10_000.0,
            },
        ),
        (
            Effect::Delay(delay),
            Effect::Delay(Delay {
                time: EffectTime::Seconds(0.023),
                ..delay
            }),
        ),
    ] {
        let (mut mixer, mut handle) = mono(RATE);
        handle.set_effects(BusId::MASTER, &[initial]).unwrap();
        play(&mut handle, signal.clone(), BusId::MASTER);
        handle
            .at(6000)
            .set_effect(BusId::MASTER, 0, target, Fade::linear(0.1))
            .unwrap();
        let mut out = vec![0.0; signal.len()];
        mixer.render(&mut out);
        let baseline = process(signal.clone(), &[initial], signal.len(), RATE);
        assert_eq!(
            out[..=6000],
            baseline[..=6000],
            "the ramp starts at the previous setting"
        );
        // The input's maximum per-sample slope is 0.00359. This allows its normal
        // waveform motion, but rejects a sudden cutoff/read-head jump at this phase.
        let largest = out[5990..10_900]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(largest < 0.006, "{initial:?} ramp discontinuity: {largest}");
    }
}

#[test]
fn master_processes_the_sum_and_bus_pause_freezes_tails_and_ramps() {
    let (mut m, mut h) = Mixer::new(MixerConfig {
        channels: 1,
        buses: vec!["wet".into()],
        ..Default::default()
    })
    .unwrap();
    let bus = h.bus("wet").unwrap();
    h.set_effects(BusId::MASTER, &[Effect::Compressor(compressor())])
        .unwrap();
    play(&mut h, vec![0.5; 10], bus);
    play(&mut h, vec![0.5; 10], BusId::MASTER);
    h.set_bus_gain(BusId::MASTER, 0.5, Fade::NONE).unwrap();
    let mut out = [0.0; 10];
    m.render(&mut out);
    let expected = 0.5 * 10f64.powf(-13.5 / 20.0);
    assert!((out[0] as f64 - expected).abs() < 1e-7);

    // Compare a paused render to the exact same timeline with that interval removed.
    let make = |pause| {
        let (mut m, mut h) = mono(RATE);
        let reverb = Reverb {
            damping: 0.0,
            mix: 1.0,
            ..Default::default()
        };
        h.set_effects(BusId::MASTER, &[Effect::Reverb(reverb)])
            .unwrap();
        play(&mut h, impulse(), BusId::MASTER);
        h.at(1000)
            .set_effect(
                BusId::MASTER,
                0,
                Effect::Reverb(Reverb {
                    decay_seconds: 0.5,
                    ..reverb
                }),
                Fade::linear(0.1),
            )
            .unwrap();
        if pause {
            h.at(2000).pause_bus(BusId::MASTER, Fade::NONE).unwrap();
            h.at(3000).resume_bus(BusId::MASTER, Fade::NONE).unwrap();
        }
        let mut out = vec![0.0; if pause { 7000 } else { 6000 }];
        m.render(&mut out);
        if pause {
            assert!(out.drain(2000..3000).all(|v| v == 0.0));
        }
        out
    };
    assert_eq!(make(true), make(false));
}

fn timeline(chunked: bool) -> Vec<f32> {
    let (mut m, mut h) = Mixer::new(MixerConfig {
        channels: 2,
        buses: vec!["music".into(), "key".into()],
        ..Default::default()
    })
    .unwrap();
    let music = h.bus("music").unwrap();
    let key = h.bus("key").unwrap();
    let reverb = Reverb::default();
    h.set_effects(
        music,
        &[
            Effect::LowPass { cutoff_hz: 2300.0 },
            Effect::Compressor(Compressor::ducking(key)),
            Effect::Delay(Delay::default()),
            Effect::Reverb(reverb),
        ],
    )
    .unwrap();
    let data: Vec<f32> = (0..8000)
        .map(|n| (((n * 127 % 251) as f32 / 251.0) - 0.5) * 0.2)
        .collect();
    let audio = Arc::new(
        Audio::new(
            vec![data.clone(), data.iter().map(|v| -v * 0.7).collect()],
            RATE,
        )
        .unwrap(),
    );
    h.play(
        audio,
        &Play {
            bus: music,
            ..Default::default()
        },
    )
    .unwrap();
    play(&mut h, vec![0.25; 3000], key);
    h.at(721)
        .set_effect(
            music,
            0,
            Effect::LowPass { cutoff_hz: 100.0 },
            Fade::linear(0.04),
        )
        .unwrap();
    h.at(1499)
        .set_effect(
            music,
            3,
            Effect::Reverb(Reverb {
                decay_seconds: 0.3,
                ..reverb
            }),
            Fade::linear(0.02),
        )
        .unwrap();
    h.at(4001)
        .set_effects(key, &[Effect::HighPass { cutoff_hz: 600.0 }])
        .unwrap();
    h.at(6003)
        .set_bus_gain(music, 0.7, Fade::linear(0.01))
        .unwrap();
    let mut out = vec![0.0; 24_000];
    if chunked {
        let sizes = [1, 17, 255, 2, 511, 37];
        let mut offset = 0;
        let mut index = 0;
        while offset < out.len() {
            let end = (offset + sizes[index % sizes.len()] * 2).min(out.len());
            m.render(&mut out[offset..end]);
            offset = end;
            index += 1;
        }
    } else {
        m.render(&mut out);
    }
    out
}
#[test]
fn scheduled_effects_are_partition_invariant_and_cross_platform_deterministic() {
    let out = timeline(false);
    assert_eq!(out, timeline(true));
    let hash = out
        .iter()
        .flat_map(|s| s.to_bits().to_le_bytes())
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ byte as u64).wrapping_mul(0x100000001b3)
        });
    // This same checksum executes on native targets and WASI in CI.
    assert_eq!(hash, 5_157_487_174_562_760_212);
}

#[test]
fn invalid_parameters_types_capacities_and_keys_are_rejected() {
    let (mut m, mut h) = mono(RATE);
    for effect in [
        Effect::LowPass {
            cutoff_hz: f32::NAN,
        },
        Effect::HighPass {
            cutoff_hz: 24_000.0,
        },
        Effect::Compressor(Compressor {
            ratio: 0.0,
            ..Default::default()
        }),
        Effect::Compressor(Compressor::ducking(BusId::MASTER)),
        Effect::Delay(Delay {
            time: EffectTime::Seconds(0.0),
            ..Default::default()
        }),
        Effect::Delay(Delay {
            time: EffectTime::Beats {
                beats: 1.0,
                bpm: f64::NAN,
            },
            ..Default::default()
        }),
        Effect::Reverb(Reverb {
            mix: f32::INFINITY,
            ..Default::default()
        }),
    ] {
        assert!(h.set_effects(BusId::MASTER, &[effect]).is_err());
    }
    let d = Delay {
        max_seconds: 0.1,
        time: EffectTime::Seconds(0.01),
        feedback: 0.99,
        ..Default::default()
    };
    h.set_effects(BusId::MASTER, &[Effect::Delay(d)]).unwrap();
    m.render(&mut []);
    h.poll_event();
    h.set_effect(
        BusId::MASTER,
        0,
        Effect::LowPass { cutoff_hz: 1000.0 },
        Fade::NONE,
    )
    .unwrap();
    m.render(&mut []);
    assert_eq!(
        h.poll_event(),
        Some(MixerEvent::EffectsRejected(
            BusId::MASTER,
            ErrorCode::InvalidArgument
        ))
    );
    h.set_effect(
        BusId::MASTER,
        0,
        Effect::Delay(Delay {
            max_seconds: 1.0,
            ..d
        }),
        Fade::NONE,
    )
    .unwrap();
    m.render(&mut []);
    assert_eq!(
        h.poll_event(),
        Some(MixerEvent::EffectsRejected(
            BusId::MASTER,
            ErrorCode::InvalidArgument
        ))
    );
    assert!(
        h.set_effects(BusId::MASTER, &vec![Effect::Delay(d); MAX_EFFECTS + 1])
            .is_err()
    );
}
