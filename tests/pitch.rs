use audiobox::analyze::{PitchOptions, PitchState, PitchTrack};
use audiobox::{Audio, ErrorCode};
use std::f64::consts::PI;

const RATE: u32 = 48_000;

fn mono(samples: Vec<f32>) -> Audio {
    Audio::new(vec![samples], RATE).unwrap()
}

/// A tone whose frequency at time `t` (s) is `pitch(t)`, with harmonics
/// `(number, amplitude)`, `seconds` long. The phase follows the frequency.
fn tone(seconds: f64, harmonics: &[(u32, f64)], pitch: impl Fn(f64) -> f64) -> Vec<f32> {
    let mut phase = 0.0;
    (0..(seconds * RATE as f64) as usize)
        .map(|i| {
            let t = i as f64 / RATE as f64;
            let value: f64 = harmonics
                .iter()
                .map(|&(k, a)| a * (k as f64 * phase).sin())
                .sum();
            phase += 2.0 * PI * pitch(t) / RATE as f64;
            value as f32
        })
        .collect()
}

fn track(samples: Vec<f32>, options: PitchOptions) -> PitchTrack {
    mono(samples).pitch_track(&options).unwrap()
}

fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

/// The time of a frame's middle, in seconds.
fn middle(track: &PitchTrack, i: usize) -> f64 {
    (track.frames[i].start_frame + track.span_frames / 2) as f64 / RATE as f64
}

#[test]
fn isolated_tones_have_their_pitch() {
    for f in [82.41, 110.0, 220.0, 440.0, 880.0, 1760.0] {
        let t = track(
            tone(0.5, &[(1, 0.5)], |_| f),
            PitchOptions::new(60.0, 2000.0, 480),
        );
        for frame in &t.frames {
            assert_eq!(frame.state, PitchState::Voiced, "{f} Hz: {frame:?}");
            let p = frame.frequency_hz.unwrap();
            assert!(cents(p, f).abs() < 5.0, "{f} Hz: {p}");
            assert!(frame.confidence > 0.95, "{f} Hz: {frame:?}");
        }
    }
}

#[test]
fn harmonic_rich_notes_have_their_fundamental() {
    // A band-limited sawtooth and a reed-like tone with odd harmonics.
    let saw: Vec<(u32, f64)> = (1..=30).map(|k| (k, 0.5 / k as f64)).collect();
    let odd: Vec<(u32, f64)> = (0..10)
        .map(|k| (2 * k + 1, 0.5 / (2 * k + 1) as f64))
        .collect();
    for (harmonics, f) in [(&saw, 147.0), (&saw, 523.25), (&odd, 196.0)] {
        let t = track(
            tone(0.5, harmonics, |_| f),
            PitchOptions::new(60.0, 2000.0, 480),
        );
        let p = t.median_frequency(0..t.input_frames).unwrap().unwrap();
        assert!(cents(p, f).abs() < 5.0, "{f} Hz: {p}");
        assert!(t.frames.iter().all(|f| f.state == PitchState::Voiced));
    }
}

#[test]
fn slides_and_vibrato_are_followed() {
    // A glide from 220 to 440 Hz in one second, even in cents.
    let glide = |t: f64| 220.0 * 2f64.powf(t);
    let t = track(
        tone(1.0, &[(1, 0.5), (2, 0.2)], glide),
        PitchOptions::new(100.0, 1000.0, 480),
    );
    for i in 0..t.frames.len() {
        let p = t.frames[i].frequency_hz.unwrap();
        let expected = glide(middle(&t, i));
        assert!(cents(p, expected).abs() < 15.0, "{i}: {p} vs {expected}");
    }
    // 440 Hz with ±50 cents of vibrato at 5 Hz.
    let vibrato = |t: f64| 440.0 * 2f64.powf(50.0 / 1200.0 * (2.0 * PI * 5.0 * t).sin());
    let t = track(
        tone(1.0, &[(1, 0.5)], vibrato),
        PitchOptions::new(100.0, 1000.0, 240),
    );
    let mut deviation = (0.0f64, 0.0f64);
    for i in 0..t.frames.len() {
        let p = t.frames[i].frequency_hz.unwrap();
        let expected = vibrato(middle(&t, i));
        assert!(cents(p, expected).abs() < 15.0, "{i}: {p} vs {expected}");
        let c = cents(p, 440.0);
        deviation = (deviation.0.min(c), deviation.1.max(c));
    }
    assert!(deviation.0 < -40.0 && deviation.1 > 40.0, "{deviation:?}");
}

