use audiobox::analyze::{EnvelopeMethod, EnvelopeOptions, EnvelopeSpan, EnvelopeThresholds};
use audiobox::{Audio, ErrorCode};

const RATE: u32 = 48_000;

fn mono(samples: Vec<f32>) -> Audio {
    Audio::new(vec![samples], RATE).unwrap()
}

fn within(span: Option<EnvelopeSpan>, expected_frames: f64, tolerance: usize) {
    let span = span.expect("a measured span");
    let error = (span.frames() as f64 - expected_frames).abs();
    assert!(
        error <= tolerance as f64,
        "{span:?}: {} frames, expected {expected_frames} within {tolerance}",
        span.frames()
    );
}

#[test]
fn a_linear_ramp_has_its_attack_time() {
    // 0 to 1 over 4800 frames, then held: 10 % to 90 % takes 3840 frames.
    let ramp: Vec<f32> = (0..9600).map(|i| (i as f32 / 4800.0).min(1.0)).collect();
    let audio = mono(ramp);
    for method in [EnvelopeMethod::Peak, EnvelopeMethod::Rms] {
        let envelope = audio.envelope(&EnvelopeOptions::new(method, 48)).unwrap();
        within(envelope.attack(0.1, 0.9).unwrap(), 3840.0, 48);
    }
}

#[test]
fn an_exponential_decay_has_its_decay_time() {
    // exp(-t / 0.1 s) falls 20 dB in 0.1 × ln 10 s.
    let tau = 0.1 * RATE as f64;
    let decay: Vec<f32> = (0..48_000)
        .map(|i| (-(i as f64) / tau).exp() as f32)
        .collect();
    let audio = mono(decay);
    let expected = tau * 10f64.ln();
    for method in [EnvelopeMethod::Peak, EnvelopeMethod::Rms] {
        let envelope = audio.envelope(&EnvelopeOptions::new(method, 64)).unwrap();
        let span = envelope.decay(20.0).unwrap();
        assert_eq!(span.unwrap().start_frame, 0);
        within(span, expected, 64);
    }
}

/// A 440 Hz sine under a linear ADSR: 20 ms attack to 1, 100 ms decay to 0.5,
/// sustain until the gate closes at 500 ms, 200 ms release, then 300 ms of silence.
fn adsr_note() -> Audio {
    let ms = |t: f64| t * RATE as f64 / 1000.0;
    let samples = (0..ms(1000.0) as usize)
        .map(|i| {
            let t = i as f64 * 1000.0 / RATE as f64;
            let level = if t < 20.0 {
                t / 20.0
            } else if t < 120.0 {
                1.0 - 0.5 * (t - 20.0) / 100.0
            } else if t < 500.0 {
                0.5
            } else if t < 700.0 {
                0.5 * (1.0 - (t - 500.0) / 200.0)
            } else {
                0.0
            };
            (level * (2.0 * std::f64::consts::PI * 440.0 * t / 1000.0).sin()) as f32
        })
        .collect();
    mono(samples)
}

#[test]
fn an_adsr_note_has_its_attack_decay_and_release() {
    let audio = adsr_note();
    // 10 ms windows: four periods of 440 Hz.
    let envelope = audio
        .envelope(&EnvelopeOptions::new(EnvelopeMethod::Peak, 480))
        .unwrap();
    let times = envelope
        .times(&EnvelopeThresholds {
            decay_db: 3.0,
            gate_off_frame: Some(24_000),
            tail_floor_db: -40.0,
            ..EnvelopeThresholds::default()
        })
        .unwrap();
    // 10 % to 90 % of a 20 ms ramp: 16 ms.
    within(times.attack, 0.016 * RATE as f64, 480);
    // From the peak at 20 ms down 3 dB (0.708) along the decay: 58.4 ms.
    let level = 10f64.powf(-3.0 / 20.0);
    within(times.decay, (1.0 - level) / 0.5 * 0.1 * RATE as f64, 480);
    // From the gate to 1 % of the peak: 0.5 × (1 − x / 200 ms) = 0.01 at 196 ms.
    let tail = times.tail.unwrap();
    assert_eq!(tail.start_frame, 24_000);
    within(Some(tail), 0.196 * RATE as f64, 480);
    assert_eq!(tail.seconds(RATE), tail.frames() as f64 / RATE as f64);
    assert!((times.peak - 1.0).abs() < 1e-2, "{}", times.peak);
}

