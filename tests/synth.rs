#![cfg(feature = "synth")]
use audiobox::ErrorCode;
use audiobox::synth::*;

const RATE: u32 = 48_000;

/// One channel playing `pitch` for one second (a quarter note at 60 BPM).
fn one_note(instrument: Instrument, pitch: Pitch, aliasing: Aliasing) -> Vec<f32> {
    let mut score = Score::new(60, 48).unwrap();
    let i = score.add_instrument("i", instrument).unwrap();
    score
        .channel("a")
        .unwrap()
        .instrument(i)
        .unwrap()
        .note(pitch, Length::QUARTER)
        .unwrap();
    let options = RenderOptions {
        aliasing,
        gain: 1.0,
        ..Default::default()
    };
    let audio = score.render_with(RATE, &options).unwrap();
    audio.channel_data(0).unwrap().to_vec()
}

/// Frequency from the interpolated times of upward zero crossings.
fn measured_frequency(samples: &[f32]) -> f64 {
    let mut crossings = Vec::new();
    for i in 1..samples.len() {
        let (a, b) = (samples[i - 1] as f64, samples[i] as f64);
        if a < 0.0 && b >= 0.0 {
            crossings.push(i as f64 - 1.0 + a / (a - b));
        }
    }
    let (first, last) = (crossings[0], crossings[crossings.len() - 1]);
    (crossings.len() - 1) as f64 * RATE as f64 / (last - first)
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

const REFERENCE: &str = "
; Reference score for the checksum and chunking tests.
tempo 150
ticks 48
instrument lead  wave=pulse duty=25,50,75 duty-step=40 adsr=5,80,60,50 vibrato=20,150,100
instrument arp   wave=pulse duty=12.5 adsr=0,30,50,20 volume=10 arpeggio=0,3,7,12 arpeggio-step=25
instrument bass  wave=triangle adsr=3,0,100,30
instrument saw   wave=saw adsr=20,0,80,80 volume=8 slide=-50
instrument kick  wave=triangle adsr=0,80,0,0 slide=-5000
instrument hat   wave=noise noise=short adsr=0,30,0,0 volume=6
instrument hiss  wave=noise adsr=0,90,0,0 volume=9

pattern riff = c8 e8 g8 >c8< b-8 g8 e4
pattern beat = @kick o3 c8 @hat o8 c8 @hiss o6 c8 @hat o8 c8

channel lead = @lead o5 q7 r2 L $riff d8 f8 a8 >d8< c+8 a8 f4 [c16 d16 e8 | g8]3 c2&c8 r4.
channel arp  = @arp o4 l4 r2 L [c f g c]4
channel bass = @bass o2 l8 v12 r2 L [[c >c< c >c<]2 [f >f< g >g<]2]2
channel saw  = @saw o3 r2 L [e1^2 d2]2
channel drum = r2 L [$beat]8
";

#[test]
fn waveform_frequencies() {
    let pitches = [Pitch::A4, Pitch::new(36).unwrap(), Pitch::new(88).unwrap()];
    let waves = [
        Waveform::Pulse(Duty::Half),
        Waveform::Pulse(Duty::Eighth),
        Waveform::Triangle,
        Waveform::Sawtooth,
    ];
    for aliasing in [Aliasing::Naive, Aliasing::BandLimited] {
        for wave in waves {
            for pitch in pitches {
                let expected = 440.0 * 2f64.powf((pitch.midi() as f64 - 69.0) / 12.0);
                assert!((pitch.frequency() - expected).abs() < 1e-9);
                let f = measured_frequency(&one_note(Instrument::new(wave), pitch, aliasing));
                assert!(
                    (f / expected - 1.0).abs() < 1e-4,
                    "{wave:?} {aliasing:?}: {f} Hz instead of {expected} Hz"
                );
            }
        }
    }
}

#[test]
fn pulse_duty() {
    for duty in [Duty::Eighth, Duty::Quarter, Duty::Half, Duty::ThreeQuarters] {
        let samples = one_note(
            Instrument::new(Waveform::Pulse(duty)),
            Pitch::A4,
            Aliasing::Naive,
        );
        let high = samples.iter().filter(|&&s| s > 0.0).count() as f64 / samples.len() as f64;
        assert!((high - duty.fraction()).abs() < 0.002, "{duty:?}: {high}");
        assert!(samples.iter().all(|&s| s.abs() == 1.0));
    }
}

#[test]
fn band_limiting_reduces_aliases() {
    // At 48 kHz, harmonics of 3520 Hz above 24 kHz fold back between the true harmonics.
    let level = |samples: &[f32], hz: f64| {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, &s) in samples.iter().enumerate() {
            let w = 2.0 * std::f64::consts::PI * hz * n as f64 / RATE as f64;
            re += s as f64 * w.cos();
            im += s as f64 * w.sin();
        }
        (re * re + im * im).sqrt()
    };
    let a7 = Pitch::new(105).unwrap();
    for wave in [
        Waveform::Sawtooth,
        Waveform::Pulse(Duty::Quarter),
        Waveform::Triangle,
    ] {
        let naive = one_note(Instrument::new(wave), a7, Aliasing::Naive);
        let clean = one_note(Instrument::new(wave), a7, Aliasing::BandLimited);
        let f = a7.frequency();
        // Sum the folded harmonics between 6 and 12 times the fundamental.
        let aliases: Vec<f64> = (6..=12)
            .map(|k| {
                let h = (k as f64 * f) % RATE as f64;
                if h > RATE as f64 / 2.0 {
                    RATE as f64 - h
                } else {
                    h
                }
            })
            .filter(|&h| h < RATE as f64 / 2.0 - 100.0 && (h / f - (h / f).round()).abs() > 0.05)
            .collect();
        let sum = |s: &[f32]| aliases.iter().map(|&h| level(s, h)).sum::<f64>() / level(s, f);
        let (n, c) = (sum(&naive), sum(&clean));
        assert!(
            c < n / 3.0,
            "{wave:?}: aliases {c} band-limited vs {n} naive"
        );
    }
}