#[test]
fn noise_and_rests_have_no_pitch() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let noise: Vec<f32> = (0..24_000)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ((seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5) as f32
        })
        .collect();
    let t = track(noise, PitchOptions::new(60.0, 2000.0, 480));
    let voiced = t
        .frames
        .iter()
        .filter(|f| f.state == PitchState::Voiced)
        .count();
    assert!(
        voiced * 20 <= t.frames.len(),
        "{voiced} of {}",
        t.frames.len()
    );
    for frame in &t.frames {
        assert_eq!(
            frame.frequency_hz.is_some(),
            frame.state == PitchState::Voiced
        );
    }

    // A note, a rest, a note.
    let mut samples = tone(0.3, &[(1, 0.5)], |_| 330.0);
    samples.extend(vec![0.0; 14_400]);
    samples.extend(tone(0.3, &[(1, 0.5)], |_| 330.0));
    let t = track(samples, PitchOptions::new(60.0, 2000.0, 480));
    let rest = 14_400..28_800;
    let mut silent = 0;
    for frame in &t.frames {
        // Frames whose first period lies in the rest.
        let heard = frame.start_frame..frame.start_frame + t.span_frames / 2;
        if rest.contains(&heard.start) && rest.contains(&(heard.end - 1)) {
            assert_eq!(frame.state, PitchState::Silent, "{frame:?}");
            assert_eq!((frame.frequency_hz, frame.confidence), (None, 0.0));
            assert_eq!(frame.alternative, None);
            silent += 1;
        }
    }
    assert!(silent > 20, "{silent}");
}

#[test]
fn a_change_of_register_is_followed() {
    let mut samples = tone(0.5, &[(1, 0.5), (2, 0.2)], |_| 110.0);
    samples.extend(tone(0.5, &[(1, 0.5), (2, 0.2)], |_| 880.0));
    let t = track(samples, PitchOptions::new(60.0, 2000.0, 480));
    for frame in &t.frames {
        let end = frame.start_frame + t.span_frames;
        let expected = if end <= 24_000 {
            110.0
        } else if frame.start_frame >= 24_000 {
            880.0
        } else {
            // Frames that straddle the change.
            continue;
        };
        let p = frame.frequency_hz.unwrap();
        assert!(cents(p, expected).abs() < 5.0, "{frame:?}");
    }
}

#[test]
fn chords_and_octaves_show_in_confidence_and_alternatives() {
    let options = PitchOptions::new(60.0, 2000.0, 480);
    // A fifth (220 + 330 Hz) repeats at 110 Hz: that is the period found.
    let fifth = track(tone(0.5, &[(2, 0.4), (3, 0.4)], |_| 110.0), options);
    let p = fifth
        .median_frequency(0..fifth.input_frames)
        .unwrap()
        .unwrap();
    assert!(cents(p, 110.0).abs() < 5.0, "{p}");
    // Two tones a tritone apart, an irrational ratio: no common period.
    let mut a = tone(0.5, &[(1, 0.4)], |_| 220.0);
    for (s, b) in a
        .iter_mut()
        .zip(tone(0.5, &[(1, 0.4)], |_| 220.0 * 2f64.sqrt()))
    {
        *s += b;
    }
    let tritone = track(a, options);
    let pure = track(tone(0.5, &[(1, 0.4)], |_| 220.0), options);
    let mean =
        |t: &PitchTrack| t.frames.iter().map(|f| f.confidence).sum::<f64>() / t.frames.len() as f64;
    assert!(
        mean(&tritone) < mean(&pure) - 0.05,
        "{} vs {}",
        mean(&tritone),
        mean(&pure)
    );
    // A weak fundamental under a strong octave: the other octave is the alternative.
    let octave = track(tone(0.5, &[(1, 0.05), (2, 0.5)], |_| 220.0), options);
    for frame in &octave.frames {
        let p = frame.frequency_hz.unwrap();
        let other = frame.alternative.unwrap().frequency_hz;
        let pair = if p < other { (p, other) } else { (other, p) };
        assert!(
            cents(pair.0, 220.0).abs() < 5.0 && cents(pair.1, 440.0).abs() < 5.0,
            "{frame:?}"
        );
    }
}

