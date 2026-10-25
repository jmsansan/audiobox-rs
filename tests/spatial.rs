//! Spatial response, scheduling, surround processing and deterministic rendering.
#![cfg(feature = "mixer")]
use audiobox::mixer::*;
use audiobox::{Audio, FadeCurve};
use std::sync::Arc;
const RATE: u32 = 48000;
fn config(channels: usize) -> MixerConfig {
    MixerConfig {
        channels,
        ..Default::default()
    }
}
fn point(position: [f32; 3]) -> Emitter {
    Emitter {
        position,
        attenuation: Attenuation {
            rolloff: 0.0,
            ..Default::default()
        },
        ..Default::default()
    }
}
fn constant() -> Arc<Audio> {
    Arc::new(Audio::new(vec![vec![1.0; 48000]], RATE).unwrap())
}
fn place(p: [f32; 3], layout: OutputLayout) -> Placement {
    SpatialConfig::default()
        .place(&Listener::default(), &point(p), layout)
        .unwrap()
}
fn render(m: &mut Mixer, frames: usize, chunk: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames * m.channels()];
    for part in out.chunks_mut(chunk * m.channels()) {
        m.render(part);
    }
    out
}
fn fnv(data: &[f32]) -> u64 {
    data.iter()
        .flat_map(|s| s.to_bits().to_le_bytes())
        .fold(0xcbf29ce484222325, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        })
}

#[test]
fn all_layouts_route_to_named_speakers_with_unit_direction_power() {
    for layout in [
        OutputLayout::Mono,
        OutputLayout::Stereo,
        OutputLayout::Quad,
        OutputLayout::Surround51,
        OutputLayout::Surround71,
    ] {
        assert_eq!(
            OutputLayout::from_channels(layout.channels()).unwrap(),
            layout
        );
        for angle in 0..360 {
            let t = angle as f64 * std::f64::consts::PI / 180.0;
            let p = place([t.sin() as f32, 0.0, -t.cos() as f32], layout);
            assert!((p.gains.iter().map(|g| g * g).sum::<f64>() - 1.0).abs() < 1e-12);
            assert!(p.gains.iter().all(|&g| g >= 0.0 && g.is_finite()));
            if layout.channels() >= 6 {
                assert_eq!(p.gains[3], 0.0);
            }
        }
        assert_eq!(
            place([0.0, 10.0, 0.0], layout).gains,
            place([0.0, 0.0, 0.0], layout).gains
        );
    }
    for (layout, positions) in [
        (
            OutputLayout::Quad,
            vec![
                (0, [-1.0, 0.0, -1.0]),
                (1, [1.0, 0.0, -1.0]),
                (2, [-1.0, 0.0, 1.0]),
                (3, [1.0, 0.0, 1.0]),
            ],
        ),
        (
            OutputLayout::Surround51,
            vec![
                (0, [-0.5, 0.0, -0.8660254]),
                (1, [0.5, 0.0, -0.8660254]),
                (2, [0.0, 0.0, -1.0]),
                (4, [-0.9396926, 0.0, 0.34202015]),
                (5, [0.9396926, 0.0, 0.34202015]),
            ],
        ),
        (
            OutputLayout::Surround71,
            vec![
                (4, [-0.5, 0.0, 0.8660254]),
                (5, [0.5, 0.0, 0.8660254]),
                (6, [-1.0, 0.0, 0.0]),
                (7, [1.0, 0.0, 0.0]),
            ],
        ),
    ] {
        for (channel, p) in positions {
            assert!(place(p, layout).gains[channel] > 0.9999999);
        }
    }
    assert_eq!(OutputLayout::Surround71.channel_mask(), 0x63f);
}

#[test]
fn attenuation_models_match_their_clamped_equations_and_rolloff() {
    for (model, mid, twice, far) in [
        (DistanceModel::Linear, 8.0 / 9.0, 7.0 / 9.0, 0.0),
        (DistanceModel::Inverse, 0.5, 1.0 / 3.0, 0.1),
        (DistanceModel::Exponential, 0.5, 0.25, 0.1),
    ] {
        let a = Attenuation {
            model,
            max_distance: 10.0,
            ..Default::default()
        };
        assert_eq!(a.gain(0.0).unwrap(), 1.0);
        assert!((a.gain(2.0).unwrap() - mid).abs() < 1e-12);
        assert!((Attenuation { rolloff: 2.0, ..a }.gain(2.0).unwrap() - twice).abs() < 1e-12);
        assert!((a.gain(1000.0).unwrap() - far).abs() < 1e-12);
        assert_eq!(Attenuation { rolloff: 0.0, ..a }.gain(1000.0).unwrap(), 1.0);
    }
}

