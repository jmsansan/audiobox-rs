#![cfg(feature = "music")]
use audiobox::mixer::{BusId, Fade, Loop, Mixer, MixerConfig, MixerEvent};
use audiobox::music::{Layer, Music, Quantize, TimeSignature, Timing, Track, Transport};
use audiobox::{Audio, ErrorCode, FadeCurve};
use std::sync::Arc;

const RATE: u32 = 1000;

fn setup(max_voices: usize, queue: usize) -> (Mixer, audiobox::mixer::MixerHandle, Music) {
    let (mixer, mut handle) = Mixer::new(MixerConfig {
        sample_rate: RATE,
        channels: 1,
        max_voices,
        queue,
        ..Default::default()
    })
    .unwrap();
    let transport = Transport::new(RATE, 120.0, TimeSignature::default(), 0).unwrap();
    let music = Music::new(&mut handle, transport, BusId::MASTER).unwrap();
    (mixer, handle, music)
}

fn sound(value: f32, frames: usize) -> Arc<Audio> {
    Arc::new(Audio::new(vec![vec![value; frames]], RATE).unwrap())
}

fn at(frame: u64) -> Timing {
    Timing {
        not_before: frame,
        quantize: Quantize::Immediate,
    }
}

fn render(mixer: &mut Mixer, frames: usize, block: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames];
    for chunk in out.chunks_mut(block) {
        mixer.render(chunk);
    }
    out
}

fn events(handle: &mut audiobox::mixer::MixerHandle) -> Vec<MixerEvent> {
    std::iter::from_fn(|| handle.poll_event()).collect()
}

#[test]
fn a_mid_bar_transition_starts_on_the_next_bars_first_frame() {
    let (mut mixer, mut handle, music) = setup(8, 64);
    music
        .transition(
            &mut handle,
            Track::new(sound(0.25, 8000)),
            0.0,
            Timing::NOW,
            Fade::NONE,
        )
        .unwrap();
    assert_eq!(render(&mut mixer, 750, 101), vec![0.25; 750]);
    let id = music
        .transition(
            &mut handle,
            Track::new(sound(0.75, 8000)),
            0.0,
            Timing::BAR,
            Fade::NONE,
        )
        .unwrap();
    let out = render(&mut mixer, 1500, 107);
    assert_eq!(&out[..1250], vec![0.25; 1250]);
    assert_eq!(&out[1250..], vec![0.75; 250]);
    assert!(events(&mut handle).contains(&MixerEvent::MusicApplied(id, 2000)));
}

#[test]
fn each_crossfade_curve_has_the_expected_incoming_and_outgoing_gains() {
    for curve in [
        FadeCurve::Linear,
        FadeCurve::EqualPower,
        FadeCurve::Exponential,
    ] {
        // Opposite stereo channels expose each gain independently.
        let (mut mixer, mut handle) = Mixer::new(MixerConfig {
            sample_rate: RATE,
            channels: 2,
            ..Default::default()
        })
        .unwrap();
        let transport = Transport::new(RATE, 120.0, TimeSignature::default(), 0).unwrap();
        let music = Music::new(&mut handle, transport, BusId::MASTER).unwrap();
        let old = Arc::new(Audio::new(vec![vec![1.0; 2000], vec![0.0; 2000]], RATE).unwrap());
        let new = Arc::new(Audio::new(vec![vec![0.0; 2000], vec![1.0; 2000]], RATE).unwrap());
        music
            .transition(&mut handle, Track::new(old), 0.0, Timing::NOW, Fade::NONE)
            .unwrap();
        music
            .transition(
                &mut handle,
                Track::new(new),
                0.0,
                at(100),
                Fade::new(0.1, curve),
            )
            .unwrap();
        let mut out = vec![0.0; 2 * 250];
        mixer.render(&mut out);
        for i in 0..100 {
            let t = i as f64 / 100.0;
            let (up, down) = match curve {
                FadeCurve::Linear => (t, 1.0 - t),
                FadeCurve::EqualPower => (
                    (t * std::f64::consts::FRAC_PI_2).sin(),
                    (t * std::f64::consts::FRAC_PI_2).cos(),
                ),
                FadeCurve::Exponential => (t * t, (1.0 - t) * (1.0 - t)),
            };
            assert!(
                (out[2 * (100 + i)] as f64 - down).abs() < 1e-6,
                "{curve:?} outgoing {i}"
            );
            assert!(
                (out[2 * (100 + i) + 1] as f64 - up).abs() < 1e-6,
                "{curve:?} incoming {i}"
            );
        }
        assert_eq!(&out[400..], &[0.0, 1.0].repeat(50));
        assert_eq!(mixer.active_voices(), 1);
    }
}