#[test]
fn median_smoothing_removes_a_lone_wrong_frame() {
    // A 3 ms burst of 1 kHz inside a 220 Hz note.
    let mut samples = tone(0.4, &[(1, 0.5)], |_| 220.0);
    let burst = tone(0.003, &[(1, 0.5)], |_| 1000.0);
    samples[9600..9600 + burst.len()].copy_from_slice(&burst);
    let options = PitchOptions {
        median: 5,
        ..PitchOptions::new(100.0, 2000.0, 480)
    };
    let smooth = track(samples.clone(), options);
    let raw = track(
        samples,
        PitchOptions {
            median: 1,
            ..options
        },
    );
    let worst = |t: &PitchTrack| {
        t.frames
            .iter()
            .filter_map(|f| f.frequency_hz)
            .map(|p| cents(p, 220.0).abs())
            .fold(0.0, f64::max)
    };
    assert!(worst(&smooth) <= worst(&raw));
    assert!(worst(&smooth) < 20.0, "{}", worst(&smooth));
    assert_eq!(smooth.options.median, 5);
}

#[test]
fn short_empty_and_repeated_inputs() {
    let options = PitchOptions::new(100.0, 1000.0, 100);
    let empty = Audio::new(vec![Vec::new()], RATE).unwrap();
    let t = empty.pitch_track(&options).unwrap();
    assert!(t.frames.is_empty());
    assert_eq!(t.median_frequency(0..0).unwrap(), None);
    // Shorter than a frame (two periods of 100 Hz and the margin, 962 frames):
    // one padded frame.
    let t = track(tone(0.005, &[(1, 0.5)], |_| 440.0), options);
    assert_eq!(t.frames.len(), 1);
    assert_eq!(t.span_frames, 962);
    // Whole frames only: none starts past the last whole one.
    let t = track(tone(0.1, &[(1, 0.5)], |_| 440.0), options);
    assert_eq!(t.frames.len(), (4800 - 962) / 100 + 1);
    assert!(t.frames.iter().all(|f| f.start_frame + 962 <= 4800));
    // The same input, the same track.
    let samples = tone(0.2, &[(1, 0.5), (3, 0.1)], |_| 300.0);
    assert_eq!(track(samples.clone(), options), track(samples, options));
}

#[test]
fn bad_arguments_are_refused() {
    let audio = mono(vec![0.5; 2000]);
    let options = PitchOptions::new(100.0, 1000.0, 100);
    let code = |o: PitchOptions| audio.pitch_track(&o).unwrap_err().code;
    for bad in [
        PitchOptions {
            min_hz: 0.0,
            ..options
        },
        PitchOptions {
            min_hz: f64::NAN,
            ..options
        },
        PitchOptions {
            min_hz: 5.0,
            ..options
        },
        PitchOptions {
            max_hz: 100.0,
            ..options
        },
        PitchOptions {
            max_hz: 6_000.1,
            ..options
        },
        PitchOptions { hop: 0, ..options },
        PitchOptions {
            threshold: 0.0,
            ..options
        },
        PitchOptions {
            threshold: 1.0,
            ..options
        },
        PitchOptions {
            silence_db: f64::NEG_INFINITY,
            ..options
        },
        PitchOptions {
            channel: Some(1),
            ..options
        },
        PitchOptions {
            median: 2,
            ..options
        },
        PitchOptions {
            median: 17,
            ..options
        },
    ] {
        assert_eq!(code(bad), ErrorCode::InvalidArgument, "{bad:?}");
    }
    let t = audio.pitch_track(&options).unwrap();
    assert_eq!(
        t.median_frequency(0..2001).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    // One analysis frame more than the cap, which is checked before the work.
    let long = Audio::new(vec![vec![0.0; (1 << 20) + 962]], RATE).unwrap();
    let error = long
        .pitch_track(&PitchOptions::new(100.0, 1000.0, 1))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::LimitExceeded);
    assert_eq!(error.message, "Too many pitch analysis frames");
    // The work limit, 2^35, at its edge: at 20 Hz a frame spans 4807 frames,
    // integrates over 2399 and transforms 8192, so 44,512 frames ×
    // (8192 × (13 + 1) + 3 × (2^16 + 64 × 2399)) fit and 44,513 do not. The input
    // is silent, so nothing is transformed.
    for (frames, fits) in [(44_512, true), (44_513, false)] {
        let long = Audio::new(vec![vec![0.0; frames + 4806]], RATE).unwrap();
        let result = long.pitch_track(&PitchOptions::new(20.0, 1000.0, 1));
        match fits {
            true => assert_eq!(result.unwrap().frames.len(), frames),
            false => assert_eq!(result.unwrap_err().code, ErrorCode::LimitExceeded),
        }
    }
    // A shorter frame: 100 Hz to 2 kHz spans 962 frames, integrates over 479 and
    // transforms 1024, so 114,593 frames × (1024 × (10 + 1) + 3 × (2^16 + 64 ×
    // 479)) fit and 114,594 do not.
    for (frames, fits) in [(114_593, true), (114_594, false)] {
        let long = Audio::new(vec![vec![0.0; frames + 961]], RATE).unwrap();
        let result = long.pitch_track(&PitchOptions::new(100.0, 2000.0, 1));
        match fits {
            true => assert_eq!(result.unwrap().frames.len(), frames),
            false => assert_eq!(result.unwrap_err().code, ErrorCode::LimitExceeded),
        }
    }
}