#[test]
fn listener_rotation_and_height_have_defined_bounded_behavior() {
    let rotated = Listener {
        forward: [1.0, 0.0, 0.0],
        up: [0.1, 4.0, 0.0],
        ..Default::default()
    };
    let p = SpatialConfig::default()
        .place(&rotated, &point([0.0, 0.0, 1.0]), OutputLayout::Stereo)
        .unwrap();
    assert!(p.gains[1] > 0.999999);
    let above = Emitter {
        position: [0.0, 3.0, -4.0],
        ..Default::default()
    };
    assert!(
        (SpatialConfig::default()
            .place(&Listener::default(), &above, OutputLayout::Quad)
            .unwrap()
            .distance_gain
            - 0.2)
            .abs()
            < 1e-12
    );
    let extreme = Emitter {
        position: [f32::MAX; 3],
        velocity: [-f32::MAX; 3],
        ..Default::default()
    };
    let p = SpatialConfig::default()
        .place(&Listener::default(), &extreme, OutputLayout::Surround71)
        .unwrap();
    assert!(p.gains.iter().all(|v| v.is_finite()));
    assert!((0.25..=4.0).contains(&p.doppler));
}

fn measured(data: &[f32]) -> f64 {
    let positions: Vec<_> = data
        .windows(2)
        .enumerate()
        .filter_map(|(i, w)| (w[0] < 0.0 && w[1] >= 0.0).then_some(i))
        .collect();
    (positions.len() - 1) as f64 * RATE as f64 / (positions.last().unwrap() - positions[0]) as f64
}
#[test]
fn doppler_uses_radial_velocities_and_shifts_the_rendered_frequency() {
    let settings = SpatialConfig {
        speed_of_sound: 300.0,
        doppler_factor: 1.0,
    };
    let emitter = Emitter {
        velocity: [0.0, 0.0, 30.0],
        ..point([0.0, 0.0, -10.0])
    };
    let p = settings
        .place(&Listener::default(), &emitter, OutputLayout::Mono)
        .unwrap();
    assert!((p.doppler - 10.0 / 9.0).abs() < 1e-12);
    let listener = Listener {
        velocity: [0.0, 0.0, -30.0],
        ..Default::default()
    };
    assert!(
        (settings
            .place(&listener, &emitter, OutputLayout::Mono)
            .unwrap()
            .doppler
            - 11.0 / 9.0)
            .abs()
            < 1e-12
    );
    let common = Listener {
        velocity: emitter.velocity,
        ..Default::default()
    };
    assert_eq!(
        settings
            .place(&common, &emitter, OutputLayout::Mono)
            .unwrap()
            .doppler,
        1.0
    );
    let source = Audio::new(
        vec![
            (0..48000)
                .map(|i| {
                    (std::f64::consts::TAU * i as f64 * 440.0 / RATE as f64).sin() as f32 * 0.4
                })
                .collect(),
        ],
        RATE,
    )
    .unwrap();
    let (mut m, mut h) = Mixer::new(MixerConfig {
        spatial: settings,
        ..config(1)
    })
    .unwrap();
    h.play(
        source,
        &Play {
            emitter: Some(emitter),
            ..Default::default()
        },
    )
    .unwrap();
    let data = render(&mut m, 40000, 113);
    assert!((measured(&data[1000..]) - 440.0 * 10.0 / 9.0).abs() < 0.05);
}

#[test]
fn scheduled_start_uses_the_listener_at_application_and_same_frame_sequence_order() {
    let (mut m, mut h) = Mixer::new(config(2)).unwrap();
    let rotated = Listener {
        forward: [0.0, 0.0, 1.0],
        ..Default::default()
    };
    h.at(100)
        .play(
            constant(),
            &Play {
                emitter: Some(point([1.0, 0.0, -1.0])),
                ..Default::default()
            },
        )
        .unwrap();
    h.at(50).set_listener(rotated, Fade::NONE).unwrap();
    let out = render(&mut m, 120, 37);
    let expected = SpatialConfig::default()
        .place(&rotated, &point([1.0, 0.0, -1.0]), OutputLayout::Stereo)
        .unwrap();
    assert!(out[..200].iter().all(|&v| v == 0.0));
    assert!((out[200] as f64 - expected.gains[0]).abs() < 1e-7);
    assert!(out[200] > out[201]);
    for reversed in [false, true] {
        let (mut m, mut h) = Mixer::new(config(2)).unwrap();
        let id = h
            .play(
                constant(),
                &Play {
                    emitter: Some(point([0.0, 0.0, -1.0])),
                    ..Default::default()
                },
            )
            .unwrap();
        if reversed {
            h.at(10)
                .set_emitter(id, point([-1.0, 0.0, -1.0]), Fade::NONE)
                .unwrap();
            h.at(10).set_listener(rotated, Fade::NONE).unwrap();
        } else {
            h.at(10).set_listener(rotated, Fade::NONE).unwrap();
            h.at(10)
                .set_emitter(id, point([-1.0, 0.0, -1.0]), Fade::NONE)
                .unwrap();
        }
        let out = render(&mut m, 11, 11);
        assert!(out[20] < out[21]);
    }
}

