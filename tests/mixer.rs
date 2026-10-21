#![cfg(feature = "mixer")]
use audiobox::mixer::*;
use audiobox::stream::Decoder;
use audiobox::{Audio, AudioFormat, ErrorCode, FadeCurve, encode};
use std::f64::consts::{PI, TAU};
use std::sync::Arc;

const RATE: u32 = 48_000;

fn mixer(channels: usize, buses: &[&str], max_voices: usize) -> (Mixer, MixerHandle) {
    Mixer::new(MixerConfig {
        sample_rate: RATE,
        channels,
        max_voices,
        buses: buses.iter().map(|b| b.to_string()).collect(),
        ..MixerConfig::default()
    })
    .unwrap()
}

fn sine(frames: usize, rate: u32, hz: f64, amplitude: f64) -> Vec<f32> {
    (0..frames)
        .map(|i| (amplitude * (TAU * hz * i as f64 / rate as f64).sin()) as f32)
        .collect()
}

fn mono(samples: Vec<f32>, rate: u32) -> Arc<Audio> {
    Arc::new(Audio::new(vec![samples], rate).unwrap())
}

/// Renders `frames` frames in buffers of `chunk` frames.
fn render(mixer: &mut Mixer, frames: usize, chunk: usize) -> Vec<f32> {
    let ch = mixer.channels();
    let mut out = vec![0.0; frames * ch];
    for part in out.chunks_mut(chunk * ch) {
        mixer.render(part);
    }
    out
}

fn channel(out: &[f32], channels: usize, c: usize) -> Vec<f32> {
    out.iter().skip(c).step_by(channels).copied().collect()
}

fn events(handle: &mut MixerHandle) -> Vec<MixerEvent> {
    std::iter::from_fn(|| handle.poll_event()).collect()
}

fn close(a: f32, b: f64, tolerance: f64) -> bool {
    (a as f64 - b).abs() <= tolerance
}

#[test]
fn a_voice_renders_its_samples() {
    let samples = sine(4800, RATE, 440.0, 0.8);
    let sound = mono(samples.clone(), RATE);
    // Mono output: the samples themselves.
    let (mut m, mut h) = mixer(1, &[], 8);
    let id = h.play(sound.clone(), &Play::default()).unwrap();
    let out = render(&mut m, 6000, 512);
    assert_eq!(&out[..4800], samples.as_slice());
    assert!(out[4800..].iter().all(|&s| s == 0.0));
    assert_eq!(events(&mut h), [MixerEvent::Ended(id)]);
    assert_eq!(m.active_voices(), 0);
    // Stereo output: a centred mono voice at -3 dB on each side.
    let (mut m, mut h) = mixer(2, &[], 8);
    h.play(sound, &Play::default()).unwrap();
    let out = render(&mut m, 4800, 333);
    let centre = (PI / 4.0).cos();
    for (i, &s) in samples.iter().enumerate() {
        assert!(
            close(out[2 * i], s as f64 * centre, 1e-6)
                && close(out[2 * i + 1], s as f64 * centre, 1e-6)
        );
    }
    // A centred stereo voice is unchanged.
    let stereo =
        Arc::new(Audio::new(vec![samples.clone(), sine(4800, RATE, 220.0, 0.5)], RATE).unwrap());
    let (mut m, mut h) = mixer(2, &[], 8);
    h.play(stereo.clone(), &Play::default()).unwrap();
    let out = render(&mut m, 4800, 4800);
    assert_eq!(channel(&out, 2, 0), stereo.channel_data(0).unwrap());
    assert_eq!(channel(&out, 2, 1), stereo.channel_data(1).unwrap());
}