#[test]
fn envelope_times_are_the_same_on_every_run() {
    // Integers in frames are the same on every host; these are pinned.
    let audio = adsr_note();
    let measure = || {
        let envelope = audio
            .envelope(&EnvelopeOptions::new(EnvelopeMethod::Rms, 240))
            .unwrap();
        let times = envelope
            .times(&EnvelopeThresholds {
                gate_off_frame: Some(24_000),
                ..EnvelopeThresholds::default()
            })
            .unwrap();
        (envelope, times)
    };
    let (a, times) = measure();
    let (b, again) = measure();
    assert_eq!(a, b);
    assert_eq!(times, again);
    let frames = |s: Option<EnvelopeSpan>| s.map(|s| (s.start_frame, s.end_frame));
    assert_eq!(
        (
            frames(times.attack),
            frames(times.decay),
            frames(times.tail)
        ),
        (Some((0, 720)), Some((960, 31_680)), Some((24_000, 33_600))),
        "{times:?}"
    );
}

#[test]
fn quiet_or_unfinished_input_measures_nothing() {
    let silent = mono(vec![0.0; 1000]);
    let envelope = silent
        .envelope(&EnvelopeOptions::new(EnvelopeMethod::Peak, 10))
        .unwrap();
    let times = envelope
        .times(&EnvelopeThresholds {
            gate_off_frame: Some(500),
            ..EnvelopeThresholds::default()
        })
        .unwrap();
    assert_eq!((times.peak, times.peak_frame), (0.0, None));
    assert_eq!((times.attack, times.decay, times.tail), (None, None, None));

    // A held tone never decays, and it still rings at the end.
    let held = mono(vec![0.5; 1000]);
    let envelope = held
        .envelope(&EnvelopeOptions::new(EnvelopeMethod::Peak, 10))
        .unwrap();
    assert_eq!(envelope.decay(1.0).unwrap(), None);
    assert_eq!(envelope.tail(500, -60.0).unwrap(), None);
    assert_eq!(
        envelope.attack(0.1, 0.9).unwrap(),
        Some(EnvelopeSpan {
            start_frame: 0,
            end_frame: 0
        })
    );

    // Already quiet at the gate: an empty tail.
    let mut short = vec![0.5; 200];
    short.extend(vec![0.0; 800]);
    let envelope = mono(short)
        .envelope(&EnvelopeOptions::new(EnvelopeMethod::Peak, 10))
        .unwrap();
    assert_eq!(
        envelope.tail(500, -60.0).unwrap(),
        Some(EnvelopeSpan {
            start_frame: 500,
            end_frame: 500
        })
    );
    // A gate inside the last loud window: the tail ends with that window.
    assert_eq!(
        envelope.tail(195, -60.0).unwrap(),
        Some(EnvelopeSpan {
            start_frame: 195,
            end_frame: 200
        })
    );

    let empty = Audio::new(vec![Vec::new()], RATE).unwrap();
    let envelope = empty
        .envelope(&EnvelopeOptions::new(EnvelopeMethod::Rms, 10))
        .unwrap();
    assert!(envelope.values.is_empty());
    assert_eq!(envelope.attack(0.1, 0.9).unwrap(), None);
}

#[test]
fn channels_are_measured_alone_or_together() {
    let audio = Audio::new(vec![vec![0.0; 100], vec![0.5; 100]], RATE).unwrap();
    let peak = |channel| {
        audio
            .envelope(&EnvelopeOptions {
                method: EnvelopeMethod::Peak,
                window_frames: 50,
                channel,
            })
            .unwrap()
            .values
    };
    assert_eq!(peak(None), [0.5, 0.5]);
    assert_eq!(peak(Some(0)), [0.0, 0.0]);
    assert_eq!(peak(Some(1)), [0.5, 0.5]);
    // RMS over both channels: sqrt(0.25 / 2).
    let rms = audio
        .envelope(&EnvelopeOptions::new(EnvelopeMethod::Rms, 30))
        .unwrap();
    assert_eq!(rms.values.len(), 4);
    for v in rms.values {
        assert_eq!(v, (0.125f64).sqrt());
    }
}