#[test]
fn movement_ramps_keep_unit_power_and_freeze_with_pause_including_listener_updates() {
    let run = |paused: bool| {
        let (mut m, mut h) = Mixer::new(config(8)).unwrap();
        let id = h
            .play(
                constant(),
                &Play {
                    emitter: Some(point([-1.0, 0.0, -1.0])),
                    ..Default::default()
                },
            )
            .unwrap();
        h.at(100)
            .set_emitter(
                id,
                point([1.0, 0.0, 1.0]),
                Fade::linear(200.0 / RATE as f64),
            )
            .unwrap();
        h.at(if paused { 250 } else { 150 })
            .set_listener(
                Listener {
                    forward: [1.0, 0.0, 0.0],
                    ..Default::default()
                },
                Fade::linear(100.0 / RATE as f64),
            )
            .unwrap();
        if paused {
            h.at(150).pause(id, Fade::NONE).unwrap();
            h.at(350).resume(id, Fade::NONE).unwrap();
        }
        render(&mut m, if paused { 600 } else { 400 }, 37)
    };
    let a = run(false);
    let b = run(true);
    assert!(b[150 * 8..350 * 8].iter().all(|&v| v == 0.0));
    let joined: Vec<_> = b[..150 * 8].iter().chain(&b[350 * 8..]).copied().collect();
    assert_eq!(a, joined);
    for frame in a.chunks_exact(8) {
        assert!((frame.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() - 1.0).abs() < 1e-6);
    }
    assert!(
        a.chunks_exact(8)
            .zip(a.chunks_exact(8).skip(1))
            .all(|(a, b)| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.04))
    );
}

#[test]
fn stereo_fold_preserves_individual_and_uncorrelated_power_with_documented_lfe_policy() {
    for layout in [
        OutputLayout::Mono,
        OutputLayout::Quad,
        OutputLayout::Surround51,
        OutputLayout::Surround71,
    ] {
        let ch = layout.channels();
        for c in 0..ch {
            let mut input = vec![0.0; ch];
            input[c] = 1.0;
            let mut out = [0.0; 2];
            layout.downmix_stereo(&input, &mut out).unwrap();
            let power = out.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
            assert!((power - if ch >= 6 && c == 3 { 0.0 } else { 1.0 }).abs() < 1e-6);
        }
        let mut seed = 1u32;
        let mut data = vec![0.0; ch * 20000];
        for frame in data.chunks_exact_mut(ch) {
            for (c, v) in frame.iter_mut().enumerate() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                *v = if ch >= 6 && c == 3 {
                    0.0
                } else {
                    (seed as f64 / u32::MAX as f64 - 0.5) as f32
                };
            }
        }
        let mut out = vec![0.0; 40000];
        layout.downmix_stereo(&data, &mut out).unwrap();
        let energy = |v: &[f32]| v.iter().map(|&x| (x as f64).powi(2)).sum::<f64>();
        assert!((energy(&out) / energy(&data) - 1.0).abs() < 0.02);
    }
}

#[test]
fn surround_effect_chains_and_linked_limiter_process_every_channel_without_crosstalk() {
    use audiobox::mixer::effects::*;
    let effects = [
        Effect::LowPass { cutoff_hz: 4000.0 },
        Effect::HighPass { cutoff_hz: 100.0 },
        Effect::Compressor(Compressor::default()),
        Effect::Delay(Delay::default()),
        Effect::Reverb(Reverb::default()),
        Effect::Chorus(Chorus::default()),
        Effect::Flanger(Flanger::default()),
        Effect::Phaser(Phaser::default()),
        Effect::Bitcrusher(Bitcrusher::default()),
        Effect::Saturation(Saturation::default()),
    ];
    for effect in effects {
        let (mut m, mut h) = Mixer::new(MixerConfig {
            limiter: Some(MasterLimiter {
                lookahead_ms: 2.0,
                ..Default::default()
            }),
            ..config(8)
        })
        .unwrap();
        h.set_effects(BusId::MASTER, &[effect]).unwrap();
        h.play(
            constant(),
            &Play {
                emitter: Some(point([1.0, 0.0, 0.0])),
                gain: 10.0,
                ..Default::default()
            },
        )
        .unwrap();
        let data = render(&mut m, 16000, 137);
        assert!(data.iter().all(|v| v.is_finite() && v.abs() <= 0.891251));
        assert!(
            data.iter().skip(7).step_by(8).any(|&v| v.abs() > 0.00001),
            "{effect:?}"
        );
        for c in 0..7 {
            assert!(
                data.iter().skip(c).step_by(8).all(|&v| v == 0.0),
                "{effect:?}/{c}"
            );
        }
    }
}