#[test]
fn a_loop_wraps_without_a_click() {
    // A 100 Hz sine has 480 frames per period: loop 10 periods after a 960-frame intro.
    let samples = sine(10_000, RATE, 100.0, 0.9);
    let max_slope = TAU * 100.0 / RATE as f64 * 0.9;
    for (rate, hz) in [(RATE, 100.0), (44_100, 100.0)] {
        let period = (rate as f64 / hz) as u64;
        let source = if rate == RATE {
            samples.clone()
        } else {
            sine(10_000, rate, hz, 0.9)
        };
        let (mut m, mut h) = mixer(1, &[], 4);
        let looping = Loop::Region {
            start: 2 * period,
            end: 12 * period,
        };
        h.play(
            mono(source, rate),
            &Play {
                looping,
                ..Play::default()
            },
        )
        .unwrap();
        let out = render(&mut m, 48_000, 700);
        let steps: Vec<f64> = out
            .windows(2)
            .skip(64)
            .map(|w| (w[1] - w[0]).abs() as f64)
            .collect();
        let worst = steps.iter().copied().fold(0.0, f64::max);
        // Tolerance: the sine's own steepest step, plus 1% (and the resampler's ripple).
        assert!(
            worst <= max_slope * 1.01,
            "{rate} Hz source: step {worst} > {max_slope}"
        );
        assert!(
            out[40_000..].iter().any(|&s| s.abs() > 0.8),
            "still looping"
        );
    }
    // A whole-sound loop of exact periods repeats the samples exactly.
    let (mut m, _h) = {
        let (m, mut h) = mixer(1, &[], 4);
        h.play(
            mono(samples[..4800].to_vec(), RATE),
            &Play {
                looping: Loop::Whole,
                ..Play::default()
            },
        )
        .unwrap();
        (m, h)
    };
    let out = render(&mut m, 4800 * 3, 1000);
    assert_eq!(&out[4800..9600], &samples[..4800]);
    assert_eq!(&out[9600..], &samples[..4800]);
}

#[test]
fn a_fade_reaches_its_gain_at_its_end() {
    let ones = mono(vec![1.0; 48_000], RATE);
    for curve in [
        FadeCurve::Linear,
        FadeCurve::EqualPower,
        FadeCurve::Exponential,
    ] {
        let (mut m, mut h) = mixer(1, &[], 4);
        let id = h
            .play(
                ones.clone(),
                &Play {
                    fade_in: Fade::new(0.01, curve),
                    ..Play::default()
                },
            )
            .unwrap();
        h.at(10_000)
            .set_gain(id, 0.25, Fade::new(0.1, curve))
            .unwrap();
        h.at(30_000).stop(id, Fade::new(0.05, curve)).unwrap();
        let out = render(&mut m, 40_000, 777);
        assert_eq!(out[0], 0.0);
        assert!(close(out[480], 1.0, 1e-6), "fade-in ends at 1 ({curve:?})");
        assert!(close(out[9_999], 1.0, 1e-6));
        assert!(
            close(out[10_000 + 4800], 0.25, 1e-6),
            "{curve:?}: {}",
            out[14_800]
        );
        assert!(close(out[20_000], 0.25, 1e-6));
        // The stop fades from 0.25 to 0 over 2400 frames, then the voice ends.
        assert!(close(out[30_000], 0.25, 1e-6));
        assert!(out[30_000 + 2400..].iter().all(|&s| s == 0.0));
        let midpoint = out[10_000 + 2400] as f64;
        let expected = match curve {
            FadeCurve::Linear => 0.625,
            FadeCurve::EqualPower => 0.25 + 0.75 * (PI / 4.0).sin(),
            FadeCurve::Exponential => 0.25 + 0.75 * 0.25,
        };
        assert!(
            (midpoint - expected).abs() < 1e-6,
            "{curve:?} midpoint {midpoint}"
        );
        assert_eq!(events(&mut h), [MixerEvent::Ended(id)]);
    }
}

#[test]
fn pan_splits_a_mono_voice_as_stated() {
    let ones = mono(vec![1.0; 1000], RATE);
    for pan in [-1.0f32, -0.5, 0.0, 0.25, 1.0] {
        let (mut m, mut h) = mixer(2, &[], 4);
        h.play(
            ones.clone(),
            &Play {
                pan,
                ..Play::default()
            },
        )
        .unwrap();
        let out = render(&mut m, 10, 10);
        let theta = (pan as f64 + 1.0) * PI / 4.0;
        assert!(
            close(out[0], theta.cos(), 1e-6) && close(out[1], theta.sin(), 1e-6),
            "pan {pan}"
        );
        let power = (out[0] as f64).powi(2) + (out[1] as f64).powi(2);
        assert!((power - 1.0).abs() < 1e-6, "constant power at {pan}");
    }
    // A stereo voice panned left keeps its left channel and loses its right.
    let stereo = Arc::new(Audio::new(vec![vec![0.5; 100], vec![0.5; 100]], RATE).unwrap());
    let (mut m, mut h) = mixer(2, &[], 4);
    let id = h
        .play(
            stereo,
            &Play {
                pan: -1.0,
                ..Play::default()
            },
        )
        .unwrap();
    h.at(50).set_pan(id, 0.0, Fade::NONE).unwrap();
    let out = render(&mut m, 60, 60);
    assert_eq!((out[0], out[1]), (0.5, 0.0));
    assert_eq!((out[100], out[101]), (0.5, 0.5));
}

