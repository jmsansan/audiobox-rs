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

#[test]
fn a_fourth_instance_follows_the_policy() {
    let coin = mono(vec![1.0; 48_000], RATE);
    for policy in [
        OverLimit::Refuse,
        OverLimit::StopOldest,
        OverLimit::StopQuietest,
    ] {
        let (mut m, mut h) = mixer(1, &[], 16);
        let limit = Instances::of(&coin).max(3).policy(policy);
        let play = |gain| Play {
            gain,
            instances: Some(limit),
            ..Play::default()
        };
        let ids: Vec<VoiceId> = [0.4, 0.1, 0.3]
            .iter()
            .enumerate()
            .map(|(i, &gain)| {
                h.at(i as u64 * 100)
                    .play(coin.clone(), &play(gain))
                    .unwrap()
            })
            .collect();
        let fourth = h.at(1000).play(coin.clone(), &play(0.2)).unwrap();
        let out = render(&mut m, 2000, 256);
        let got = events(&mut h);
        match policy {
            OverLimit::Refuse => {
                assert_eq!(got, [MixerEvent::Limited(fourth)]);
                assert!(close(out[1500], 0.8, 1e-6));
            }
            OverLimit::StopOldest => {
                assert_eq!(got, [MixerEvent::Stolen(ids[0]), MixerEvent::Ended(ids[0])]);
                assert!(close(out[1500], 0.1 + 0.3 + 0.2, 1e-6));
            }
            OverLimit::StopQuietest => {
                assert_eq!(got, [MixerEvent::Stolen(ids[1]), MixerEvent::Ended(ids[1])]);
                assert!(close(out[1500], 0.4 + 0.3 + 0.2, 1e-6));
            }
        }
        assert_eq!(m.active_voices(), 3, "{policy:?}");
    }
}

#[test]
fn a_start_within_the_minimum_gap_is_refused() {
    let coin = mono(vec![0.5; 100], RATE);
    let (mut m, mut h) = mixer(1, &[], 16);
    let play = Play {
        instances: Some(Instances::of(&coin).min_gap(0.05)),
        ..Play::default()
    };
    let first = h.at(0).play(coin.clone(), &play).unwrap();
    let early = h.at(2000).play(coin.clone(), &play).unwrap();
    let later = h.at(2400).play(coin.clone(), &play).unwrap();
    // Another sound is not limited by this one's gap.
    let other = mono(vec![0.25; 100], RATE);
    h.at(2401)
        .play(
            other.clone(),
            &Play {
                instances: Some(Instances::of(&other).min_gap(0.05)),
                ..Play::default()
            },
        )
        .unwrap();
    let out = render(&mut m, 3000, 500);
    let got = events(&mut h);
    assert!(got.contains(&MixerEvent::Limited(early)), "{got:?}");
    assert!(got.contains(&MixerEvent::Ended(first)) && got.contains(&MixerEvent::Ended(later)));
    assert_eq!(out[2000], 0.0);
    assert!(close(out[2401], 0.75, 1e-6));
}

#[test]
fn instance_limits_and_priorities_together() {
    let coin = mono(vec![1.0; 48_000], RATE);
    let music = mono(vec![1.0; 48_000], RATE);
    let (mut m, mut h) = mixer(1, &[], 2);
    h.play(
        music,
        &Play {
            priority: 255,
            gain: 0.5,
            ..Play::default()
        },
    )
    .unwrap();
    let limit = Instances::of(&coin).max(2).policy(OverLimit::StopOldest);
    let coins = Play {
        gain: 0.1,
        instances: Some(limit),
        ..Play::default()
    };
    let a = h.at(100).play(coin.clone(), &coins).unwrap();
    // The mixer's bound of two voices: this coin takes the place of the older coin
    // through its priority rule, and its own limit is not reached.
    let b = h.at(200).play(coin.clone(), &coins).unwrap();
    render(&mut m, 1000, 256);
    assert_eq!(
        events(&mut h),
        [MixerEvent::Stolen(a), MixerEvent::Ended(a)]
    );
    // Music is never stolen by coins: a lower priority than every playing voice is rejected.
    let quiet = Play {
        priority: 10,
        ..coins
    };
    let c = h.at(1200).play(coin.clone(), &quiet).unwrap();
    render(&mut m, 500, 256);
    assert_eq!(events(&mut h), [MixerEvent::Rejected(c)]);
    assert_eq!(h.voice_status(b).unwrap().state, VoiceState::Playing);
}