/// The evaluation recorded in `docs/sound-measurements.md`: the largest error in
/// cents and the voiced share per fixture, all generated here and free to share.
#[test]
fn evaluation_on_generated_fixtures() {
    let options = PitchOptions::new(60.0, 2000.0, 480);
    let saw: Vec<(u32, f64)> = (1..=30).map(|k| (k, 0.5 / k as f64)).collect();
    let cases: Vec<(&str, Vec<f32>, f64)> = vec![
        ("sine 82.41 Hz", tone(1.0, &[(1, 0.5)], |_| 82.41), 82.41),
        ("sine 440 Hz", tone(1.0, &[(1, 0.5)], |_| 440.0), 440.0),
        ("sine 1760 Hz", tone(1.0, &[(1, 0.5)], |_| 1760.0), 1760.0),
        ("sawtooth 147 Hz", tone(1.0, &saw, |_| 147.0), 147.0),
        ("sawtooth 523.25 Hz", tone(1.0, &saw, |_| 523.25), 523.25),
    ];
    for (name, samples, f) in cases {
        let t = track(samples, options);
        let voiced = t
            .frames
            .iter()
            .filter(|f| f.state == PitchState::Voiced)
            .count();
        let worst = t
            .frames
            .iter()
            .filter_map(|f| f.frequency_hz)
            .map(|p| cents(p, f).abs())
            .fold(0.0, f64::max);
        eprintln!(
            "{name}: {voiced}/{} voiced, worst {worst:.3} cents",
            t.frames.len()
        );
        assert_eq!(voiced, t.frames.len(), "{name}");
        assert!(worst < 5.0, "{name}: {worst}");
    }
}

#[test]
fn pitches_stay_within_the_range() {
    let options = PitchOptions::new(60.0, 2000.0, 480);
    // Just outside the range: no pitch, rather than the nearest one in it.
    for f in [55.0, 2050.0] {
        let t = track(tone(0.5, &[(1, 0.5)], |_| f), options);
        for frame in &t.frames {
            assert_ne!(frame.state, PitchState::Voiced, "{f} Hz: {frame:?}");
            assert_eq!(frame.frequency_hz, None);
        }
    }
    // Just inside it: the pitch.
    for f in [62.0, 1950.0] {
        let t = track(tone(0.5, &[(1, 0.5)], |_| f), options);
        let p = t.median_frequency(0..t.input_frames).unwrap().unwrap();
        assert!(cents(p, f).abs() < 5.0, "{f} Hz: {p}");
    }
}

#[test]
fn a_constant_level_is_silence() {
    // A level stuck at one value, or a DC offset alone, has no period.
    for level in [0.5f32, -0.01] {
        let t = track(vec![level; 9600], PitchOptions::new(60.0, 2000.0, 480));
        for frame in &t.frames {
            assert_eq!(frame.state, PitchState::Silent, "{frame:?}");
        }
    }
    // A tone on a DC offset keeps its pitch.
    let shifted: Vec<f32> = tone(0.3, &[(1, 0.3)], |_| 330.0)
        .iter()
        .map(|s| s + 0.4)
        .collect();
    let t = track(shifted, PitchOptions::new(60.0, 2000.0, 480));
    let p = t.median_frequency(0..t.input_frames).unwrap().unwrap();
    assert!(cents(p, 330.0).abs() < 5.0, "{p}");
}