#[test]
fn a_bus_scales_its_voices() {
    let ones = mono(vec![1.0; 10_000], RATE);
    let (mut m, mut h) = mixer(1, &["music", "effects"], 8);
    let (music, effects) = (h.bus("music").unwrap(), h.bus("effects").unwrap());
    assert_eq!(h.bus("voices"), None);
    h.play(
        ones.clone(),
        &Play {
            bus: music,
            gain: 0.5,
            ..Play::default()
        },
    )
    .unwrap();
    h.play(
        ones,
        &Play {
            bus: effects,
            gain: 0.25,
            ..Play::default()
        },
    )
    .unwrap();
    h.set_bus_gain(music, 0.5, Fade::NONE).unwrap();
    h.at(1000).set_bus_gain(effects, 0.0, Fade::NONE).unwrap();
    h.at(2000)
        .set_bus_gain(BusId::MASTER, 2.0, Fade::linear(0.01))
        .unwrap();
    let out = render(&mut m, 3000, 128);
    assert!(close(out[0], 0.25 + 0.25, 1e-7));
    assert!(close(out[1500], 0.25, 1e-7));
    assert!(
        close(out[2240], 0.375, 1e-6),
        "master fading to 2: {}",
        out[2240]
    );
    assert!(close(out[2999], 0.5, 1e-7));
}

#[test]
fn overlapping_voices_sum() {
    let a = sine(5000, RATE, 300.0, 0.3);
    let b = sine(5000, RATE, 450.0, 0.2);
    let (mut m, mut h) = mixer(1, &[], 8);
    let sound = mono(a.clone(), RATE);
    // Two voices of the same sound, one starting 100 frames later, and another sound.
    h.play(sound.clone(), &Play::default()).unwrap();
    h.at(100).play(sound, &Play::default()).unwrap();
    h.play(mono(b.clone(), RATE), &Play::default()).unwrap();
    let out = render(&mut m, 5200, 256);
    for (i, &sample) in out.iter().enumerate() {
        let expected = a.get(i).copied().unwrap_or(0.0) as f64
            + i.checked_sub(100)
                .and_then(|j| a.get(j))
                .copied()
                .unwrap_or(0.0) as f64
            + b.get(i).copied().unwrap_or(0.0) as f64;
        assert!(close(sample, expected, 1e-6), "frame {i}");
    }
}

#[test]
fn the_voice_bound_and_its_policy() {
    let ones = mono(vec![1.0; 48_000], RATE);
    let (mut m, mut h) = mixer(1, &[], 2);
    let low = h
        .play(
            ones.clone(),
            &Play {
                priority: 10,
                gain: 0.1,
                ..Play::default()
            },
        )
        .unwrap();
    let high = h
        .play(
            ones.clone(),
            &Play {
                priority: 200,
                gain: 0.2,
                ..Play::default()
            },
        )
        .unwrap();
    render(&mut m, 100, 100);
    // A third voice of middle priority takes the low voice's place, which fades out.
    let middle = h
        .play(
            ones.clone(),
            &Play {
                priority: 100,
                gain: 0.4,
                ..Play::default()
            },
        )
        .unwrap();
    let out = render(&mut m, 1000, 100);
    assert_eq!(
        events(&mut h),
        [MixerEvent::Stolen(low), MixerEvent::Ended(low)]
    );
    assert!(close(out[0], 0.1 + 0.2 + 0.4, 1e-6));
    assert!(
        close(out[240], 0.2 + 0.4, 1e-6),
        "the stolen voice is gone after 5 ms"
    );
    // A voice of lower priority than every playing voice is rejected.
    let rejected = h
        .play(
            ones.clone(),
            &Play {
                priority: 50,
                ..Play::default()
            },
        )
        .unwrap();
    render(&mut m, 100, 100);
    assert_eq!(events(&mut h), [MixerEvent::Rejected(rejected)]);
    // Among equal priorities, the oldest gives way.
    let newest = h
        .play(
            ones,
            &Play {
                priority: 200,
                ..Play::default()
            },
        )
        .unwrap();
    render(&mut m, 1000, 100);
    assert_eq!(
        events(&mut h),
        [MixerEvent::Stolen(middle), MixerEvent::Ended(middle)]
    );
    assert_eq!(m.active_voices(), 2);
    h.stop_all(Fade::NONE).unwrap();
    render(&mut m, 1, 1);
    let mut ended = events(&mut h);
    ended.sort_by_key(|e| format!("{e:?}"));
    assert_eq!(ended, [MixerEvent::Ended(high), MixerEvent::Ended(newest)]);
}