#[test]
fn envelope_stage_timing() {
    let mut instrument = Instrument::new(Waveform::Pulse(Duty::Half));
    instrument.envelope = Envelope {
        attack_ms: 10,
        decay_ms: 20,
        sustain: 50,
        release_ms: 30,
    };
    let s: Vec<f32> = one_note(instrument, Pitch::A4, Aliasing::Naive)
        .iter()
        .map(|s| s.abs())
        .collect();
    let close = |i: usize, level: f32| {
        assert!(
            (s[i] - level).abs() < 1e-6,
            "sample {i}: {} != {level}",
            s[i]
        )
    };
    close(0, 0.0);
    close(240, 0.5);
    close(479, 479.0 / 480.0);
    close(480, 1.0); // Attack ends after 10 ms.
    close(960, 0.75);
    close(1440, 0.5); // Decay ends after 20 ms more.
    close(30_000, 0.5);
    close(47_999, 0.5);
    close(48_000, 0.5); // The note ends at one second and releases.
    close(48_720, 0.25);
    assert_eq!(s.len(), 48_000 + 1440); // Release lasts 30 ms.
}

#[test]
fn gate_shortens_notes() {
    let score = Score::parse(
        "tempo 60\ninstrument p wave=pulse adsr=0,0,100,0\nchannel a = @p q4 c4 q8 c4",
    )
    .unwrap();
    let options = RenderOptions {
        aliasing: Aliasing::Naive,
        gain: 1.0,
        ..Default::default()
    };
    let s = score.render_with(RATE, &options).unwrap();
    let s = s.channel_data(0).unwrap();
    assert_eq!(s.len(), 96_000);
    assert!(s[..24_000].iter().all(|v| v.abs() == 1.0));
    assert!(s[24_000..48_000].iter().all(|&v| v == 0.0));
    assert!(s[48_000..].iter().all(|v| v.abs() == 1.0));
}