#[test]
fn alternatives_appear_only_for_voiced_frames_with_a_real_rival() {
    let options = PitchOptions::new(60.0, 2000.0, 480);
    // A clean sine repeats every two periods too: that is not a rival.
    let t = track(tone(0.5, &[(1, 0.5)], |_| 220.0), options);
    for frame in &t.frames {
        if let Some(alternative) = frame.alternative {
            assert!(
                (alternative.frequency_hz / 110.0 - 1.0).abs() > 0.03,
                "{frame:?}"
            );
        }
    }
    // Noise: no pitch and no alternative.
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let noise: Vec<f32> = (0..24_000)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ((seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5) as f32
        })
        .collect();
    for frame in &track(noise, options).frames {
        if frame.state != PitchState::Voiced {
            assert_eq!(frame.alternative, None, "{frame:?}");
        }
    }
}

#[test]
fn pitches_at_the_ends_of_the_range_are_found() {
    // The range's own ends: the dips sit on its first and last lags.
    let options = PitchOptions::new(60.0, 2000.0, 480);
    for f in [60.0, 60.05, 1990.0, 1999.0] {
        let t = track(tone(0.5, &[(1, 0.5)], |_| f), options);
        for frame in &t.frames {
            assert_eq!(frame.state, PitchState::Voiced, "{f} Hz: {frame:?}");
            assert!(
                cents(frame.frequency_hz.unwrap(), f).abs() < 5.0,
                "{f} Hz: {frame:?}"
            );
        }
    }
}

#[test]
fn a_period_above_the_range_gives_no_pitch() {
    // YIN takes the first dip below the threshold. When its period is above the
    // range, the frame has no pitch rather than one of the period's multiples:
    // a sine above the range repeats at twice and four times its period too.
    let options = PitchOptions::new(60.0, 430.0, 480);
    for f in [500.0, 1000.0] {
        let t = track(tone(0.5, &[(1, 0.5)], |_| f), options);
        for frame in &t.frames {
            assert_ne!(frame.state, PitchState::Voiced, "{f} Hz: {frame:?}");
        }
    }
    let t = track(
        tone(0.3, &[(1, 0.5)], |_| 5050.0),
        PitchOptions::new(60.0, 2000.0, 480),
    );
    for frame in &t.frames {
        assert_ne!(frame.state, PitchState::Voiced, "{frame:?}");
    }
    // A weak fundamental in the range under a strong harmonic above it is a
    // limit of the method: no pitch. A ceiling above the harmonic gives it, with
    // the fundamental as the alternative.
    let weak = tone(0.5, &[(1, 0.05), (2, 0.5)], |_| 220.0);
    for frame in &track(weak.clone(), options).frames {
        assert_ne!(frame.state, PitchState::Voiced, "{frame:?}");
    }
    for frame in &track(weak, PitchOptions::new(60.0, 1000.0, 480)).frames {
        let other = frame.alternative.unwrap().frequency_hz;
        assert!(
            cents(frame.frequency_hz.unwrap(), 440.0).abs() < 5.0,
            "{frame:?}"
        );
        assert!(cents(other, 220.0).abs() < 5.0, "{frame:?}");
    }
    // A tone just below the range: no pitch, and no confidence from the slope
    // into the end of the range.
    let t = track(
        tone(0.5, &[(1, 0.5)], |_| 59.0),
        PitchOptions::new(60.0, 2000.0, 480),
    );
    for frame in &t.frames {
        assert_ne!(frame.state, PitchState::Voiced, "{frame:?}");
        assert!(frame.confidence < 0.5, "{frame:?}");
    }
}

#[test]
fn alternatives_stay_within_the_range() {
    // A strong octave above the range: it is not offered as the alternative.
    let options = PitchOptions::new(60.0, 300.0, 480);
    let t = track(tone(0.5, &[(1, 0.1), (2, 0.5)], |_| 220.0), options);
    for frame in &t.frames {
        if let Some(alternative) = frame.alternative {
            let f = alternative.frequency_hz;
            assert!((60.0..=300.0).contains(&f), "{frame:?}");
        }
    }
}

#[test]
fn high_clean_tones_have_no_rival() {
    // Short periods, a few samples long: the depths between samples decide.
    let options = PitchOptions::new(100.0, 6000.0, 480);
    for f in [3000.0, 3840.0] {
        let t = track(tone(0.3, &[(1, 0.5)], |_| f), options);
        for frame in &t.frames {
            assert_eq!(frame.state, PitchState::Voiced, "{f} Hz: {frame:?}");
            assert!(
                cents(frame.frequency_hz.unwrap(), f).abs() < 5.0,
                "{f} Hz: {frame:?}"
            );
            if let Some(alternative) = frame.alternative {
                assert!(
                    alternative.confidence < frame.confidence - 0.01,
                    "{f} Hz: {frame:?}"
                );
            }
        }
    }
}