#[test]
fn a_2d_position_pans_and_fades_with_distance() {
    let space = Space2d {
        width: 8.0,
        min_distance: 2.0,
        max_distance: 20.0,
        model: Distance::Linear,
    };
    // At the listener: centred, full gain.
    assert_eq!(space.place([0.0, 0.0]), (0.0, 1.0));
    // At the stated width: fully to one side.
    assert_eq!(space.place([8.0, 0.0]).0, 1.0);
    assert_eq!(space.place([-30.0, 0.0]).0, -1.0);
    assert_eq!(space.place([4.0, 0.0]).0, 0.5);
    // Beyond the maximum distance: silent; between: as the model states.
    assert_eq!(space.place([0.0, 20.0]).1, 0.0);
    assert_eq!(space.place([12.0, 16.0]).1, 0.0);
    assert!((space.place([0.0, 11.0]).1 - 0.5).abs() < 1e-6);
    let inverse = Space2d {
        model: Distance::Inverse,
        ..space
    };
    let expected = (2.0 / 5.0 - 0.1) / 0.9;
    assert!((inverse.place([3.0, 4.0]).1 - expected).abs() < 1e-6);
    assert!(
        inverse.place([0.0, 19.999]).1 < 1e-4,
        "inverse reaches 0 at the maximum"
    );

    // In the mixer: a coin to the right sounds to the right, quieter with distance.
    let (mut m, mut h) = Mixer::new(MixerConfig {
        sample_rate: RATE,
        space,
        ..MixerConfig::default()
    })
    .unwrap();
    h.play(
        mono(vec![1.0; 1000], RATE),
        &Play {
            position: Some([8.0, 6.0]),
            ..Play::default()
        },
    )
    .unwrap();
    let out = render(&mut m, 10, 10);
    let gain = space.place([8.0, 6.0]).1 as f64;
    assert!(
        close(out[0], 0.0, 1e-7) && close(out[1], gain, 1e-6),
        "{:?}",
        &out[..2]
    );
    assert!((gain - (1.0 - 8.0 / 18.0)).abs() < 1e-6);
    let (_m, mut h) = mixer(2, &[], 4);
    assert!(
        h.play(
            mono(vec![0.0; 10], RATE),
            &Play {
                position: Some([f32::NAN, 0.0]),
                ..Play::default()
            }
        )
        .is_err()
    );
    assert!(
        Mixer::new(MixerConfig {
            space: Space2d {
                max_distance: 1.0,
                ..space
            },
            ..MixerConfig::default()
        })
        .is_err()
    );
}