#[test]
fn noise_modes() {
    let autocorrelation = |s: &[f32], lag: usize| {
        let s = &s[..s.len() - lag];
        let dot: f64 = s.iter().zip(&s[lag..]).map(|(a, b)| (a * b) as f64).sum();
        dot / s.iter().map(|a| (a * a) as f64).sum::<f64>()
    };
    let lag = (RATE as f64 / 440.0).round() as usize;
    let short = one_note(
        Instrument::new(Waveform::Noise(NoiseMode::Short)),
        Pitch::A4,
        Aliasing::Naive,
    );
    let long = one_note(
        Instrument::new(Waveform::Noise(NoiseMode::Long)),
        Pitch::A4,
        Aliasing::Naive,
    );
    let (s, l) = (autocorrelation(&short, lag), autocorrelation(&long, lag));
    assert!(s > 0.7, "short noise repeats at the note's period: {s}");
    assert!(l.abs() < 0.1, "long noise does not: {l}");
    assert!((measured_frequency(&short) / 440.0 - 1.0).abs() < 0.2 || s > 0.7);

    for (mode, period) in [(NoiseMode::Long, 32767), (NoiseMode::Short, 93)] {
        let mut lfsr = Lfsr::new(mode);
        let first: Vec<u16> = (0..period)
            .map(|_| {
                lfsr.step();
                lfsr.state()
            })
            .collect();
        assert_eq!(first.last(), Some(&1));
        let mut distinct = first.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            period,
            "{mode:?} repeats every {period} steps"
        );
    }
}

#[test]
fn parsed_score_equals_built_score() {
    let text = "
        tempo 132
        ticks 96
        instrument lead wave=pulse duty=25,50 duty-step=30 adsr=2,50,70,40 volume=12 vibrato=15,180,120
        instrument bass wave=triangle adsr=0,0,100,20 slide=-100
        instrument hat wave=noise noise=short adsr=0,30,0,0 arpeggio=0,12 arpeggio-step=10
        pattern up = c d e
        channel melody = @lead o5 l8 v10 $up f+4. g-16 r16
        channel melody = L [a b | >c<]2 c4^8 d4&d16 q6 e%24
        channel bass = @bass o2 l2 c g < b > c
        channel hat = @hat o7 l16 [c r]7 L c
    ";
    let parsed = Score::parse(text).unwrap();

    let mut score = Score::new(132, 96).unwrap();
    let lead = score
        .add_instrument(
            "lead",
            Instrument {
                waveform: Waveform::Pulse(Duty::Quarter),
                envelope: Envelope {
                    attack_ms: 2,
                    decay_ms: 50,
                    sustain: 70,
                    release_ms: 40,
                },
                volume: 12,
                vibrato: Some(Vibrato {
                    depth_cents: 15,
                    period_ms: 180,
                    delay_ms: 120,
                }),
                duty_sweep: Some(DutySweep {
                    duties: vec![Duty::Half],
                    step_ms: 30,
                }),
                ..Instrument::default()
            },
        )
        .unwrap();
    let bass = score
        .add_instrument(
            "bass",
            Instrument {
                waveform: Waveform::Triangle,
                envelope: Envelope {
                    release_ms: 20,
                    ..Envelope::default()
                },
                slide: -100,
                ..Instrument::default()
            },
        )
        .unwrap();
    let hat = score
        .add_instrument(
            "hat",
            Instrument {
                waveform: Waveform::Noise(NoiseMode::Short),
                envelope: Envelope {
                    sustain: 0,
                    decay_ms: 30,
                    ..Envelope::default()
                },
                arpeggio: Some(Arpeggio {
                    offsets: vec![0, 12],
                    step_ms: 10,
                }),
                ..Instrument::default()
            },
        )
        .unwrap();
    let p = |letter, accidental, octave| Pitch::from_name(letter, accidental, octave).unwrap();
    let e8 = Length::EIGHTH;
    let mut m = score.channel("melody").unwrap();
    m.instrument(lead).unwrap().volume(10).unwrap();
    for letter in ['c', 'd', 'e'] {
        m.note(p(letter, 0, 5), e8).unwrap();
    }
    m.note(p('f', 1, 5), Length::QUARTER.dotted()).unwrap();
    m.note(p('g', -1, 5), Length::SIXTEENTH).unwrap();
    m.rest(Length::SIXTEENTH).unwrap();
    m.loop_point().unwrap();
    m.note(p('a', 0, 5), e8)
        .unwrap()
        .note(p('b', 0, 5), e8)
        .unwrap();
    m.note(p('c', 0, 6), e8).unwrap();
    m.note(p('a', 0, 5), e8)
        .unwrap()
        .note(p('b', 0, 5), e8)
        .unwrap();
    m.note(p('c', 0, 5), Length::QUARTER)
        .unwrap()
        .tie(e8)
        .unwrap();
    m.note(p('d', 0, 5), Length::QUARTER)
        .unwrap()
        .tie(Length::SIXTEENTH)
        .unwrap();
    m.gate(6)
        .unwrap()
        .note(p('e', 0, 5), Length::Ticks(24))
        .unwrap();
    let mut b = score.channel("bass").unwrap();
    b.instrument(bass).unwrap();
    for (letter, octave) in [('c', 2), ('g', 2), ('b', 1), ('c', 2)] {
        b.note(p(letter, 0, octave), Length::HALF).unwrap();
    }
    let mut h = score.channel("hat").unwrap();
    h.instrument(hat).unwrap();
    for _ in 0..7 {
        h.note(p('c', 0, 7), Length::SIXTEENTH).unwrap();
        h.rest(Length::SIXTEENTH).unwrap();
    }
    h.loop_point()
        .unwrap()
        .note(p('c', 0, 7), Length::SIXTEENTH)
        .unwrap();

    assert_eq!(parsed, score);
    assert_eq!(parsed.loop_tick(), Some(336));
    assert_eq!(
        parsed.render(22_050).unwrap().channel_data(0).unwrap(),
        score.render(22_050).unwrap().channel_data(0).unwrap()
    );
}