#[test]
fn high_tones_at_the_top_of_the_range_and_strict_thresholds() {
    // A 12-sample period at the top of the range.
    let t = track(
        tone(0.3, &[(1, 0.5)], |_| 4000.0),
        PitchOptions::new(60.0, 4000.0, 480),
    );
    for frame in &t.frames {
        assert_eq!(frame.state, PitchState::Voiced, "{frame:?}");
        assert!(
            cents(frame.frequency_hz.unwrap(), 4000.0).abs() < 5.0,
            "{frame:?}"
        );
    }
    // A strict threshold is applied to the refined depth: still the fundamental.
    let strict = PitchOptions {
        threshold: 0.02,
        ..PitchOptions::new(100.0, 6000.0, 480)
    };
    for frame in &track(tone(0.3, &[(1, 0.5)], |_| 3840.0), strict).frames {
        assert_eq!(frame.state, PitchState::Voiced, "{frame:?}");
        assert!(
            cents(frame.frequency_hz.unwrap(), 3840.0).abs() < 5.0,
            "{frame:?}"
        );
    }
    // A clean 5050 Hz sine, a 9.5-sample period: its repeats are no rival.
    let t = track(
        tone(0.3, &[(1, 0.5)], |_| 5050.0),
        PitchOptions::new(100.0, 6000.0, 480),
    );
    for frame in &t.frames {
        assert_eq!(frame.state, PitchState::Voiced, "{frame:?}");
        assert!(
            cents(frame.frequency_hz.unwrap(), 5050.0).abs() < 5.0,
            "{frame:?}"
        );
        assert_eq!(frame.alternative, None, "{frame:?}");
    }
}