fn reference(chunk: usize) -> Vec<f32> {
    let (mut m, mut h) = Mixer::new(config(8)).unwrap();
    let source = Audio::new(
        vec![(0..48000).map(|i| (i % 101) as f32 / 101.0 - 0.5).collect()],
        RATE,
    )
    .unwrap();
    let id = h
        .play(
            source,
            &Play {
                emitter: Some(point([-1.0, 0.5, -2.0])),
                ..Default::default()
            },
        )
        .unwrap();
    h.at(67)
        .set_emitter(
            id,
            Emitter {
                velocity: [0.0, 0.0, 30.0],
                ..point([2.0, 1.0, 2.0])
            },
            Fade::linear(0.004),
        )
        .unwrap();
    h.at(311)
        .set_listener(
            Listener {
                position: [1.0, 0.0, 0.0],
                forward: [1.0, 0.0, 0.0],
                ..Default::default()
            },
            Fade::linear(0.003),
        )
        .unwrap();
    h.at(700)
        .stop(id, Fade::new(0.002, FadeCurve::EqualPower))
        .unwrap();
    render(&mut m, 1000, chunk)
}
#[test]
fn spatial_scheduling_is_chunk_independent_and_cross_platform_pinned() {
    let expected = reference(1000);
    for chunk in [1, 13, 256] {
        assert_eq!(reference(chunk), expected);
    }
    assert_eq!(fnv(&expected), 0xa9e241aa41616199);
}