#[test]
fn parse_errors_have_positions_and_codes() {
    let cases: &[(&str, usize, usize, ErrorCode)] = &[
        ("foo 1", 1, 1, ErrorCode::Syntax),
        ("tempo 1000", 1, 7, ErrorCode::InvalidArgument),
        ("tempo 90\ntempo 91", 2, 1, ErrorCode::Syntax),
        ("tempo 99999999999", 1, 7, ErrorCode::InvalidArgument),
        ("tempo 90 x", 1, 10, ErrorCode::Syntax),
        ("instrument p wave=sine", 1, 19, ErrorCode::Syntax),
        ("instrument p volume=16", 1, 21, ErrorCode::InvalidArgument),
        (
            "instrument p wave=saw duty=25",
            1,
            23,
            ErrorCode::InvalidArgument,
        ),
        ("instrument p colour=red", 1, 14, ErrorCode::Syntax),
        ("instrument p adsr=1,2,3", 1, 19, ErrorCode::Syntax),
        (
            "instrument p vibrato=10,0",
            1,
            12,
            ErrorCode::InvalidArgument,
        ),
        (
            "instrument p\ninstrument p",
            2,
            12,
            ErrorCode::InvalidArgument,
        ),
        ("channel a = c", 1, 13, ErrorCode::InvalidArgument),
        ("channel a c", 1, 11, ErrorCode::Syntax),
        ("channel a = @x c", 1, 13, ErrorCode::Syntax),
        (
            "instrument p\nchannel a = @p c4 [d e",
            2,
            19,
            ErrorCode::Syntax,
        ),
        ("instrument p\nchannel a = @p ]", 2, 16, ErrorCode::Syntax),
        (
            "instrument p\nchannel a = @p c | d",
            2,
            18,
            ErrorCode::Syntax,
        ),
        (
            "instrument p\nchannel a = @p [c | d | e]",
            2,
            23,
            ErrorCode::Syntax,
        ),
        (
            "instrument p\nchannel a = @p c64.",
            2,
            16,
            ErrorCode::InvalidArgument,
        ),
        (
            "instrument p\nchannel a = @p c4&d4",
            2,
            18,
            ErrorCode::Syntax,
        ),
        ("instrument p\nchannel a = @p c4&", 2, 18, ErrorCode::Syntax),
        (
            "instrument p\nchannel a = @p c4& r",
            2,
            18,
            ErrorCode::Syntax,
        ),
        (
            "instrument p\nchannel a = @p v16",
            2,
            16,
            ErrorCode::InvalidArgument,
        ),
        (
            "instrument p\nchannel a = @p o9 > c",
            2,
            19,
            ErrorCode::InvalidArgument,
        ),
        (
            "instrument p\nchannel a = @p o9 b+",
            2,
            19,
            ErrorCode::InvalidArgument,
        ),
        (
            "instrument p\nchannel a = @p c%0",
            2,
            17,
            ErrorCode::InvalidArgument,
        ),
        (
            "instrument p\nchannel a = @p c%3.",
            2,
            17,
            ErrorCode::Syntax,
        ),
        ("instrument p\nchannel a = @p l c", 2, 16, ErrorCode::Syntax),
        (
            "instrument p\nchannel a = @p c4 L\nchannel b = @p L",
            3,
            16,
            ErrorCode::InvalidArgument,
        ),
        ("  channel a = x", 1, 15, ErrorCode::Syntax),
        ("channel a = ^4", 1, 13, ErrorCode::InvalidArgument),
        ("channel a = $nope", 1, 13, ErrorCode::Syntax),
        (
            "pattern p = c $p\ninstrument i\nchannel a = @i $p",
            1,
            15,
            ErrorCode::Syntax,
        ),
        (
            "; é ü\ninstrument p ; ok\nchannel a = @p ü",
            3,
            16,
            ErrorCode::Syntax,
        ),
    ];
    for &(text, line, column, code) in cases {
        let error = Score::parse(text).expect_err(text);
        assert_eq!(
            (error.line, error.column, error.code),
            (line, column, code),
            "{text:?}: {error}"
        );
        let converted: audiobox::Error = error.clone().into();
        assert_eq!(converted.code, code);
        assert!(
            converted
                .message
                .starts_with(&format!("line {line}, column {column}: "))
        );
    }
}