#[test]
fn resampling_keeps_a_sines_frequency() {
    // 1 kHz at 44.1 kHz played at 48 kHz.
    let source = sine(44_100, 44_100, 1000.0, 0.5);
    let (mut m, mut h) = mixer(1, &[], 4);
    h.play(mono(source, 44_100), &Play::default()).unwrap();
    let out = render(&mut m, 48_500, 512);
    let crossings: Vec<f64> = (1..47_000)
        .filter(|&i| out[i - 1] < 0.0 && out[i] >= 0.0)
        .map(|i| {
            let (a, b) = (out[i - 1] as f64, out[i] as f64);
            i as f64 - 1.0 + a / (a - b)
        })
        .collect();
    let hz = (crossings.len() - 1) as f64 * RATE as f64
        / (crossings[crossings.len() - 1] - crossings[0]);
    // Tolerance: 0.01%, 0.1 Hz.
    assert!((hz - 1000.0).abs() < 0.1, "{hz} Hz");
    let peak = out[1000..46_000].iter().fold(0.0f32, |p, s| p.max(s.abs()));
    assert!((peak - 0.5).abs() < 0.005, "amplitude {peak}");
    // The source ends after one second; the voice ends with it, after the filter's tail.
    assert!(out[48_020..].iter().all(|&s| s == 0.0));
    assert_eq!(m.active_voices(), 0);
}

#[test]
fn empty_and_uneven_buffers() {
    let ones = mono(vec![1.0; 100], RATE);
    let (mut m, mut h) = mixer(2, &[], 4);
    let id = h.play(ones, &Play::default()).unwrap();
    // A render of 0 frames applies the due changes without advancing.
    m.render(&mut []);
    assert_eq!((m.frame(), m.active_voices()), (0, 1));
    h.set_pan(id, -1.0, Fade::NONE).unwrap();
    m.render(&mut [0.0]);
    assert_eq!(m.frame(), 0);
    // Five samples of stereo: two frames, and the last sample set to 0.
    let mut out = [9.0f32; 5];
    m.render(&mut out);
    assert_eq!(out, [1.0, 0.0, 1.0, 0.0, 0.0]);
    assert_eq!((m.frame(), h.frame()), (2, 2));
}

#[test]
fn renders_do_not_depend_on_buffer_sizes() {
    let flac = encode(
        &Audio::new(vec![sine(30_000, 44_100, 523.0, 0.4)], 44_100).unwrap(),
        AudioFormat::Flac,
    )
    .unwrap();
    let flac: Arc<[u8]> = flac.into();
    let run = |chunk: usize| {
        let (mut m, mut h) = mixer(2, &["music"], 4);
        let music = h.bus("music").unwrap();
        let tone = mono(sine(20_000, 22_050, 300.0, 0.3), 22_050);
        let song = h
            .play(
                Decoder::new(flac.clone()).unwrap(),
                &Play {
                    bus: music,
                    looping: Loop::Whole,
                    ..Play::default()
                },
            )
            .unwrap();
        let a = h
            .at(1234)
            .play(
                tone.clone(),
                &Play {
                    pan: 0.3,
                    ..Play::default()
                },
            )
            .unwrap();
        h.at(5000)
            .set_pan(a, -0.7, Fade::new(0.2, FadeCurve::EqualPower))
            .unwrap();
        h.at(7777)
            .set_bus_gain(music, 0.3, Fade::linear(0.3))
            .unwrap();
        h.at(20_000)
            .play(
                tone,
                &Play {
                    gain: 0.5,
                    ..Play::default()
                },
            )
            .unwrap();
        h.at(40_001).stop(song, Fade::linear(0.1)).unwrap();
        render(&mut m, 60_000, chunk)
    };
    let reference = run(60_000);
    for chunk in [1, 64, 256, 1000, 4095] {
        let out = run(chunk);
        assert!(
            out.iter()
                .zip(&reference)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "chunks of {chunk}"
        );
    }
    assert!(reference.iter().any(|&s| s != 0.0));
}