#[test]
fn invalid_spatial_states_and_ambiguous_placements_are_rejected() {
    for channels in [0, 3, 5, 7, 9] {
        assert!(Mixer::new(config(channels)).is_err());
    }
    for listener in [
        Listener {
            forward: [0.0; 3],
            ..Default::default()
        },
        Listener {
            up: [0.0, 0.0, 1.0],
            ..Default::default()
        },
        Listener {
            position: [f32::NAN; 3],
            ..Default::default()
        },
    ] {
        assert!(listener.validate().is_err());
    }
    for a in [
        Attenuation {
            min_distance: 0.0,
            ..Default::default()
        },
        Attenuation {
            rolloff: f32::NAN,
            ..Default::default()
        },
        Attenuation {
            max_distance: 0.5,
            ..Default::default()
        },
    ] {
        assert!(a.validate().is_err());
    }
    let (_, mut h) = Mixer::new(config(2)).unwrap();
    assert!(
        h.play(
            constant(),
            &Play {
                emitter: Some(point([0.0; 3])),
                position: Some([0.0; 2]),
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        OutputLayout::Quad
            .downmix_stereo(&[0.0; 5], &mut [0.0; 2])
            .is_err()
    );
}

#[test]
fn converting_a_right_panned_mono_voice_does_not_drop_the_first_spatial_sample() {
    let (mut m, mut h) = Mixer::new(config(1)).unwrap();
    let id = h
        .play(
            constant(),
            &Play {
                pan: 1.0,
                ..Default::default()
            },
        )
        .unwrap();
    h.at(10)
        .set_emitter(id, point([0.0, 0.0, -1.0]), Fade::linear(0.1))
        .unwrap();
    assert_eq!(render(&mut m, 40, 7), vec![1.0; 40]);
}

#[test]
fn quietest_instance_uses_updated_spatial_distance() {
    for moved in [false, true] {
        let (mut m, mut h) = Mixer::new(config(2)).unwrap();
        let instances = Some(Instances::new(1).max(2).policy(OverLimit::StopQuietest));
        let mut ids = Vec::new();
        for distance in [2.0, 10.0] {
            ids.push(
                h.play(
                    constant(),
                    &Play {
                        instances,
                        emitter: Some(Emitter {
                            position: [0.0, 0.0, -distance],
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
                .unwrap(),
            );
        }
        if moved {
            h.at(5)
                .set_listener(
                    Listener {
                        position: [0.0, 0.0, -9.0],
                        ..Default::default()
                    },
                    Fade::NONE,
                )
                .unwrap();
        }
        h.at(10)
            .play(
                constant(),
                &Play {
                    instances,
                    emitter: Some(point([0.0, 0.0, -1.0])),
                    ..Default::default()
                },
            )
            .unwrap();
        render(&mut m, 20, 13);
        let events: Vec<_> = std::iter::from_fn(|| h.poll_event()).collect();
        assert!(events.contains(&MixerEvent::Stolen(ids[if moved { 0 } else { 1 }])));
    }
}

#[test]
fn explicit_wave_masks_preserve_channel_order_and_default_encoding() {
    use audiobox::{AudioFormat, EncodeOptions, SampleFormat, decode, encode, encode_with_options};
    for layout in [
        OutputLayout::Quad,
        OutputLayout::Surround51,
        OutputLayout::Surround71,
    ] {
        let audio = Audio::new(
            (0..layout.channels())
                .map(|c| vec![c as f32 / 16.0; 13])
                .collect(),
            RATE,
        )
        .unwrap();
        let options = EncodeOptions {
            wav_channel_mask: Some(layout.channel_mask()),
            sample_format: SampleFormat::F32,
            ..Default::default()
        };
        let bytes = encode_with_options(&audio, AudioFormat::Wav, &options).unwrap();
        assert_eq!(
            u16::from_le_bytes(bytes[20..22].try_into().unwrap()),
            0xfffe
        );
        assert_eq!(
            u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
            layout.channel_mask()
        );
        let decoded = decode(&bytes).unwrap();
        for c in 0..layout.channels() {
            assert_eq!(
                decoded.channel_data(c).unwrap(),
                audio.channel_data(c).unwrap()
            );
        }
        assert!(encode_with_options(&audio, AudioFormat::Flac, &options).is_err());
        assert!(audiobox::mp3::encode_mp3(&audio, &options).is_err());
        assert!(
            encode_with_options(
                &audio,
                AudioFormat::Wav,
                &EncodeOptions {
                    wav_channel_mask: Some(3),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert_eq!(
            u16::from_le_bytes(
                encode(&audio, AudioFormat::Wav).unwrap()[20..22]
                    .try_into()
                    .unwrap()
            ),
            1
        );
    }
}

/// Held and moving emitters, a pitch ramp during Doppler, gain ramps and a pause:
/// the cached per-segment work of 0.4.1 must render exactly what 0.4.0 rendered.
fn held_and_moving(chunk: usize) -> Vec<f32> {
    let (mut m, mut h) = Mixer::new(config(6)).unwrap();
    let left = (0..24000).map(|i| (i % 97) as f32 / 97.0 - 0.5).collect();
    let right = (0..24000).map(|i| (i % 61) as f32 / 61.0 - 0.5).collect();
    let stereo = Audio::new(vec![left, right], RATE).unwrap();
    let held = h
        .play(
            stereo,
            &Play {
                emitter: Some(point([-2.0, 0.0, -3.0])),
                ..Default::default()
            },
        )
        .unwrap();
    let moving = h
        .play(
            constant(),
            &Play {
                rate: 1.25,
                emitter: Some(Emitter {
                    velocity: [25.0, 0.0, 0.0],
                    ..point([-5.0, 0.0, -1.0])
                }),
                ..Default::default()
            },
        )
        .unwrap();
    h.at(200)
        .set_emitter(
            moving,
            Emitter {
                velocity: [25.0, 0.0, 0.0],
                ..point([5.0, 0.0, -1.0])
            },
            Fade::linear(0.02),
        )
        .unwrap();
    h.at(500)
        .set_pitch(moving, 3.0, Fade::linear(0.01))
        .unwrap();
    h.at(900).set_gain(held, 0.5, Fade::linear(0.003)).unwrap();
    h.at(1100)
        .set_emitter(moving, point([5.0, 0.0, -1.0]), Fade::linear(0.005))
        .unwrap();
    h.at(1300).pause(held, Fade::linear(0.002)).unwrap();
    h.at(1700).resume(held, Fade::linear(0.002)).unwrap();
    render(&mut m, 2400, chunk)
}
#[test]
fn held_emitters_and_doppler_with_pitch_ramps_render_as_before() {
    let expected = held_and_moving(2400);
    for chunk in [1, 7, 64, 256] {
        assert_eq!(held_and_moving(chunk), expected);
    }
    assert_eq!(fnv(&expected), 0x3e9e630ad3e1ba9e);
}