#[test]
fn limits_are_enforced() {
    let limited = |text: &str, limits: ScoreLimits| {
        let error = Score::parse_with_limits(text, &limits).expect_err(text);
        assert_eq!(error.code, ErrorCode::LimitExceeded, "{text}: {error}");
        error
    };
    let defaults = ScoreLimits::default;
    let text = "x".repeat(101);
    limited(
        &text,
        ScoreLimits {
            max_text_bytes: 100,
            ..defaults()
        },
    );

    let channels: String = (0..9).map(|i| format!("channel c{i} = r\n")).collect();
    assert_eq!(limited(&channels, defaults()).line, 9);
    limited(
        &channels[..60],
        ScoreLimits {
            max_channels: 3,
            ..defaults()
        },
    );
    assert!(
        Score::with_limits(
            120,
            48,
            ScoreLimits {
                max_channels: 9,
                ..defaults()
            }
        )
        .is_err()
    );

    let nested = "instrument p\nchannel a = @p [[[[c]2]2]2]2";
    assert!(Score::parse(nested).is_ok());
    limited(
        nested,
        ScoreLimits {
            max_nesting: 3,
            ..defaults()
        },
    );
    let calls = "instrument p\npattern a = c\npattern b = $a\npattern c = $b\nchannel x = @p $c";
    limited(
        calls,
        ScoreLimits {
            max_nesting: 2,
            ..defaults()
        },
    );

    // Repeats of commands that take no time are bounded by the event budget.
    let start = std::time::Instant::now();
    limited("channel a = [[[[[[o4]999]999]999]999]999]999", defaults());
    assert!(start.elapsed().as_secs() < 5);

    let long = "tempo 60\ninstrument p\nchannel a = @p [c1]4";
    assert!(
        Score::parse_with_limits(
            long,
            &ScoreLimits {
                max_duration_seconds: 16.0,
                ..defaults()
            }
        )
        .is_ok()
    );
    limited(
        long,
        ScoreLimits {
            max_duration_seconds: 15.0,
            ..defaults()
        },
    );
    limited("channel a = [r1]9999", defaults());
    limited(
        long,
        ScoreLimits {
            max_notes: 3,
            ..defaults()
        },
    );
}