#[test]
fn bad_arguments_are_refused() {
    let audio = mono(vec![0.5; 100]);
    let invalid = |r: audiobox::Result<_>| {
        let error: audiobox::Error = r.map(|_: ()| ()).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{error}");
    };
    let options = EnvelopeOptions::new(EnvelopeMethod::Peak, 10);
    invalid(
        audio
            .envelope(&EnvelopeOptions {
                window_frames: 0,
                ..options
            })
            .map(drop),
    );
    invalid(
        audio
            .envelope(&EnvelopeOptions {
                channel: Some(1),
                ..options
            })
            .map(drop),
    );
    let envelope = audio.envelope(&options).unwrap();
    for (from, to) in [
        (0.0, 0.9),
        (0.5, 0.5),
        (0.9, 0.1),
        (0.1, 1.5),
        (f64::NAN, 0.9),
    ] {
        invalid(envelope.attack(from, to).map(drop));
    }
    for drop_db in [0.0, -3.0, f64::INFINITY, f64::NAN] {
        invalid(envelope.decay(drop_db).map(drop));
    }
    invalid(envelope.tail(101, -60.0).map(drop));
    for floor in [0.0, 6.0, f64::NEG_INFINITY, f64::NAN] {
        invalid(envelope.tail(50, floor).map(drop));
    }
    invalid(
        envelope
            .times(&EnvelopeThresholds {
                tail_floor_db: 0.0,
                ..EnvelopeThresholds::default()
            })
            .map(drop),
    );
}

#[test]
fn levels_exactly_at_a_threshold_reach_it() {
    // 0.625 to 0.0625 is exactly 20 dB, which dB conversion rounds.
    let mut samples = vec![0.625; 10];
    samples.extend(vec![0.0625; 30]);
    let envelope = mono(samples)
        .envelope(&EnvelopeOptions::new(EnvelopeMethod::Peak, 10))
        .unwrap();
    assert_eq!(
        envelope.decay(20.0).unwrap(),
        Some(EnvelopeSpan {
            start_frame: 0,
            end_frame: 10
        })
    );
    // 0.78125 to 0.0078125 is exactly 40 dB: the tail ends where it reaches it.
    let mut samples = vec![0.78125; 10];
    samples.extend(vec![0.0078125; 30]);
    let envelope = mono(samples)
        .envelope(&EnvelopeOptions::new(EnvelopeMethod::Peak, 10))
        .unwrap();
    assert_eq!(
        envelope.tail(5, -40.0).unwrap(),
        Some(EnvelopeSpan {
            start_frame: 5,
            end_frame: 10
        })
    );
    // 0.5 is exactly half of 1.
    let envelope = mono(vec![0.5, 1.0])
        .envelope(&EnvelopeOptions::new(EnvelopeMethod::Peak, 1))
        .unwrap();
    assert_eq!(
        envelope.attack(0.5, 1.0).unwrap(),
        Some(EnvelopeSpan {
            start_frame: 0,
            end_frame: 1
        })
    );
}

#[test]
fn a_gate_at_the_end_has_an_empty_tail() {
    // Loud to the end, in a last window that is shorter than the others.
    for frames in [100, 105] {
        let envelope = mono(vec![0.5; frames])
            .envelope(&EnvelopeOptions::new(EnvelopeMethod::Peak, 10))
            .unwrap();
        assert_eq!(
            envelope.tail(frames, -60.0).unwrap(),
            Some(EnvelopeSpan {
                start_frame: frames,
                end_frame: frames
            })
        );
        assert_eq!(envelope.tail(frames - 1, -60.0).unwrap(), None);
    }
}

#[test]
fn envelope_values_are_pinned() {
    // The bits of every value, hashed: the same on every host. The input is built
    // with arithmetic alone (a triangle wave under a linear envelope), so it is too.
    let audio = mono(
        (0..48_000)
            .map(|i| {
                let level = (i as f64 / 4800.0).min(1.0) * (1.0 - i as f64 / 48_000.0);
                let phase = (i % 100) as f64 / 100.0;
                let triangle = 1.0 - 4.0 * (phase - 0.5).abs();
                (level * triangle) as f32
            })
            .collect(),
    );
    let hash = |method| {
        let envelope = audio.envelope(&EnvelopeOptions::new(method, 240)).unwrap();
        envelope
            .values
            .iter()
            .fold(0xcbf2_9ce4_8422_2325u64, |h, v| {
                (h ^ v.to_bits()).wrapping_mul(0x0000_0100_0000_01b3)
            })
    };
    let hashes = [hash(EnvelopeMethod::Peak), hash(EnvelopeMethod::Rms)];
    assert_eq!(
        hashes,
        [0xce1d_783e_f725_c8c5, 0x5dfe_1ab8_b928_2302],
        "{hashes:x?}"
    );
}