#[test]
fn streams_play_and_loop() {
    // A decoder looping: the output repeats the decoded stream.
    let samples = sine(9600, RATE, 250.0, 0.5);
    let wav = encode(
        &Audio::new(vec![samples.clone()], RATE).unwrap(),
        AudioFormat::Wav,
    )
    .unwrap();
    let decoded = audiobox::decode(&wav).unwrap();
    let decoded = decoded.channel_data(0).unwrap();
    let (mut m, mut h) = mixer(1, &[], 4);
    h.play(
        Decoder::new(wav.clone()).unwrap(),
        &Play {
            looping: Loop::Whole,
            ..Play::default()
        },
    )
    .unwrap();
    h.play(
        Decoder::new(wav).unwrap(),
        &Play {
            looping: Loop::Region {
                start: 4800,
                end: 9600,
            },
            gain: 0.0,
            ..Play::default()
        },
    )
    .unwrap();
    let out = render(&mut m, 9600 * 3, 500);
    for k in 0..3 {
        assert_eq!(&out[k * 9600..(k + 1) * 9600], decoded);
    }
    // A failing stream reports its error and stops.
    let mut broken = encode(&Audio::new(vec![samples], RATE).unwrap(), AudioFormat::Flac).unwrap();
    let n = broken.len();
    broken.truncate(n - 200);
    let (mut m, mut h) = mixer(1, &[], 4);
    let id = h
        .play(Decoder::new(broken).unwrap(), &Play::default())
        .unwrap();
    render(&mut m, 20_000, 1000);
    assert_eq!(
        events(&mut h),
        [MixerEvent::Failed(id, ErrorCode::DecodeError)]
    );
}

#[cfg(feature = "synth")]
#[test]
fn a_synthesizer_plays_as_a_stream() {
    use audiobox::synth::{RenderOptions, Score, Synth};
    let score: Score = "instrument p wave=triangle\nchannel a = @p o4 c8 e8 g8"
        .parse()
        .unwrap();
    let offline = score
        .render_with(
            RATE,
            &RenderOptions {
                gain: 1.0,
                ..Default::default()
            },
        )
        .unwrap();
    let synth = Synth::new(
        &score,
        RATE,
        &RenderOptions {
            gain: 1.0,
            ..Default::default()
        },
    )
    .unwrap();
    let (mut m, mut h) = mixer(1, &[], 4);
    h.play(synth, &Play::default()).unwrap();
    let out = render(&mut m, offline.frames() + 100, 300);
    assert_eq!(&out[..offline.frames()], offline.channel_data(0).unwrap());
}