#[test]
fn chunks_equal_offline_render() {
    let score = Score::parse(REFERENCE).unwrap();
    for aliasing in [Aliasing::BandLimited, Aliasing::Naive] {
        let options = RenderOptions {
            aliasing,
            loops: Some(2),
            ..Default::default()
        };
        let offline = score.render_with(44_100, &options).unwrap();
        let offline = offline.channel_data(0).unwrap();
        let mut synth = Synth::new(&score, 44_100, &options).unwrap();
        for round in 0..2 {
            let mut chunked = Vec::new();
            let sizes = [1, 7, 64, 333, 1000, 4096];
            let mut buffer = [0.0f32; 4096];
            let mut i = 0;
            while !synth.is_finished() {
                let n = sizes[i % sizes.len()];
                let written = synth.fill(&mut buffer[..n]);
                chunked.extend_from_slice(&buffer[..written]);
                i += 1;
            }
            assert_eq!(synth.fill(&mut buffer), 0);
            assert_eq!(chunked.len(), offline.len(), "round {round}");
            assert!(
                chunked
                    .iter()
                    .zip(offline)
                    .all(|(a, b)| a.to_bits() == b.to_bits())
            );
            assert_eq!(synth.frames_rendered(), offline.len() as u64);
            synth.reset();
        }
    }
}