#[test]
fn short_periods_and_the_margins_of_the_range() {
    let sine = |f: f64, rate: u32, seconds: f64| -> Audio {
        let samples = (0..(seconds * rate as f64) as usize)
            .map(|i| (0.5 * (2.0 * PI * f * i as f64 / rate as f64).sin()) as f32)
            .collect();
        Audio::new(vec![samples], rate).unwrap()
    };
    // The ceiling is at most an 8-sample period: 2 kHz at 16 kHz.
    let low_rate = sine(1998.0, 16_000, 0.3);
    assert_eq!(
        low_rate
            .pitch_track(&PitchOptions::new(60.0, 2500.0, 160))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    let t = low_rate
        .pitch_track(&PitchOptions::new(60.0, 2000.0, 160))
        .unwrap();
    for frame in &t.frames {
        assert_eq!(frame.state, PitchState::Voiced, "{frame:?}");
        assert!(
            cents(frame.frequency_hz.unwrap(), 1998.0).abs() < 5.0,
            "{frame:?}"
        );
    }
    // Tones above the range, of a few samples per period, whose first dip
    // refines too shallow: still no pitch, rather than a repeat.
    for (f, rate, max_hz) in [(4500.0, 16_000, 2000.0), (7700.0, 48_000, 6000.0)] {
        let t = sine(f, rate, 0.3)
            .pitch_track(&PitchOptions::new(100.0, max_hz, 480))
            .unwrap();
        for frame in &t.frames {
            assert_ne!(frame.state, PitchState::Voiced, "{f} Hz: {frame:?}");
        }
    }
    // Within the margin below the floor: set to it.
    let t = track(
        tone(0.5, &[(1, 0.5)], |_| 59.9),
        PitchOptions::new(60.0, 2000.0, 480),
    );
    for frame in &t.frames {
        assert_eq!(frame.state, PitchState::Voiced, "{frame:?}");
        assert_eq!(frame.frequency_hz, Some(60.0), "{frame:?}");
    }
}

#[test]
fn strong_second_harmonics_short_ranges_and_strict_thresholds() {
    let sine = |f: f64, rate: u32| -> Audio {
        let samples = (0..(0.3 * rate as f64) as usize)
            .map(|i| (0.5 * (2.0 * PI * f * i as f64 / rate as f64).sin()) as f32)
            .collect();
        Audio::new(vec![samples], rate).unwrap()
    };
    // A strong second harmonic above the range does not hide the fundamental
    // when its own dip misses the threshold.
    let t = track(
        tone(0.5, &[(1, 0.17), (2, 0.5)], |_| 220.0),
        PitchOptions::new(60.0, 430.0, 480),
    );
    for frame in &t.frames {
        assert_eq!(frame.state, PitchState::Voiced, "{frame:?}");
        assert!(
            cents(frame.frequency_hz.unwrap(), 220.0).abs() < 5.0,
            "{frame:?}"
        );
    }
    // Within the margin below a 50 Hz floor, a dip past the floor's lag.
    let t = track(
        tone(0.5, &[(1, 0.5)], |_| 49.86),
        PitchOptions::new(50.0, 2000.0, 480),
    );
    for frame in &t.frames {
        assert_eq!(frame.frequency_hz, Some(50.0), "{frame:?}");
    }
    // A strict threshold and a tone above the range: no octave error.
    let strict = PitchOptions {
        threshold: 0.02,
        ..PitchOptions::new(60.0, 2000.0, 160)
    };
    for frame in &sine(3520.0, 16_000).pitch_track(&strict).unwrap().frames {
        assert_ne!(frame.state, PitchState::Voiced, "{frame:?}");
    }
    // A range of short periods alone: 1998 Hz at 16 kHz, 1.5 to 2 kHz.
    let t = sine(1998.0, 16_000)
        .pitch_track(&PitchOptions::new(1500.0, 2000.0, 160))
        .unwrap();
    for frame in &t.frames {
        assert_eq!(frame.state, PitchState::Voiced, "{frame:?}");
        assert!(
            cents(frame.frequency_hz.unwrap(), 1998.0).abs() < 5.0,
            "{frame:?}"
        );
    }
}

#[test]
fn short_periods_are_refined_before_the_threshold() {
    let at = |parts: &[(f64, f64)], rate: u32| -> Audio {
        let samples = (0..(0.3 * rate as f64) as usize)
            .map(|i| {
                parts
                    .iter()
                    .map(|&(f, a)| a * (2.0 * PI * f * i as f64 / rate as f64).sin())
                    .sum::<f64>() as f32
            })
            .collect();
        Audio::new(vec![samples], rate).unwrap()
    };
    // B♭6 at 16 kHz with strong second and third harmonics: its own pitch, not
    // the octave below, though its dip's parabola is above the threshold.
    let f = 1864.655;
    let note = at(&[(f, 0.3), (2.0 * f, 0.5), (3.0 * f, 0.5)], 16_000);
    let t = note
        .pitch_track(&PitchOptions::new(60.0, 2000.0, 160))
        .unwrap();
    for frame in &t.frames {
        assert_eq!(frame.state, PitchState::Voiced, "{frame:?}");
        assert!(
            cents(frame.frequency_hz.unwrap(), f).abs() < 5.0,
            "{frame:?}"
        );
    }
    // A voiced frame's refined dip is below the threshold: G6 and B6 together.
    let dyad = at(&[(1567.98, 0.4), (1975.53, 0.4)], 16_000);
    let t = dyad
        .pitch_track(&PitchOptions::new(60.0, 2000.0, 160))
        .unwrap();
    for frame in t.frames.iter().filter(|f| f.state == PitchState::Voiced) {
        assert!(frame.confidence > 0.85, "{frame:?}");
    }
    // In the margin above a ceiling of 1 kHz (48 samples): set to the ceiling.
    let t = track(
        tone(0.3, &[(1, 0.5)], |_| 1002.0),
        PitchOptions::new(100.0, 1000.0, 480),
    );
    for frame in &t.frames {
        assert_eq!(frame.frequency_hz, Some(1000.0), "{frame:?}");
    }
}

#[test]
fn above_the_range_transients_and_octave_alternatives() {
    let at = |parts: &[(f64, f64)], rate: u32| -> Audio {
        let samples = (0..(0.3 * rate as f64) as usize)
            .map(|i| {
                parts
                    .iter()
                    .map(|&(f, a)| a * (2.0 * PI * f * i as f64 / rate as f64).sin())
                    .sum::<f64>() as f32
            })
            .collect();
        Audio::new(vec![samples], rate).unwrap()
    };
    // B♭6 above a 1 kHz ceiling at 16 kHz: no pitch, not the octave below.
    let f = 1864.655;
    let note = at(&[(f, 0.3), (2.0 * f, 0.5), (3.0 * f, 0.5)], 16_000);
    let t = note
        .pitch_track(&PitchOptions::new(60.0, 1000.0, 160))
        .unwrap();
    for frame in &t.frames {
        assert_ne!(frame.state, PitchState::Voiced, "{frame:?}");
    }
    // A 3 kHz ping that dies away in half a millisecond, every 50 ms: no
    // confident pitch from the quiet part of a frame.
    let samples: Vec<f32> = (0..24_000)
        .map(|i| {
            let t = (i % 2400) as f64 / RATE as f64;
            (0.5 * (-t / 0.0005).exp() * (2.0 * PI * 3000.0 * t).sin()) as f32
        })
        .collect();
    let t = track(samples, PitchOptions::new(60.0, 2000.0, 480));
    for frame in t.frames.iter().filter(|f| f.state == PitchState::Voiced) {
        assert!(frame.confidence < 0.99, "{frame:?}");
    }
    // The alternative is the other octave only: a strong octave above the range
    // leaves no rival at its odd repeats.
    let t = track(
        tone(0.5, &[(1, 0.15), (2, 0.5)], |_| 220.0),
        PitchOptions::new(60.0, 430.0, 480),
    );
    for frame in &t.frames {
        let p = frame.frequency_hz.unwrap();
        assert!(cents(p, 220.0).abs() < 5.0, "{frame:?}");
        if let Some(other) = frame.alternative {
            let ratio = other.frequency_hz / p;
            assert!(
                (ratio / 2.0 - 1.0).abs() < 0.06 || (ratio * 2.0 - 1.0).abs() < 0.06,
                "{frame:?}"
            );
        }
    }
}

#[test]
fn strict_and_loose_thresholds_attacks_and_the_upper_margin() {
    let at = |parts: &[(f64, f64)], rate: u32| -> Audio {
        let samples = (0..(0.3 * rate as f64) as usize)
            .map(|i| {
                parts
                    .iter()
                    .map(|&(f, a)| a * (2.0 * PI * f * i as f64 / rate as f64).sin())
                    .sum::<f64>() as f32
            })
            .collect();
        Audio::new(vec![samples], rate).unwrap()
    };
    // F6 with a strong fifth harmonic at 16 kHz and a strict threshold: its own
    // pitch under a 2 kHz ceiling, none under 1 kHz, never the octave below.
    let f = 1396.913;
    let note = at(&[(f, 0.4), (5.0 * f, 0.5)], 16_000);
    for (max_hz, voiced) in [(2000.0, true), (1000.0, false)] {
        let strict = PitchOptions {
            threshold: 0.02,
            ..PitchOptions::new(60.0, max_hz, 160)
        };
        for frame in &note.pitch_track(&strict).unwrap().frames {
            assert_eq!(frame.state == PitchState::Voiced, voiced, "{frame:?}");
            if let Some(p) = frame.frequency_hz {
                assert!(cents(p, f).abs() < 5.0, "{frame:?}");
            }
        }
    }
    // A loose threshold: lag 1 is no dip, so a clean tone keeps its pitch.
    let loose = PitchOptions {
        threshold: 0.65,
        ..PitchOptions::new(60.0, 2000.0, 480)
    };
    for frame in &track(tone(0.3, &[(1, 0.5)], |_| 440.0), loose).frames {
        assert!(
            cents(frame.frequency_hz.unwrap(), 440.0).abs() < 5.0,
            "{frame:?}"
        );
    }
    // An attack whose pitch and level fall within the first frame is judged by
    // the whole window, not its settled part.
    let attack: Vec<f32> = {
        let mut phase = 0.0;
        (0..9600)
            .map(|i| {
                let t = i as f64 / RATE as f64;
                let decay = (-t / 0.0015).exp();
                let f = 1760.0 * (1.0 + decay);
                let a = 0.10 + 0.15 * decay;
                phase += 2.0 * PI * f / RATE as f64;
                (a * phase.sin()) as f32
            })
            .collect()
    };
    let first = track(attack, PitchOptions::new(60.0, 2000.0, 480)).frames[0];
    assert!(
        first.state != PitchState::Voiced || first.confidence < 0.95,
        "{first:?}"
    );
    // The octave at the top of a 26–55 Hz range, within its margin, is offered.
    let t = track(
        tone(0.5, &[(1, 0.11), (2, 0.35)], |_| 27.5),
        PitchOptions::new(26.0, 55.0, 480),
    );
    let frame = t.frames[0];
    assert!(
        cents(frame.frequency_hz.unwrap(), 27.5).abs() < 5.0,
        "{frame:?}"
    );
    let other = frame.alternative.unwrap().frequency_hz;
    assert!(cents(other, 55.0).abs() < 10.0, "{frame:?}");
}