#[test]
fn a_moving_sound_changes_smoothly() {
    let space = Space2d {
        width: 10.0,
        min_distance: 0.0,
        max_distance: 20.0,
        model: Distance::Linear,
    };
    let (mut m, mut h) = Mixer::new(MixerConfig {
        sample_rate: RATE,
        space,
        ..MixerConfig::default()
    })
    .unwrap();
    let id = h
        .play(
            mono(vec![1.0; 48_000], RATE),
            &Play {
                position: Some([-10.0, 0.0]),
                ..Play::default()
            },
        )
        .unwrap();
    // A sound crossing from left to right, updated every 800 frames (60 per second)
    // with a ramp of one update.
    for k in 1..=30 {
        let x = -10.0 + 20.0 * k as f32 / 30.0;
        h.at(k * 800)
            .set_position(id, [x, 0.0], Fade::linear(800.0 / RATE as f64))
            .unwrap();
    }
    let out = render(&mut m, 30 * 800 + 1000, 256);
    let (l, r) = (channel(&out, 2, 0), channel(&out, 2, 1));
    let worst = |c: &[f32]| {
        c.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    // Each update moves 2/3 of a unit over 800 frames: per frame, gains change by far
    // less than a zipper step.
    assert!(
        worst(&l) < 2e-4 && worst(&r) < 2e-4,
        "{} {}",
        worst(&l),
        worst(&r)
    );
    // Ten units away of a maximum of twenty: half gain, fully left.
    assert!(
        (l[0] - 0.5).abs() < 1e-6 && r[0] < 1e-6,
        "starts on the left: {} {}",
        l[0],
        r[0]
    );
    let end = 30 * 800 + 900;
    let (pan, gain) = space.place([10.0, 0.0]);
    assert_eq!(pan, 1.0);
    assert!(
        l[end] < 1e-6 && (r[end] as f64 - gain as f64).abs() < 1e-6,
        "ends on the right at its distance gain"
    );
}

fn limited_mixer(limiter: MasterLimiter) -> (Mixer, MixerHandle) {
    Mixer::new(MixerConfig {
        sample_rate: RATE,
        channels: 1,
        limiter: Some(limiter),
        ..MixerConfig::default()
    })
    .unwrap()
}

#[test]
fn the_limiter_holds_a_loud_sum_at_the_ceiling() {
    let ceiling = 10f64.powf(-1.0 / 20.0);
    let tone = mono(sine(48_000, RATE, 220.0, 1.0), RATE);
    for lookahead_ms in [0.0, 5.0] {
        let (mut m, mut h) = limited_mixer(MasterLimiter {
            lookahead_ms,
            ..MasterLimiter::default()
        });
        // Two voices of the same full-scale sine: +6 dBFS without the limiter.
        h.play(tone.clone(), &Play::default()).unwrap();
        h.play(tone.clone(), &Play::default()).unwrap();
        let out = render(&mut m, 48_000, 512);
        let peak = out.iter().fold(0.0f32, |p, s| p.max(s.abs())) as f64;
        assert!(peak <= ceiling + 1e-7, "{lookahead_ms} ms: peak {peak}");
        assert!(
            peak > ceiling * 0.99,
            "the limiter reduces gain, it does not silence"
        );
        // Gain does the work: only a handful of samples sit exactly at the ceiling.
        let held = out
            .iter()
            .filter(|s| (s.abs() as f64 - ceiling).abs() < 1e-9)
            .count();
        assert!(held < 400, "{held} samples at the ceiling");
        assert!(h.limiter_gain().unwrap() < 0.5);
    }
}

#[test]
fn the_limiter_passes_quieter_signals_unchanged() {
    let samples = sine(9600, RATE, 330.0, 0.8);
    let (mut m, mut h) = limited_mixer(MasterLimiter::default());
    h.play(mono(samples.clone(), RATE), &Play::default())
        .unwrap();
    assert_eq!(render(&mut m, 9600, 500), samples);
    assert_eq!((h.latency(), h.limiter_gain()), (0, Some(1.0)));
    // A look-ahead delays the output and changes nothing else.
    let (mut m, mut h) = limited_mixer(MasterLimiter {
        lookahead_ms: 5.0,
        ..MasterLimiter::default()
    });
    h.play(mono(samples.clone(), RATE), &Play::default())
        .unwrap();
    let out = render(&mut m, 9600 + 240, 333);
    assert_eq!(h.latency(), 240);
    assert!(out[..240].iter().all(|&s| s == 0.0));
    assert_eq!(&out[240..], samples.as_slice());
}

#[test]
fn the_limiter_releases_as_stated() {
    // A loud burst of 2.0 for 0.1 s, then a steady 0.5, below the ceiling.
    let mut samples = vec![2.0f32; 4800];
    samples.extend(vec![0.5f32; 24_000]);
    let (mut m, mut h) = limited_mixer(MasterLimiter {
        release_ms: 50.0,
        ..MasterLimiter::default()
    });
    h.play(mono(samples, RATE), &Play::default()).unwrap();
    let out = render(&mut m, 28_800, 1000);
    let ceiling = 10f64.powf(-1.0 / 20.0);
    let floor = ceiling / 2.0;
    assert!((out[4799] as f64 - ceiling).abs() < 1e-6);
    // The gain recovers as 1 - (1 - floor)·exp(-k / 2400) for the k-th quiet frame.
    for k in [1usize, 2400, 4800, 12_000] {
        let gain = 1.0 - (1.0 - floor) * (-(k as f64) / 2400.0).exp();
        let got = out[4799 + k] as f64 / 0.5;
        assert!((got - gain).abs() < 1e-5, "frame {k}: {got} vs {gain}");
    }
}

#[test]
fn limited_renders_do_not_depend_on_buffer_sizes() {
    let run = |chunk: usize| {
        let (mut m, mut h) = limited_mixer(MasterLimiter {
            lookahead_ms: 2.0,
            ..MasterLimiter::default()
        });
        let tone = mono(sine(20_000, RATE, 440.0, 0.9), RATE);
        for i in 0..5 {
            h.at(i * 3000).play(tone.clone(), &Play::default()).unwrap();
        }
        render(&mut m, 30_000, chunk)
    };
    let reference = run(30_000);
    for chunk in [1, 100, 777] {
        assert!(
            run(chunk)
                .iter()
                .zip(&reference)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "{chunk}"
        );
    }
    let bad = MasterLimiter {
        lookahead_ms: 20.0,
        ..MasterLimiter::default()
    };
    assert!(
        Mixer::new(MixerConfig {
            limiter: Some(bad),
            ..MixerConfig::default()
        })
        .is_err()
    );
}

// Regressions found in the 0.3 review.

#[test]
fn stopping_a_voice_on_a_paused_bus_ends_it() {
    let (mut m, mut h) = mixer(1, &["game"], 4);
    let game = h.bus("game").unwrap();
    let id = h
        .play(
            mono(vec![1.0; 48_000], RATE),
            &Play {
                bus: game,
                ..Play::default()
            },
        )
        .unwrap();
    render(&mut m, 100, 100);
    h.pause_bus(game, Fade::NONE).unwrap();
    render(&mut m, 100, 100);
    h.stop(id, Fade::linear(0.5)).unwrap();
    render(&mut m, 100, 100);
    assert_eq!(events(&mut h), [MixerEvent::Ended(id)]);
    h.resume_bus(game, Fade::NONE).unwrap();
    let out = render(&mut m, 1000, 100);
    assert!(
        out.iter().all(|&s| s == 0.0),
        "the stopped sound is not heard on resume"
    );
    // The same through the master bus.
    let id = h
        .play(mono(vec![1.0; 48_000], RATE), &Play::default())
        .unwrap();
    render(&mut m, 10, 10);
    h.pause_bus(BusId::MASTER, Fade::NONE).unwrap();
    h.stop_all(Fade::linear(0.5)).unwrap();
    render(&mut m, 10, 10);
    assert_eq!(events(&mut h), [MixerEvent::Ended(id)]);
}

#[test]
fn a_gap_outlasts_many_other_limited_starts() {
    let coin = mono(vec![0.5; 10], RATE);
    let blip = mono(vec![0.1; 10], RATE);
    let (mut m, mut h) = Mixer::new(MixerConfig {
        sample_rate: RATE,
        queue: 4096,
        ..MixerConfig::default()
    })
    .unwrap();
    let coin_play = Play {
        instances: Some(Instances::of(&coin).min_gap(1.0)),
        ..Play::default()
    };
    let blip_play = Play {
        instances: Some(Instances::of(&blip)),
        ..Play::default()
    };
    h.at(0).play(coin.clone(), &coin_play).unwrap();
    for i in 0..200 {
        h.at(10 + i).play(blip.clone(), &blip_play).unwrap();
    }
    let early = h.at(1000).play(coin.clone(), &coin_play).unwrap();
    render(&mut m, 2000, 256);
    assert!(
        events(&mut h).contains(&MixerEvent::Limited(early)),
        "the coin's gap still holds"
    );
}

#[test]
fn a_rejected_start_does_not_begin_a_gap_or_stop_instances() {
    let music = mono(vec![1.0; 96_000], RATE);
    let coin = mono(vec![0.5; 48_000], RATE);
    let (mut m, mut h) = mixer(1, &[], 1);
    let song = h
        .play(
            music,
            &Play {
                priority: 255,
                ..Play::default()
            },
        )
        .unwrap();
    let limit = Instances::of(&coin).min_gap(0.5);
    let low = Play {
        priority: 10,
        instances: Some(limit),
        ..Play::default()
    };
    let rejected = h.at(100).play(coin.clone(), &low).unwrap();
    render(&mut m, 200, 100);
    assert_eq!(events(&mut h), [MixerEvent::Rejected(rejected)]);
    h.at(300).stop(song, Fade::NONE).unwrap();
    let played = h.at(400).play(coin.clone(), &low).unwrap();
    render(&mut m, 500, 100);
    assert_eq!(events(&mut h), [MixerEvent::Ended(song)]);
    assert_eq!(
        h.voice_status(played).unwrap().state,
        VoiceState::Playing,
        "the earlier refusal began no gap"
    );
}

#[test]
fn every_accepted_rate_and_pitch_combination_plays_at_its_speed() {
    // 10 Hz at 128 times is 1280 Hz; 12.8 kHz at 1/256 is 50 Hz. At 128 times, the
    // 96,000-frame source lasts 750 output frames.
    let cases = [
        (10.0, 8.0, 48.0, 1280.0, 50..700),
        (12_800.0, 0.0625, -48.0, 50.0, 2000..28_000),
    ];
    for (hz, rate, pitch, expected, window) in cases {
        let (mut m, mut h) = mixer(1, &[], 4);
        h.play(
            mono(sine(96_000, RATE, hz, 0.5), RATE),
            &Play {
                rate,
                pitch,
                ..Play::default()
            },
        )
        .unwrap();
        let out = render(&mut m, 30_000, 1000);
        let got = frequency(&out, window.start, window.end);
        assert!(
            (got / expected - 1.0).abs() < 2e-3,
            "rate {rate}, pitch {pitch}: {got} Hz, expected {expected}"
        );
    }
}

#[test]
fn invalid_dimensions_are_errors_with_a_limiter() {
    let limiter = Some(MasterLimiter {
        lookahead_ms: 5.0,
        ..MasterLimiter::default()
    });
    for config in [
        MixerConfig {
            channels: usize::MAX,
            limiter,
            ..MixerConfig::default()
        },
        MixerConfig {
            sample_rate: 0,
            limiter,
            ..MixerConfig::default()
        },
        MixerConfig {
            sample_rate: u32::MAX,
            limiter,
            ..MixerConfig::default()
        },
    ] {
        assert!(Mixer::new(config).is_err());
    }
}

#[test]
fn each_start_is_checked_against_its_own_gap() {
    let coin = mono(vec![0.5; 10], RATE);
    let (mut m, mut h) = mixer(1, &[], 8);
    let short = Play {
        instances: Some(Instances::of(&coin).min_gap(0.01)),
        ..Play::default()
    };
    let long = Play {
        instances: Some(Instances::of(&coin).min_gap(1.0)),
        ..Play::default()
    };
    let unlimited = Play {
        instances: Some(Instances::of(&coin)),
        ..Play::default()
    };
    h.at(0).play(coin.clone(), &short).unwrap();
    // 20 ms later: the first start's 10 ms gap has passed, but this start asks for 1 s.
    let refused = h.at(960).play(coin.clone(), &long).unwrap();
    // A start without a gap plays, and becomes the sound's last start.
    h.at(1000).play(coin.clone(), &unlimited).unwrap();
    let after_unlimited = h.at(1200).play(coin.clone(), &short).unwrap();
    let late = h.at(49_000).play(coin.clone(), &long).unwrap();
    render(&mut m, 50_000, 1000);
    let got = events(&mut h);
    assert!(got.contains(&MixerEvent::Limited(refused)), "{got:?}");
    assert!(
        got.contains(&MixerEvent::Limited(after_unlimited)),
        "{got:?}"
    );
    assert!(!got.contains(&MixerEvent::Limited(late)), "{got:?}");
}

#[test]
fn instance_keys_stay_unique_after_a_sound_is_freed() {
    let (mut m, mut h) = mixer(1, &[], 4);
    let first = mono(vec![0.5; 10], RATE);
    assert_eq!(
        Instances::of(&first),
        Instances::of(&(*first).clone()),
        "clones share a key"
    );
    let twin = mono(vec![0.5; 10], RATE);
    assert_ne!(
        Instances::of(&first),
        Instances::of(&twin),
        "equal content, distinct sounds"
    );
    h.play(
        first.clone(),
        &Play {
            instances: Some(Instances::of(&first).min_gap(1.0)),
            ..Play::default()
        },
    )
    .unwrap();
    render(&mut m, 100, 100);
    drop(first);
    while h.poll_event().is_some() {}
    // A new sound, possibly at the freed address, does not inherit the old gap.
    for _ in 0..20 {
        let next = mono(vec![0.25; 10], RATE);
        let id = h
            .play(
                next.clone(),
                &Play {
                    instances: Some(Instances::of(&next).min_gap(1.0)),
                    ..Play::default()
                },
            )
            .unwrap();
        render(&mut m, 50, 50);
        assert!(!events(&mut h).contains(&MixerEvent::Limited(id)));
    }
}

#[test]
fn a_lower_maximum_stops_enough_instances() {
    let coin = mono(vec![0.25; 48_000], RATE);
    let (mut m, mut h) = mixer(1, &[], 16);
    let key = Instances::of(&coin);
    let ids: Vec<VoiceId> = (0..3)
        .map(|i| {
            h.at(i * 10)
                .play(
                    coin.clone(),
                    &Play {
                        instances: Some(key),
                        ..Play::default()
                    },
                )
                .unwrap()
        })
        .collect();
    let only = Play {
        instances: Some(key.max(1).policy(OverLimit::StopOldest)),
        ..Play::default()
    };
    let newest = h.at(100).play(coin.clone(), &only).unwrap();
    let out = render(&mut m, 1000, 256);
    let got = events(&mut h);
    for id in &ids {
        assert!(
            got.contains(&MixerEvent::Stolen(*id)) && got.contains(&MixerEvent::Ended(*id)),
            "{got:?}"
        );
    }
    assert_eq!(m.active_voices(), 1);
    assert_eq!(h.voice_status(newest).unwrap().state, VoiceState::Playing);
    assert!(close(out[999], 0.25, 1e-6));
}

#[test]
fn positions_wrap_into_the_loop_at_high_speed() {
    let (mut m, mut h) = mixer(1, &[], 4);
    let id = h
        .play(
            mono(counting(1000), RATE),
            &Play {
                looping: Loop::Whole,
                rate: 8.0,
                pitch: 48.0,
                ..Play::default()
            },
        )
        .unwrap();
    render(&mut m, 8, 8);
    assert_eq!(h.voice_status(id).unwrap().position, 24);
    let looping = Loop::Region {
        start: 200,
        end: 700,
    };
    let (mut m, mut h) = mixer(1, &[], 4);
    let id = h
        .play(
            mono(counting(1000), RATE),
            &Play {
                looping,
                rate: 8.0,
                pitch: 48.0,
                ..Play::default()
            },
        )
        .unwrap();
    render(&mut m, 6, 6);
    // 768 source frames: the first pass reaches 700, then 68 more from 200.
    assert_eq!(h.voice_status(id).unwrap().position, 268);
}

#[test]
fn a_bus_pause_holds_an_unfinished_pause_fade() {
    let (mut m, mut h) = mixer(1, &["game"], 4);
    let game = h.bus("game").unwrap();
    let id = h
        .play(
            mono(vec![1.0; 96_000], RATE),
            &Play {
                bus: game,
                ..Play::default()
            },
        )
        .unwrap();
    // A 1000-frame pause fade starts at 1000; the bus pauses halfway through it.
    h.at(1000)
        .pause(id, Fade::linear(1000.0 / RATE as f64))
        .unwrap();
    h.at(1500).pause_bus(game, Fade::NONE).unwrap();
    h.at(5000).resume_bus(game, Fade::NONE).unwrap();
    let out = render(&mut m, 6000, 256);
    assert!(close(out[1499], 0.501, 1e-6));
    assert!(out[1500..5000].iter().all(|&s| s == 0.0));
    // The fade resumes where it was, not finished.
    assert!(close(out[5000], 0.5, 1e-6), "{}", out[5000]);
    assert!(close(out[5250], 0.25, 1e-6));
    assert!(out[5500..].iter().all(|&s| s == 0.0));
}

#[test]
fn panned_stereo_audio_is_a_new_sound() {
    let stereo = Audio::new(vec![vec![0.5; 10], vec![0.5; 10]], RATE).unwrap();
    let panned = stereo.pan(-0.5).unwrap();
    assert_ne!(Instances::of(&stereo), Instances::of(&panned));
    let mono_sound = Audio::new(vec![vec![0.5; 10]], RATE).unwrap();
    assert_ne!(
        Instances::of(&mono_sound),
        Instances::of(&mono_sound.pan(0.5).unwrap())
    );
}

#[test]
fn replacing_many_voices_at_once_loses_no_event() {
    let coin = mono(vec![0.1; 48_000], RATE);
    let (mut m, mut h) = Mixer::new(MixerConfig {
        sample_rate: RATE,
        channels: 1,
        max_voices: 32,
        queue: 1,
        buses: vec!["game".into()],
        ..MixerConfig::default()
    })
    .unwrap();
    let game = h.bus("game").unwrap();
    let key = Instances::of(&coin);
    for _ in 0..32 {
        h.play(
            coin.clone(),
            &Play {
                bus: game,
                instances: Some(key),
                ..Play::default()
            },
        )
        .unwrap();
        render(&mut m, 1, 1);
    }
    h.pause_bus(game, Fade::NONE).unwrap();
    render(&mut m, 1, 1);
    let only = Play {
        bus: game,
        instances: Some(key.max(1).policy(OverLimit::StopOldest)),
        ..Play::default()
    };
    h.play(coin.clone(), &only).unwrap();
    render(&mut m, 1, 1);
    let got = events(&mut h);
    let stolen = got
        .iter()
        .filter(|e| matches!(e, MixerEvent::Stolen(_)))
        .count();
    let ended = got
        .iter()
        .filter(|e| matches!(e, MixerEvent::Ended(_)))
        .count();
    assert_eq!((stolen, ended), (32, 32), "{got:?}");
}

#[test]
fn a_stopping_voice_that_becomes_paused_ends() {
    for by_bus in [false, true] {
        let (mut m, mut h) = mixer(1, &["game"], 4);
        let game = h.bus("game").unwrap();
        let id = h
            .play(
                mono(vec![1.0; 96_000], RATE),
                &Play {
                    bus: game,
                    ..Play::default()
                },
            )
            .unwrap();
        if by_bus {
            h.at(100).pause_bus(game, Fade::linear(0.01)).unwrap();
        } else {
            h.at(100).pause(id, Fade::linear(0.01)).unwrap();
        }
        h.at(200).stop(id, Fade::linear(0.05)).unwrap();
        render(&mut m, 2000, 256);
        assert_eq!(
            events(&mut h),
            [MixerEvent::Ended(id)],
            "paused by bus: {by_bus}"
        );
        if by_bus {
            h.resume_bus(game, Fade::NONE).unwrap();
        } else {
            h.resume(id, Fade::NONE).unwrap();
        }
        assert!(render(&mut m, 1000, 256).iter().all(|&s| s == 0.0));
    }
}

#[test]
fn the_limiter_bounds_overflowing_sums() {
    let (mut m, mut h) = limited_mixer(MasterLimiter::default());
    let tone = mono(sine(4800, RATE, 300.0, 1.0), RATE);
    for _ in 0..2 {
        h.play(
            tone.clone(),
            &Play {
                gain: f32::MAX,
                ..Play::default()
            },
        )
        .unwrap();
    }
    let out = render(&mut m, 4800, 512);
    let ceiling = 10f32.powf(-1.0 / 20.0);
    assert!(
        out.iter()
            .all(|s| s.is_finite() && s.abs() <= ceiling + 1e-6)
    );
}

#[test]
fn pitch_variation_is_not_clipped_at_the_extremes() {
    // 10 Hz at rate 8 and pitch 48 is 128 times faster: 84 semitones up. Variation of
    // 24 semitones may take it as far as 108 semitones, 512 times.
    let mut above = false;
    for seed in 0..6 {
        let (mut m, mut h) = Mixer::new(MixerConfig {
            sample_rate: RATE,
            channels: 1,
            seed,
            ..MixerConfig::default()
        })
        .unwrap();
        let play = Play {
            rate: 8.0,
            pitch: 48.0,
            variation: Variation {
                pitch: 24.0,
                gain_db: 0.0,
            },
            ..Play::default()
        };
        h.play(mono(sine(480_000, RATE, 10.0, 0.5), RATE), &play)
            .unwrap();
        let out = render(&mut m, 900, 900);
        let semitones = 12.0 * (frequency(&out, 20, 880) / 10.0).log2();
        assert!(
            (60.0 - 0.05..=108.0 + 0.05).contains(&semitones),
            "seed {seed}: {semitones}"
        );
        above |= semitones > 84.1;
    }
    assert!(above, "positive draws play above 128 times");
}

/// A looping stream that does not report its length.
struct Counting {
    at: u64,
    len: u64,
}
impl Stream for Counting {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn channels(&self) -> usize {
        1
    }
    fn read(&mut self, out: &mut [f32]) -> audiobox::Result<usize> {
        let n = out.len().min((self.len - self.at) as usize);
        for (i, s) in out[..n].iter_mut().enumerate() {
            *s = (self.at + i as u64) as f32 / 100.0;
        }
        self.at += n as u64;
        Ok(n)
    }
    fn seek(&mut self, frame: u64) -> audiobox::Result<()> {
        self.at = frame.min(self.len);
        Ok(())
    }
}

#[test]
fn positions_wrap_for_loops_of_unknown_length() {
    let (mut m, mut h) = mixer(1, &[], 4);
    let play = Play {
        looping: Loop::Whole,
        rate: 8.0,
        pitch: 48.0,
        ..Play::default()
    };
    let id = h
        .play(Sound::stream(Counting { at: 0, len: 10 }), &play)
        .unwrap();
    render(&mut m, 1, 1);
    assert_eq!(
        h.voice_status(id).unwrap().position,
        8,
        "128 frames into a 10-frame loop"
    );
    render(&mut m, 2, 2);
    assert_eq!(h.voice_status(id).unwrap().position, (3 * 128) % 10);
}

#[test]
fn instance_limits_respect_priorities() {
    let coin = mono(vec![0.5; 48_000], RATE);
    let (mut m, mut h) = mixer(1, &[], 1);
    let limit = Instances::of(&coin).max(1).policy(OverLimit::StopOldest);
    let high = h
        .play(
            coin.clone(),
            &Play {
                priority: 255,
                instances: Some(limit),
                ..Play::default()
            },
        )
        .unwrap();
    let low = h
        .at(100)
        .play(
            coin.clone(),
            &Play {
                priority: 0,
                instances: Some(limit),
                ..Play::default()
            },
        )
        .unwrap();
    render(&mut m, 500, 100);
    assert_eq!(events(&mut h), [MixerEvent::Limited(low)]);
    assert_eq!(h.voice_status(high).unwrap().state, VoiceState::Playing);
}

#[test]
fn a_sound_that_has_ended_leaves_room_for_the_next_instance() {
    let tick = mono(vec![0.5; 100], RATE);
    for block in [500, 100] {
        let (mut m, mut h) = mixer(1, &[], 1);
        let limit = Instances::of(&tick).max(1).policy(OverLimit::Refuse);
        let play = Play {
            instances: Some(limit),
            ..Play::default()
        };
        let first = h.play(tick.clone(), &play).unwrap();
        let second = h.at(100).play(tick.clone(), &play).unwrap();
        let out = render(&mut m, 500, block);
        assert_eq!(
            events(&mut h),
            [MixerEvent::Ended(first), MixerEvent::Ended(second)],
            "blocks of {block}"
        );
        assert!(out[100..200].iter().all(|&s| s != 0.0), "blocks of {block}");
    }
}

#[test]
fn a_stop_overtaken_by_a_pause_ends_the_voice_whatever_the_blocks() {
    let mut outs = Vec::new();
    for block in [1500, 1] {
        let (mut m, mut h) = mixer(2, &[], 1);
        let id = h
            .play(mono(vec![1.0; 48_000], RATE), &Play::default())
            .unwrap();
        h.at(10)
            .pause(id, Fade::linear(100.0 / RATE as f64))
            .unwrap();
        h.at(20)
            .stop(id, Fade::linear(1000.0 / RATE as f64))
            .unwrap();
        h.at(150).resume(id, Fade::NONE).unwrap();
        let out = render(&mut m, 1500, block);
        assert!(
            out[2 * 150..].iter().all(|&s| s == 0.0),
            "blocks of {block}"
        );
        assert_eq!(events(&mut h), [MixerEvent::Ended(id)], "blocks of {block}");
        outs.push(out);
    }
    assert_eq!(outs[0], outs[1]);
}

#[test]
fn a_speed_change_where_a_sound_ends_does_not_depend_on_the_blocks() {
    let mut outs = Vec::new();
    for block in [300, 100] {
        let (mut m, mut h) = mixer(1, &[], 1);
        let id = h
            .play(mono(vec![0.5; 100], RATE), &Play::default())
            .unwrap();
        h.at(100).set_rate(id, 0.5, Fade::NONE).unwrap();
        outs.push(render(&mut m, 300, block));
        assert_eq!(events(&mut h), [MixerEvent::Ended(id)], "blocks of {block}");
    }
    assert_eq!(outs[0], outs[1]);
}

#[test]
fn a_fast_sound_past_its_end_leaves_room_for_the_next_instance() {
    let tick = mono(vec![0.5; 110], RATE);
    let limit = Instances::of(&tick).max(1);
    let (mut m, mut h) = mixer(1, &[], 1);
    let first = h
        .play(
            tick.clone(),
            &Play {
                rate: 8.0,
                pitch: 24.0,
                instances: Some(limit),
                ..Play::default()
            },
        )
        .unwrap();
    let second = h
        .at(4)
        .play(
            tick.clone(),
            &Play {
                instances: Some(limit),
                ..Play::default()
            },
        )
        .unwrap();
    render(&mut m, 500, 500);
    assert_eq!(
        events(&mut h),
        [MixerEvent::Ended(first), MixerEvent::Ended(second)]
    );
}

/// A stream that gives `frames` frames, then fails.
struct Failing {
    frames: usize,
}
impl Stream for Failing {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn channels(&self) -> usize {
        1
    }
    fn read(&mut self, out: &mut [f32]) -> audiobox::Result<usize> {
        if self.frames == 0 {
            return Err(audiobox::Error::new(ErrorCode::DecodeError, "broken"));
        }
        let n = out.len().min(self.frames);
        out[..n].fill(0.25);
        self.frames -= n;
        Ok(n)
    }
    fn seek(&mut self, _frame: u64) -> audiobox::Result<()> {
        Ok(())
    }
}

#[test]
fn a_stream_failing_while_publishing_is_reported_at_once() {
    let (mut m, mut h) = mixer(1, &[], 4);
    let id = h
        .play(Sound::stream(Failing { frames: 256 }), &Play::default())
        .unwrap();
    render(&mut m, 248, 248);
    assert_eq!(
        events(&mut h),
        [MixerEvent::Failed(id, ErrorCode::DecodeError)]
    );
    assert_eq!(h.voice_status(id).unwrap().state, VoiceState::Finished);
    assert_eq!(m.active_voices(), 0);
}