#[test]
fn invalid_commands_are_rejected() {
    assert!(
        Mixer::new(MixerConfig {
            channels: 3,
            ..MixerConfig::default()
        })
        .is_err()
    );
    assert!(
        Mixer::new(MixerConfig {
            max_voices: 0,
            ..MixerConfig::default()
        })
        .is_err()
    );
    assert!(
        Mixer::new(MixerConfig {
            buses: vec!["a".into(), "a".into()],
            ..MixerConfig::default()
        })
        .is_err()
    );
    let (_m, mut h) = mixer(2, &[], 4);
    let quad = Arc::new(Audio::new(vec![vec![0.0; 10]; 4], RATE).unwrap());
    assert_eq!(
        h.play(quad, &Play::default()).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let one = mono(vec![0.0; 10], RATE);
    let bad_loop = Play {
        looping: Loop::Region { start: 5, end: 11 },
        ..Play::default()
    };
    assert!(h.play(one.clone(), &bad_loop).is_err());
    assert!(
        h.play(
            one.clone(),
            &Play {
                gain: f32::NAN,
                ..Play::default()
            }
        )
        .is_err()
    );
    assert!(
        h.play(
            one.clone(),
            &Play {
                bus: BusId::MASTER,
                fade_in: Fade::linear(-1.0),
                ..Play::default()
            }
        )
        .is_err()
    );
    // The queue holds `queue` commands until a render takes them.
    let (mut m, mut h) = Mixer::new(MixerConfig {
        queue: 3,
        ..MixerConfig::default()
    })
    .unwrap();
    for _ in 0..3 {
        h.play(one.clone(), &Play::default()).unwrap();
    }
    assert_eq!(
        h.play(one.clone(), &Play::default()).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    m.render(&mut [0.0; 2]);
    h.play(one.clone(), &Play::default()).unwrap();
    drop(m);
    assert!(h.play(one, &Play::default()).is_err());
}

/// Mean frequency of `samples[from..to]` from the times of upward zero crossings.
fn frequency(samples: &[f32], from: usize, to: usize) -> f64 {
    let crossings: Vec<f64> = (from.max(1)..to)
        .filter(|&i| samples[i - 1] < 0.0 && samples[i] >= 0.0)
        .map(|i| {
            let (a, b) = (samples[i - 1] as f64, samples[i] as f64);
            i as f64 - 1.0 + a / (a - b)
        })
        .collect();
    (crossings.len() - 1) as f64 * RATE as f64 / (crossings[crossings.len() - 1] - crossings[0])
}

#[test]
fn rate_and_pitch_change_speed_and_frequency() {
    let tone = mono(sine(48_000, RATE, 300.0, 0.5), RATE);
    let render_with = |play: Play| {
        let (mut m, mut h) = mixer(1, &[], 4);
        h.play(tone.clone(), &play).unwrap();
        render(&mut m, 50_000, 512)
    };
    // A rate of 2 halves the duration and doubles the frequency.
    let fast = render_with(Play {
        rate: 2.0,
        ..Play::default()
    });
    let last = fast.iter().rposition(|s| s.abs() > 1e-4).unwrap();
    assert!((23_990..24_010).contains(&last), "ends at {last}");
    assert!((frequency(&fast, 1000, 23_000) / 600.0 - 1.0).abs() < 1e-3);
    // Pitch +12 semitones doubles the frequency; +7 is a fifth.
    let octave = render_with(Play {
        pitch: 12.0,
        ..Play::default()
    });
    assert!((frequency(&octave, 1000, 23_000) / 600.0 - 1.0).abs() < 1e-3);
    let fifth = render_with(Play {
        pitch: 7.0,
        ..Play::default()
    });
    let expected = 300.0 * 2f64.powf(7.0 / 12.0);
    assert!((frequency(&fifth, 1000, 30_000) / expected - 1.0).abs() < 1e-3);
    // Rate and pitch combine, also for a resampled source.
    let (mut m, mut h) = mixer(1, &[], 4);
    h.play(
        mono(sine(44_100, 44_100, 1000.0, 0.5), 44_100),
        &Play {
            rate: 0.5,
            pitch: 12.0,
            ..Play::default()
        },
    )
    .unwrap();
    let out = render(&mut m, 40_000, 700);
    assert!((frequency(&out, 1000, 39_000) / 1000.0 - 1.0).abs() < 1e-3);
    // A voice back at speed 1 after a change keeps playing in tune.
    let (mut m, mut h) = mixer(1, &[], 4);
    let id = h.play(tone.clone(), &Play::default()).unwrap();
    h.at(1000).set_pitch(id, 3.0, Fade::NONE).unwrap();
    h.at(5000).set_pitch(id, 0.0, Fade::NONE).unwrap();
    let out = render(&mut m, 40_000, 512);
    assert!((frequency(&out, 6000, 39_000) / 300.0 - 1.0).abs() < 1e-3);
}

#[test]
fn a_rate_ramp_reaches_its_target_without_a_click() {
    let amplitude = 0.5;
    let tone = mono(sine(96_000, RATE, 200.0, amplitude), RATE);
    let (mut m, mut h) = mixer(1, &[], 4);
    let id = h.play(tone, &Play::default()).unwrap();
    h.at(4800).set_rate(id, 2.0, Fade::linear(0.5)).unwrap();
    let out = render(&mut m, 40_000, 333);
    // Tolerance: the steepest step of the sine at its highest frequency, plus 2%.
    let bound = std::f64::consts::TAU * 400.0 / RATE as f64 * amplitude * 1.02;
    let worst = out[..39_000]
        .windows(2)
        .map(|w| (w[1] - w[0]).abs() as f64)
        .fold(0.0, f64::max);
    assert!(worst <= bound, "step {worst} > {bound}");
    assert!(
        (frequency(&out, 1000, 4700) / 200.0 - 1.0).abs() < 1e-3,
        "before the ramp"
    );
    assert!(
        (frequency(&out, 30_000, 39_000) / 400.0 - 1.0).abs() < 1e-3,
        "after the ramp"
    );
}

#[test]
fn a_seed_gives_the_same_variations() {
    let step = mono(sine(2400, RATE, 500.0, 0.5), RATE);
    let run = |seed: u64| {
        let (mut m, mut h) = Mixer::new(MixerConfig {
            sample_rate: RATE,
            channels: 1,
            seed,
            ..MixerConfig::default()
        })
        .unwrap();
        let footstep = Play {
            variation: Variation {
                pitch: 2.0,
                gain_db: 3.0,
            },
            ..Play::default()
        };
        for i in 0..6 {
            h.at(i * 4800).play(step.clone(), &footstep).unwrap();
        }
        render(&mut m, 6 * 4800, 1000)
    };
    let (a, b, c) = (run(7), run(7), run(8));
    assert!(
        a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()),
        "same seed, same sounds"
    );
    assert!(
        a.iter().zip(&c).any(|(x, y)| x != y),
        "another seed, other sounds"
    );
    // Each step differs in pitch and level, within the stated ranges.
    let mut seen = Vec::new();
    for i in 0..6 {
        let part = &a[i * 4800..i * 4800 + 2000];
        let hz = frequency(part, 1, part.len());
        let peak = part.iter().fold(0.0f32, |p, s| p.max(s.abs())) as f64;
        let semitones = 12.0 * (hz / 500.0).log2();
        let db = 20.0 * (peak / 0.5).log10();
        assert!(
            semitones.abs() <= 2.0 + 1e-3 && db.abs() <= 3.0 + 0.05,
            "step {i}: {semitones} st, {db} dB"
        );
        seen.push((semitones * 100.0).round() as i64);
    }
    seen.dedup();
    assert!(seen.len() > 3, "the steps vary: {seen:?}");
}

#[test]
fn invalid_rates_are_rejected() {
    let (_m, mut h) = mixer(1, &[], 4);
    let one = mono(vec![0.0; 10], RATE);
    for play in [
        Play {
            rate: 0.0,
            ..Play::default()
        },
        Play {
            rate: 9.0,
            ..Play::default()
        },
        Play {
            pitch: 49.0,
            ..Play::default()
        },
        Play {
            variation: Variation {
                pitch: 25.0,
                gain_db: 0.0,
            },
            ..Play::default()
        },
        Play {
            rate: f32::NAN,
            ..Play::default()
        },
    ] {
        assert_eq!(
            h.play(one.clone(), &play).unwrap_err().code,
            ErrorCode::InvalidArgument,
            "{play:?}"
        );
    }
    let id = h.play(one, &Play::default()).unwrap();
    assert!(h.set_rate(id, -1.0, Fade::NONE).is_err());
    assert!(h.set_pitch(id, f32::INFINITY, Fade::NONE).is_err());
}

/// A sound whose sample at frame `i` is `i / 100_000`, so positions can be read back.
fn counting(frames: usize) -> Vec<f32> {
    (0..frames).map(|i| i as f32 / 100_000.0).collect()
}

#[test]
fn a_paused_voice_resumes_from_the_same_sample() {
    let samples = counting(20_000);
    let (mut m, mut h) = mixer(1, &[], 4);
    let id = h
        .play(mono(samples.clone(), RATE), &Play::default())
        .unwrap();
    h.at(1000).pause(id, Fade::NONE).unwrap();
    h.at(3000).resume(id, Fade::NONE).unwrap();
    let out = render(&mut m, 6000, 256);
    assert_eq!(&out[..1000], &samples[..1000]);
    assert!(out[1000..3000].iter().all(|&s| s == 0.0));
    assert_eq!(&out[3000..6000], &samples[1000..4000]);

    // With fades, the voice fades out over 480 frames, keeps still, and fades back in.
    let (mut m, mut h) = mixer(1, &[], 4);
    let id = h
        .play(mono(samples.clone(), RATE), &Play::default())
        .unwrap();
    h.at(1000).pause(id, Fade::linear(0.01)).unwrap();
    h.at(3000).resume(id, Fade::linear(0.01)).unwrap();
    let out = render(&mut m, 6000, 300);
    assert!(
        (out[1240] as f64 - samples[1240] as f64 * 0.5).abs() < 1e-7,
        "halfway through the fade"
    );
    assert!(out[1480..3000].iter().all(|&s| s == 0.0));
    assert_eq!(out[3000], 0.0);
    assert!((out[3240] as f64 - samples[1480 + 240] as f64 * 0.5).abs() < 1e-7);
    assert_eq!(&out[3480..6000], &samples[1960..4480]);
    // Stopping a paused voice ends it at once.
    h.pause(id, Fade::NONE).unwrap();
    render(&mut m, 10, 10);
    h.stop(id, Fade::linear(1.0)).unwrap();
    render(&mut m, 10, 10);
    assert!(events(&mut h).contains(&MixerEvent::Ended(id)));
}

#[test]
fn pausing_a_bus_pauses_its_voices_and_no_others() {
    let samples = counting(20_000);
    let (mut m, mut h) = mixer(2, &["game", "menu"], 8);
    let (game, menu) = (h.bus("game").unwrap(), h.bus("menu").unwrap());
    let left = Play {
        bus: game,
        pan: -1.0,
        ..Play::default()
    };
    let right = Play {
        bus: menu,
        pan: 1.0,
        ..Play::default()
    };
    h.play(mono(samples.clone(), RATE), &left).unwrap();
    h.play(mono(samples.clone(), RATE), &right).unwrap();
    h.at(1000).pause_bus(game, Fade::NONE).unwrap();
    // A voice started on the paused bus waits for it.
    h.at(1500).play(mono(samples.clone(), RATE), &left).unwrap();
    h.at(3000).resume_bus(game, Fade::NONE).unwrap();
    let out = render(&mut m, 5000, 128);
    let (l, r) = (channel(&out, 2, 0), channel(&out, 2, 1));
    assert_eq!(&r[..5000], &samples[..5000], "the menu keeps playing");
    assert_eq!(&l[..1000], &samples[..1000]);
    assert!(l[1000..3000].iter().all(|&s| s == 0.0));
    for i in 3000..5000 {
        let expected = samples[i - 2000] + samples[i - 3000];
        assert!((l[i] - expected).abs() < 1e-6, "frame {i}");
    }
    // Pausing the master pauses every voice.
    h.pause_bus(BusId::MASTER, Fade::NONE).unwrap();
    let out = render(&mut m, 100, 100);
    assert!(out.iter().all(|&s| s == 0.0));
    h.resume_bus(BusId::MASTER, Fade::NONE).unwrap();
    let out = render(&mut m, 1, 1);
    assert_eq!(out[1], samples[5000]);
}

#[test]
fn a_voice_starts_partway_through() {
    let samples = counting(48_000);
    let sound = mono(samples.clone(), RATE);
    let first = |start: audiobox::TimePosition| {
        let (mut m, mut h) = mixer(1, &[], 4);
        h.play(
            sound.clone(),
            &Play {
                start,
                ..Play::default()
            },
        )
        .unwrap();
        render(&mut m, 10, 10)
    };
    assert_eq!(
        first(audiobox::TimePosition::Frames(5000))[0],
        samples[5000]
    );
    assert_eq!(
        first(audiobox::TimePosition::Seconds(0.25))[0],
        samples[12_000]
    );
    assert_eq!(
        first(audiobox::TimePosition::Seconds(-0.1))[0],
        samples[43_200]
    );
    // A stream seeks to its start.
    let wav = encode(
        &Audio::new(vec![samples.clone()], RATE).unwrap(),
        AudioFormat::Wav,
    )
    .unwrap();
    let decoded = audiobox::decode(&wav).unwrap();
    let (mut m, mut h) = mixer(1, &[], 4);
    let start = audiobox::TimePosition::Frames(30_000);
    h.play(
        Decoder::new(wav).unwrap(),
        &Play {
            start,
            ..Play::default()
        },
    )
    .unwrap();
    let out = render(&mut m, 18_100, 1000);
    assert_eq!(&out[..18_000], &decoded.channel_data(0).unwrap()[30_000..]);
    assert!(out[18_000..].iter().all(|&s| s == 0.0));
    let (_m, mut h) = mixer(1, &[], 4);
    let past = Play {
        start: audiobox::TimePosition::Frames(48_001),
        ..Play::default()
    };
    assert!(h.play(sound, &past).is_err());
}

#[test]
fn the_position_follows_the_frames_rendered() {
    let (mut m, mut h) = mixer(1, &[], 4);
    let id = h
        .play(
            mono(counting(4800), RATE),
            &Play {
                looping: Loop::Whole,
                ..Play::default()
            },
        )
        .unwrap();
    assert_eq!(
        h.voice_status(id),
        None,
        "not started before the first render"
    );
    render(&mut m, 1000, 333);
    assert_eq!(
        h.voice_status(id),
        Some(VoiceStatus {
            state: VoiceState::Playing,
            position: 1000
        })
    );
    // The position wraps with the loop.
    render(&mut m, 5000, 333);
    assert_eq!(h.voice_status(id).unwrap().position, 1200);
    h.pause(id, Fade::NONE).unwrap();
    render(&mut m, 2000, 500);
    assert_eq!(
        h.voice_status(id),
        Some(VoiceStatus {
            state: VoiceState::Paused,
            position: 1200
        })
    );
    h.resume(id, Fade::NONE).unwrap();
    h.stop(id, Fade::linear(0.1)).unwrap();
    render(&mut m, 100, 100);
    assert_eq!(h.voice_status(id).unwrap().state, VoiceState::Stopping);
    render(&mut m, 5000, 500);
    assert_eq!(h.voice_status(id).unwrap().state, VoiceState::Finished);
    // Half speed plays half as many source frames.
    let (mut m, mut h) = mixer(1, &[], 4);
    let id = h
        .play(
            mono(counting(48_000), RATE),
            &Play {
                rate: 0.5,
                ..Play::default()
            },
        )
        .unwrap();
    render(&mut m, 2000, 256);
    assert_eq!(h.voice_status(id).unwrap().position, 1000);
}