#[test]
fn a_cue_starts_on_the_beat_and_survives_a_track_transition() {
    let (mut mixer, mut handle, music) = setup(8, 64);
    music
        .transition(
            &mut handle,
            Track::new(sound(0.1, 4000)),
            0.0,
            Timing::NOW,
            Fade::NONE,
        )
        .unwrap();
    render(&mut mixer, 123, 123);
    let cue = music
        .cue(&mut handle, sound(0.5, 100), 1.0, Timing::BEAT)
        .unwrap();
    music
        .transition(
            &mut handle,
            Track::new(sound(0.2, 4000)),
            0.0,
            at(550),
            Fade::NONE,
        )
        .unwrap();
    let out = render(&mut mixer, 600, 73);
    assert!(out[..377].iter().all(|&s| s == 0.1));
    assert!(out[377..427].iter().all(|&s| (s - 0.6).abs() < 1e-6));
    assert!(out[427..477].iter().all(|&s| (s - 0.7).abs() < 1e-6));
    assert!(out[477..].iter().all(|&s| s == 0.2));
    assert!(events(&mut handle).contains(&MixerEvent::MusicApplied(cue, 500)));
}

#[test]
fn silent_layers_stay_sample_aligned_through_intensity_fades_and_loops() {
    let (mut mixer, mut handle, music) = setup(8, 64);
    let samples: Vec<f32> = (0..128).map(|i| (i as f32 - 64.0) / 256.0).collect();
    let source = Arc::new(Audio::new(vec![samples.clone()], RATE).unwrap());
    let mut track = Track::new(source.clone());
    track.looping = Loop::Whole;
    track.layers.push(Layer {
        intensity: [0.25, 0.75],
        ..Layer::new(source)
    });
    music
        .transition(&mut handle, track, 0.0, Timing::NOW, Fade::NONE)
        .unwrap();
    music
        .set_intensity(&mut handle, 0.75, at(350), Fade::linear(0.1))
        .unwrap();
    music
        .set_intensity(&mut handle, 0.0, at(650), Fade::linear(0.1))
        .unwrap();
    let out = render(&mut mixer, 1000, 37);
    for (i, &s) in out.iter().enumerate() {
        let extra = if i < 350 {
            0.0
        } else if i < 450 {
            (i - 350) as f32 / 100.0
        } else if i < 650 {
            1.0
        } else if i < 750 {
            1.0 - (i - 650) as f32 / 100.0
        } else {
            0.0
        };
        assert!(
            (s - samples[i % 128] * (1.0 + extra)).abs() < 1e-6,
            "frame {i}: {s}"
        );
    }
    assert_eq!(mixer.active_voices(), 2);
}