#[test]
fn loops_repeat_the_loop_section() {
    let score = Score::parse(REFERENCE).unwrap();
    let rate = 44_100u64;
    let samples = |ticks: u64| ticks * rate * 60 / (150 * 48);
    let once = score.render(rate as u32).unwrap();
    let twice = score
        .render_with(
            rate as u32,
            &RenderOptions {
                loops: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    let end = samples(score.length_ticks());
    let body = end - samples(score.loop_tick().unwrap());
    assert!(once.frames() as u64 >= end && once.frames() as u64 <= end + rate / 10);
    assert_eq!(twice.frames() - once.frames(), body as usize);
    // The repeat of the loop section continues the first pass without restarting the song.
    let a = once.channel_data(0).unwrap();
    let b = twice.channel_data(0).unwrap();
    assert_eq!(&a[..end as usize], &b[..end as usize]);

    let endless = RenderOptions {
        loops: None,
        ..Default::default()
    };
    assert!(score.render_with(44_100, &endless).is_err());
    let mut synth = Synth::new(&score, 44_100, &endless).unwrap();
    assert_eq!(synth.max_frames(), None);
    let mut buffer = vec![0.0; 44_100];
    for _ in 0..(3 * body as usize / buffer.len() + 2) {
        assert_eq!(synth.fill(&mut buffer), buffer.len());
    }
    assert!(!synth.is_finished());

    // Without a loop point, the whole song repeats.
    let plain = Score::parse("instrument p\nchannel a = @p c d").unwrap();
    let audio = plain
        .render_with(
            8000,
            &RenderOptions {
                loops: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(audio.frames(), 3 * 8000);
}

#[test]
fn loop_point_joins_without_a_click() {
    // A triangle ends one note at the song's end and starts another at the loop point.
    let score = Score::parse(
        "tempo 100\ninstrument t wave=triangle adsr=8,0,100,8\nchannel a = @t o3 c4 L e4 g4 >c4<",
    )
    .unwrap();
    let options = RenderOptions {
        loops: Some(2),
        gain: 1.0,
        ..Default::default()
    };
    let audio = score.render_with(RATE, &options).unwrap();
    let s = audio.channel_data(0).unwrap();
    let samples = |ticks: u64| (ticks * RATE as u64 * 60 / (100 * 48)) as usize;
    let end = samples(score.length_ticks());
    let body = end - samples(48);
    // Tolerance: the steepest slope of a full-scale triangle at the highest note, C4, plus 1%.
    let bound = 4.0 * Pitch::C4.frequency() / RATE as f64;
    let max_step = |range: std::ops::Range<usize>| {
        range
            .map(|i| (s[i + 1] - s[i]).abs() as f64)
            .fold(0.0, f64::max)
    };
    for join in [end, end + body] {
        let step = max_step(join - 4..join + 4);
        assert!(
            step <= bound * 1.01,
            "step {step} at the join exceeds {bound}"
        );
    }
}

#[test]
fn note_across_the_loop_point_restarts() {
    let score = Score::parse(
        "tempo 60\ninstrument p wave=pulse adsr=100,0,100,0\nchannel a = @p c2\nchannel b = r4 L r4",
    )
    .unwrap();
    let options = RenderOptions {
        aliasing: Aliasing::Naive,
        loops: Some(1),
        gain: 1.0,
    };
    let s = score.render_with(1000, &options).unwrap();
    let s = s.channel_data(0).unwrap();
    assert_eq!(s.len(), 3000);
    assert_eq!(s[1500].abs(), 1.0);
    // At the loop point, the attack restarts from the level the note had at the end.
    assert_eq!(s[2000].abs(), 1.0);
    assert_eq!(s[2050].abs(), 1.0);
}

#[test]
fn reference_score_checksum() {
    let score = Score::parse(REFERENCE).unwrap();
    let band = score.render(44_100).unwrap();
    let naive = score
        .render_with(
            44_100,
            &RenderOptions {
                aliasing: Aliasing::Naive,
                ..Default::default()
            },
        )
        .unwrap();
    let (band, naive) = (
        band.channel_data(0).unwrap(),
        naive.channel_data(0).unwrap(),
    );
    assert!(band.iter().all(|s| s.abs() < 2.0));
    // 7.2 s of song and the saw's 80 ms release.
    assert_eq!(band.len(), 321_048);
    assert_eq!(fnv1a(band), 0x2465_3f3e_8981_4c8f);
    assert_eq!(fnv1a(naive), 0xea84_59e0_7d5d_00b8);
}

#[test]
fn builder_validation() {
    assert!(Score::new(0, 48).is_err());
    assert!(Score::new(120, 961).is_err());
    assert_eq!(Length::note(4).dotted().ticks(48).unwrap(), 72);
    assert_eq!(Length::note(3).ticks(48).unwrap(), 64);
    assert!(Length::note(64).dotted().ticks(48).is_err());
    assert!(Length::note(0).ticks(48).is_err());
    assert!(Length::Ticks(0).ticks(48).is_err());
    assert_eq!(Pitch::from_name('a', 0, 4).unwrap(), Pitch::A4);
    assert_eq!(Pitch::from_name('c', -1, 4).unwrap().midi(), 59);
    assert!(Pitch::from_name('h', 0, 4).is_err());
    assert!(Pitch::new(128).is_err());
    let mut score = Score::new(120, 48).unwrap();
    let mut bad = Instrument::new(Waveform::Triangle);
    bad.duty_sweep = Some(DutySweep {
        duties: vec![Duty::Half],
        step_ms: 10,
    });
    assert!(score.add_instrument("bad", bad).is_err());
    let i = score.add_instrument("ok", Instrument::default()).unwrap();
    let mut channel = score.channel("a").unwrap();
    assert!(channel.tie(Length::QUARTER).is_err());
    assert!(channel.instrument(5).is_err());
    assert!(channel.volume(16).is_err());
    assert!(channel.gate(0).is_err());
    channel
        .instrument(i)
        .unwrap()
        .note(Pitch::C4, Length::HALF)
        .unwrap();
    assert_eq!(channel.position(), 96);
    assert_eq!(score.duration_seconds(), 1.0);
    assert!(Synth::new(&score, 0, &RenderOptions::default()).is_err());
    let empty = Score::new(120, 48).unwrap();
    assert_eq!(empty.render(8000).unwrap().frames(), 0);
}

#[test]
fn malformed_text_never_panics() {
    let alphabet: Vec<char> = "abcdefgrlovq@$L[]|^&<>+-#.%0123456789 =\n;,xé"
        .chars()
        .collect();
    let limits = ScoreLimits {
        max_duration_seconds: 30.0,
        ..Default::default()
    };
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let chars: Vec<char> = REFERENCE.chars().collect();
    let mut checked = 0;
    for cut in (0..chars.len()).step_by(7) {
        let text: String = chars[..cut].iter().collect();
        let _ = Score::parse_with_limits(&text, &limits);
    }
    for _ in 0..400 {
        let mut text = chars.clone();
        for _ in 0..1 + next() % 6 {
            let at = (next() % text.len() as u64) as usize;
            text[at] = alphabet[(next() % alphabet.len() as u64) as usize];
        }
        let text: String = text.into_iter().collect();
        if let Ok(score) = Score::parse_with_limits(&text, &limits) {
            score.render(8000).unwrap();
            checked += 1;
        }
    }
    assert!(checked > 10);
}