#[test]
fn fractional_tempos_and_six_eight_use_absolute_rounded_boundaries_without_drift() {
    let grid = Transport::new(
        44_100,
        123.4567,
        TimeSignature {
            numerator: 6,
            denominator: 8,
        },
        99,
    )
    .unwrap();
    assert_eq!(grid.bpm(), 123.457);
    assert_eq!(grid.sample_rate(), 44_100);
    assert_eq!(grid.origin(), 99);
    assert_eq!(
        grid.signature(),
        TimeSignature {
            numerator: 6,
            denominator: 8
        }
    );
    let numerator = 44_100u128 * 240_000;
    let denominator = 123_457u128 * 8;
    for n in [1u128, 2, 3, 100, 1_000_000] {
        let expected = 99 + ((n * numerator + denominator / 2) / denominator) as u64;
        assert_eq!(
            grid.boundary_after(expected - 1, Quantize::Beat).unwrap(),
            expected
        );
        assert!(grid.boundary_after(expected, Quantize::Beat).unwrap() > expected);
    }
    let bar = 99 + ((6 * numerator + denominator / 2) / denominator) as u64;
    assert_eq!(grid.boundary_after(100, Quantize::Bar).unwrap(), bar);
    assert_eq!(grid.boundary_after(0, Quantize::Bar).unwrap(), 99);
    assert_eq!(grid.boundary_after(0, Quantize::Immediate).unwrap(), 0);
    assert_eq!(
        grid.boundary_after(u64::MAX, Quantize::Beat)
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
}

#[test]
fn quantization_happens_when_audio_receives_the_request() {
    let (mut mixer, mut handle, music) = setup(8, 1);
    // Occupy pending with a future command, delaying the new music request.
    handle
        .at(700)
        .set_bus_gain(BusId::MASTER, 1.0, Fade::NONE)
        .unwrap();
    render(&mut mixer, 100, 100);
    let id = music
        .cue(&mut handle, sound(0.5, 10), 1.0, Timing::BEAT)
        .unwrap();
    assert_eq!(render(&mut mixer, 650, 650), vec![0.0; 650]);
    // Receipt is at 750, not submission frame 100. The next beat is frame 1000.
    let out = render(&mut mixer, 300, 11);
    assert_eq!(&out[..250], vec![0.0; 250]);
    assert_eq!(&out[250..260], vec![0.5; 10]);
    assert!(events(&mut handle).contains(&MixerEvent::MusicApplied(id, 1000)));
}

#[test]
fn insufficient_capacity_rejects_all_layers_without_changing_the_previous_track() {
    let (mut mixer, mut handle, music) = setup(2, 64);
    music
        .transition(
            &mut handle,
            Track::new(sound(0.25, 4000)),
            0.0,
            Timing::NOW,
            Fade::NONE,
        )
        .unwrap();
    render(&mut mixer, 10, 10);
    let track = Track {
        layers: vec![Layer::new(sound(0.5, 4000)), Layer::new(sound(0.5, 4000))],
        looping: Loop::Off,
    };
    let rejected = music
        .transition(&mut handle, track, 0.0, Timing::NOW, Fade::linear(0.1))
        .unwrap();
    assert_eq!(render(&mut mixer, 300, 51), vec![0.25; 300]);
    assert_eq!(mixer.active_voices(), 1);
    assert!(events(&mut handle).contains(&MixerEvent::MusicRejected(
        rejected,
        ErrorCode::LimitExceeded
    )));
    // An immediate swap has no overlap and can use both slots.
    let track = Track {
        layers: vec![Layer::new(sound(0.25, 4000)), Layer::new(sound(0.25, 4000))],
        looping: Loop::Off,
    };
    music
        .transition(&mut handle, track, 0.0, Timing::NOW, Fade::NONE)
        .unwrap();
    assert_eq!(render(&mut mixer, 50, 50), vec![0.5; 50]);
    assert_eq!(mixer.active_voices(), 2);
}

#[test]
fn full_queue_and_invalid_later_layers_cannot_partially_change_music() {
    let (mut mixer, mut handle, music) = setup(4, 1);
    music
        .transition(
            &mut handle,
            Track::new(sound(0.25, 4000)),
            0.0,
            Timing::NOW,
            Fade::NONE,
        )
        .unwrap();
    render(&mut mixer, 10, 10);
    let invalid = Track {
        layers: vec![
            Layer::new(sound(0.5, 4000)),
            Layer {
                gain: f32::NAN,
                ..Layer::new(sound(0.5, 4000))
            },
        ],
        looping: Loop::Off,
    };
    assert_eq!(
        music
            .transition(&mut handle, invalid, 0.0, Timing::NOW, Fade::NONE)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    handle.set_bus_gain(BusId::MASTER, 1.0, Fade::NONE).unwrap();
    assert_eq!(
        music
            .transition(
                &mut handle,
                Track::new(sound(0.75, 4000)),
                0.0,
                Timing::NOW,
                Fade::NONE
            )
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(render(&mut mixer, 50, 50), vec![0.25; 50]);
    assert_eq!(mixer.active_voices(), 1);
}

#[test]
fn queued_transitions_follow_execution_order_and_stop_the_actual_active_track() {
    let (mut mixer, mut handle, music) = setup(8, 64);
    music
        .transition(
            &mut handle,
            Track::new(sound(0.1, 4000)),
            0.0,
            Timing::NOW,
            Fade::NONE,
        )
        .unwrap();
    music
        .transition(
            &mut handle,
            Track::new(sound(0.2, 4000)),
            0.0,
            Timing::BAR,
            Fade::NONE,
        )
        .unwrap();
    music
        .transition(
            &mut handle,
            Track::new(sound(0.3, 4000)),
            0.0,
            at(1000),
            Fade::NONE,
        )
        .unwrap();
    music
        .stop(&mut handle, at(2500), Fade::linear(0.1))
        .unwrap();
    let out = render(&mut mixer, 3000, 113);
    assert_eq!(&out[..1000], vec![0.1; 1000]);
    assert_eq!(&out[1000..2000], vec![0.3; 1000]);
    assert_eq!(&out[2000..2500], vec![0.2; 500]);
    assert!(out[2600..].iter().all(|&s| s == 0.0));
    assert_eq!(mixer.active_voices(), 0);
}

#[test]
fn intensity_changes_only_the_active_track_during_a_crossfade() {
    let (mut mixer, mut handle) = Mixer::new(MixerConfig {
        sample_rate: RATE,
        channels: 2,
        ..Default::default()
    })
    .unwrap();
    let music = Music::new(
        &mut handle,
        Transport::new(RATE, 120.0, TimeSignature::default(), 0).unwrap(),
        BusId::MASTER,
    )
    .unwrap();
    let left = Arc::new(Audio::new(vec![vec![1.0; 2000], vec![0.0; 2000]], RATE).unwrap());
    let right = Arc::new(Audio::new(vec![vec![0.0; 2000], vec![1.0; 2000]], RATE).unwrap());
    let track = |source| Track {
        layers: vec![Layer {
            intensity: [0.0, 1.0],
            ..Layer::new(source)
        }],
        looping: Loop::Off,
    };
    music
        .transition(&mut handle, track(left), 1.0, Timing::NOW, Fade::NONE)
        .unwrap();
    music
        .transition(&mut handle, track(right), 1.0, at(100), Fade::linear(1.0))
        .unwrap();
    music
        .set_intensity(&mut handle, 0.0, at(300), Fade::NONE)
        .unwrap();
    let mut out = vec![0.0; 2 * 500];
    mixer.render(&mut out);
    // The outgoing left stem continues its original fade (0.8 at frame 300);
    // only the new right track becomes silent.
    assert!((out[600] - 0.8).abs() < 1e-6);
    assert!(out[601..].iter().step_by(2).all(|&s| s == 0.0));
}

#[test]
fn stop_ends_both_sides_of_an_unfinished_crossfade_but_keeps_cues() {
    for fade in [Fade::NONE, Fade::linear(0.1)] {
        let (mut mixer, mut handle, music) = setup(8, 64);
        music
            .transition(
                &mut handle,
                Track::new(sound(0.25, 4000)),
                0.0,
                Timing::NOW,
                Fade::NONE,
            )
            .unwrap();
        music
            .transition(
                &mut handle,
                Track::new(sound(0.5, 4000)),
                0.0,
                at(100),
                Fade::linear(1.0),
            )
            .unwrap();
        music
            .cue(&mut handle, sound(0.125, 300), 1.0, at(250))
            .unwrap();
        music.stop(&mut handle, at(300), fade).unwrap();
        let out = render(&mut mixer, 1200, 23);
        let end = if fade.seconds == 0.0 { 300 } else { 400 };
        assert_eq!(&out[end..550], vec![0.125; 550 - end]);
        assert!(out[550..].iter().all(|&s| s == 0.0));
        assert_eq!(mixer.active_voices(), 0);
    }
}

#[test]
fn prepared_operations_give_the_same_samples_for_every_callback_size() {
    let run = |block| {
        let (mut mixer, mut handle, music) = setup(8, 64);
        music
            .transition(
                &mut handle,
                Track::new(sound(0.25, 4000)),
                0.0,
                Timing::NOW,
                Fade::NONE,
            )
            .unwrap();
        music
            .transition(
                &mut handle,
                Track::new(sound(0.5, 4000)),
                0.0,
                Timing {
                    not_before: 750,
                    quantize: Quantize::Bar,
                },
                Fade::new(0.2, FadeCurve::EqualPower),
            )
            .unwrap();
        music
            .cue(
                &mut handle,
                sound(0.1, 50),
                1.0,
                Timing {
                    not_before: 123,
                    quantize: Quantize::Beat,
                },
            )
            .unwrap();
        render(&mut mixer, 3000, block)
    };
    let reference = run(1);
    for block in [7, 113, 512, 4000] {
        assert_eq!(run(block), reference, "block {block}");
    }
}

#[test]
fn transport_tracks_intensity_and_mixer_binding_are_validated() {
    for bpm in [0.0, -1.0, f64::NAN, f64::INFINITY, 1001.0] {
        assert_eq!(
            Transport::new(RATE, bpm, TimeSignature::default(), 0)
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
    }
    for signature in [
        TimeSignature {
            numerator: 0,
            denominator: 4,
        },
        TimeSignature {
            numerator: 4,
            denominator: 0,
        },
        TimeSignature {
            numerator: 4,
            denominator: 3,
        },
        TimeSignature {
            numerator: 33,
            denominator: 4,
        },
    ] {
        assert!(Transport::new(RATE, 120.0, signature, 0).is_err());
    }
    let (mut mixer, mut handle, music) = setup(8, 64);
    let (_, mut other, _) = setup(8, 64);
    assert_eq!(
        music
            .stop(&mut other, Timing::NOW, Fade::NONE)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert!(
        Music::new(
            &mut handle,
            Transport::new(2000, 120.0, TimeSignature::default(), 0).unwrap(),
            BusId::MASTER
        )
        .is_err()
    );
    for value in [-0.1, 1.1, f32::NAN, f32::INFINITY] {
        assert!(
            music
                .set_intensity(&mut handle, value, Timing::NOW, Fade::NONE)
                .is_err()
        );
    }
    assert!(
        music
            .transition(
                &mut handle,
                Track {
                    layers: vec![],
                    looping: Loop::Off
                },
                0.0,
                Timing::NOW,
                Fade::NONE
            )
            .is_err()
    );
    let mismatched = Track {
        layers: vec![Layer::new(sound(0.1, 100)), Layer::new(sound(0.1, 101))],
        looping: Loop::Off,
    };
    assert!(
        music
            .transition(&mut handle, mismatched, 0.0, Timing::NOW, Fade::NONE)
            .is_err()
    );
    assert!(
        music
            .cue(&mut handle, sound(0.1, 10), f32::NAN, Timing::NOW)
            .is_err()
    );
    assert!(
        music
            .stop(&mut handle, Timing::NOW, Fade::linear(f64::NAN))
            .is_err()
    );
    assert!(render(&mut mixer, 10, 10).iter().all(|&s| s == 0.0));
}

#[test]
fn recorded_streams_and_synthesized_scores_use_the_same_music_path() {
    use audiobox::{AudioFormat, encode, stream::Decoder};
    let (mut mixer, mut handle, music) = setup(8, 64);
    let encoded = encode(&sound(0.25, 4000), AudioFormat::Flac).unwrap();
    music
        .transition(
            &mut handle,
            Track::new(Decoder::new(encoded).unwrap()),
            0.0,
            Timing::NOW,
            Fade::NONE,
        )
        .unwrap();
    assert_eq!(render(&mut mixer, 128, 17), vec![0.25; 128]);
    #[cfg(feature = "synth")]
    {
        use audiobox::synth::{RenderOptions, Score, Synth};
        let score: Score = "instrument a wave=triangle\nchannel a = @a o4 l4 c"
            .parse()
            .unwrap();
        let synth = Synth::new(&score, RATE, &RenderOptions::default()).unwrap();
        music
            .transition(&mut handle, Track::new(synth), 0.0, Timing::NOW, Fade::NONE)
            .unwrap();
        let out = render(&mut mixer, 500, 19);
        assert!(out.iter().any(|&s| s.abs() > 0.01));
    }
}
