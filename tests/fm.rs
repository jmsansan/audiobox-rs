#![cfg(feature = "synth")]
use audiobox::ErrorCode;
use audiobox::synth::*;

const EPIANO: &str = "; a six-operator patch using every line kind
fm e-piano algorithm=5 feedback=6 transpose=-12 volume=12 voices=4 unison=2 unison-detune=8 spread=60 pan=-10
  pitch rates=94,67,95,60 levels=50,50,50,50
  lfo wave=sine speed=34 delay=33 pitch=4 amp=20 sync=off pitch-sensitivity=2 seed=7
  op 1 ratio=1 detune=3 level=99 rates=96,25,25,67 levels=99,75,0,0 velocity=2 rate-scaling=3
  op 2 ratio=14 level=58 rates=95,50,35,78 levels=99,75,0,0 velocity=7 amp-sensitivity=1
  op 3 ratio=1 level=99 rates=95,20,20,50 levels=99,95,0,0 velocity=2
  op 4 ratio=1 level=89 rates=95,29,20,50 levels=99,95,0,0 velocity=6 ; modulator
  op 5 ratio=0.5,50 level=99 scaling=41,0,19 curves=-lin,+exp detune=-7
  op 6 fixed=100,25 level=79 detune=7
end
";

fn error(text: &str) -> ParseError {
    FmPatch::parse(text).unwrap_err()
}
fn at(text: &str, line: usize, column: usize, code: ErrorCode) {
    let e = error(text);
    assert_eq!((e.line, e.column, e.code), (line, column, code), "{e}");
}

#[test]
fn parses_every_field() {
    let patch = FmPatch::parse(EPIANO).unwrap();
    assert_eq!((patch.name.as_str(), patch.volume), ("e-piano", 12));
    let v = patch.voice;
    assert_eq!((v.algorithm, v.feedback, v.transpose), (5, 6, -12));
    assert_eq!((v.voices, v.unison, v.unison_detune), (4, 2, 8));
    assert_eq!((v.spread, v.pan, v.key_sync), (60, -10, true));
    assert_eq!(v.pitch_envelope.rates, [94, 67, 95, 60]);
    assert_eq!(
        v.lfo,
        FmLfo {
            wave: FmLfoWave::Sine,
            speed: 34,
            delay: 33,
            pitch_depth: 4,
            amplitude_depth: 20,
            key_sync: false,
            pitch_sensitivity: 2,
            beats: None,
            seed: 7,
        }
    );
    let op1 = v.operators[0].unwrap();
    assert_eq!((op1.detune, op1.level, op1.velocity), (3, 99, 2));
    assert_eq!(op1.rate_scaling, 3);
    assert_eq!(op1.envelope.rates, [96, 25, 25, 67]);
    assert_eq!(op1.envelope.levels, [99, 75, 0, 0]);
    let op2 = v.operators[1].unwrap();
    assert_eq!(
        op2.frequency,
        FmFrequency::Ratio {
            coarse: 14,
            fine: 0
        }
    );
    assert_eq!(op2.amplitude_sensitivity, 1);
    let op5 = v.operators[4].unwrap();
    assert_eq!(op5.frequency.ratio(), Some(0.75));
    assert_eq!(
        op5.scaling,
        FmScaling {
            break_point: 41,
            left_depth: 0,
            right_depth: 19,
            left_curve: FmCurve::NegativeLinear,
            right_curve: FmCurve::PositiveExponential,
        }
    );
    let op6 = v.operators[5].unwrap();
    assert_eq!(
        op6.frequency,
        FmFrequency::Fixed {
            decade: 2,
            fine: 25
        }
    );
    assert!((op6.frequency.fixed_hz().unwrap() - 177.827941).abs() < 1e-5);
    assert_eq!(op6.envelope, FmEnvelope::OPERATOR);
}

#[test]
fn text_round_trips() {
    let patch = FmPatch::parse(EPIANO).unwrap();
    let text = patch.to_string();
    assert_eq!(FmPatch::parse(&text).unwrap(), patch);
    assert_eq!(FmPatch::parse(&text).unwrap().to_string(), text);
    assert_eq!(text.parse::<FmPatch>().unwrap(), patch);
}

#[test]
fn defaults_keep_short_patches_short() {
    let patch = FmPatch::parse("fm sine\nop 1\nend").unwrap();
    assert_eq!(patch.voice, Fm::default());
    assert_eq!(patch.volume, 15);
    assert_eq!(
        patch.to_string(),
        "fm sine algorithm=1\n  op 1 ratio=1 level=99\nend\n"
    );
    let op = FmOperator::default();
    assert_eq!(op.frequency.ratio(), Some(1.0));
    assert_eq!(op.frequency.fixed_hz(), None);
    assert_eq!(op.scaling.break_point + 21, 60);
    assert!(Fm::default().validate().is_ok());
}

// Deterministic patches that visit every field's range, including both ends.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, min: i64, max: i64) -> i64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let span = (max - min + 1) as u64;
        match (self.0 >> 33) % 5 {
            0 => min,
            1 => max,
            _ => min + ((self.0 >> 20) % span) as i64,
        }
    }
    fn four(&mut self) -> [u8; 4] {
        [0; 4].map(|_| self.next(0, 99) as u8)
    }
    fn curve(&mut self) -> FmCurve {
        [
            FmCurve::NegativeLinear,
            FmCurve::NegativeExponential,
            FmCurve::PositiveExponential,
            FmCurve::PositiveLinear,
        ][self.next(0, 3) as usize]
    }
}
fn generated(seed: u64) -> FmPatch {
    let mut r = Lcg(seed);
    let algorithm = r.next(1, 32) as u8;
    let routing = FmRouting::algorithm(algorithm).unwrap();
    let mut operators = [None; FM_OPERATORS];
    for (index, slot) in operators.iter_mut().enumerate() {
        let carrier = routing.is_carrier(index as u8 + 1);
        if !carrier && r.next(0, 3) == 0 {
            continue;
        }
        let frequency = if r.next(0, 2) == 0 {
            FmFrequency::Fixed {
                decade: r.next(0, 3) as u8,
                fine: r.next(0, 99) as u8,
            }
        } else {
            FmFrequency::Ratio {
                coarse: r.next(0, 31) as u8,
                fine: r.next(0, 99) as u8,
            }
        };
        *slot = Some(FmOperator {
            frequency,
            detune: r.next(-7, 7) as i8,
            level: r.next(0, 99) as u8,
            envelope: FmEnvelope {
                rates: r.four(),
                levels: r.four(),
            },
            scaling: FmScaling {
                break_point: r.next(0, 99) as u8,
                left_depth: r.next(0, 99) as u8,
                right_depth: r.next(0, 99) as u8,
                left_curve: r.curve(),
                right_curve: r.curve(),
            },
            rate_scaling: r.next(0, 7) as u8,
            velocity: r.next(0, 7) as u8,
            amplitude_sensitivity: r.next(0, 3) as u8,
        });
    }
    let mono = r.next(0, 3) == 0;
    let unison = r.next(1, 8) as u8;
    let voices = if mono {
        1
    } else {
        r.next(1, (32 / unison as i64).min(16)) as u8
    };
    let waves = [
        FmLfoWave::Triangle,
        FmLfoWave::SawDown,
        FmLfoWave::SawUp,
        FmLfoWave::Square,
        FmLfoWave::Sine,
        FmLfoWave::SampleHold,
    ];
    FmPatch {
        name: format!("p_{seed}-x"),
        volume: r.next(0, 15) as u8,
        voice: Fm {
            algorithm,
            feedback: r.next(0, 7) as u8,
            key_sync: r.next(0, 1) == 0,
            transpose: r.next(-24, 24) as i8,
            operators,
            pitch_envelope: FmEnvelope {
                rates: r.four(),
                levels: r.four(),
            },
            lfo: FmLfo {
                wave: waves[r.next(0, 5) as usize],
                speed: r.next(0, 99) as u8,
                delay: r.next(0, 99) as u8,
                pitch_depth: r.next(0, 99) as u8,
                amplitude_depth: r.next(0, 99) as u8,
                key_sync: r.next(0, 1) == 0,
                pitch_sensitivity: r.next(0, 7) as u8,
                beats: (r.next(0, 1) == 0).then(|| (r.next(1, 64) as u16, r.next(1, 64) as u16)),
                seed: r.next(0, u32::MAX as i64) as u32,
            },
            voices,
            unison,
            unison_detune: r.next(0, 100) as u16,
            spread: r.next(0, 100) as u8,
            pan: r.next(-100, 100) as i8,
            glide_ms: if mono { r.next(0, 60000) as u32 } else { 0 },
            legato: mono && r.next(0, 1) == 0,
        },
    }
}

#[test]
fn generated_patches_round_trip() {
    for seed in 0..2000 {
        let patch = generated(seed);
        patch.validate().unwrap();
        let text = patch.to_string();
        let parsed = FmPatch::parse(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(parsed, patch, "{text}");
        assert_eq!(parsed.to_string(), text);
    }
}

// Carriers of algorithms 1..=32, typed from the instrument's algorithm chart
// independently of the table under test.
const CARRIERS: [&[u8]; 32] = [
    &[1, 3],
    &[1, 3],
    &[1, 4],
    &[1, 4],
    &[1, 3, 5],
    &[1, 3, 5],
    &[1, 3],
    &[1, 3],
    &[1, 3],
    &[1, 4],
    &[1, 4],
    &[1, 3],
    &[1, 3],
    &[1, 3],
    &[1, 3],
    &[1],
    &[1],
    &[1],
    &[1, 4, 5],
    &[1, 2, 4],
    &[1, 2, 4, 5],
    &[1, 3, 4, 5],
    &[1, 2, 4, 5],
    &[1, 2, 3, 4, 5],
    &[1, 2, 3, 4, 5],
    &[1, 2, 4],
    &[1, 2, 4],
    &[1, 3, 6],
    &[1, 2, 3, 5],
    &[1, 2, 3, 6],
    &[1, 2, 3, 4, 5],
    &[1, 2, 3, 4, 5, 6],
];
// Feedback operator of each algorithm; 4 and 6 loop through several operators.
const FEEDBACK: [u8; 32] = [
    6, 2, 6, 4, 6, 5, 6, 4, 2, 3, 6, 2, 6, 6, 2, 6, 2, 3, 6, 3, 3, 6, 6, 6, 6, 6, 3, 5, 6, 5, 6, 6,
];

fn edges(routing: &FmRouting) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    for target in 1..=6u8 {
        for source in 1..=6u8 {
            if routing.modulators[target as usize - 1] & (1 << (source - 1)) != 0 {
                out.push((source, target));
            }
        }
    }
    out
}

#[test]
fn algorithms_follow_the_chart() {
    for number in 1..=FM_ALGORITHMS {
        let routing = FmRouting::algorithm(number).unwrap();
        let carriers: Vec<u8> = (1..=6).filter(|&n| routing.is_carrier(n)).collect();
        assert_eq!(
            carriers,
            CARRIERS[number as usize - 1],
            "algorithm {number}"
        );
        assert_eq!(routing.feedback_from, FEEDBACK[number as usize - 1]);
        let looped = match number {
            4 | 6 => 6,
            _ => routing.feedback_from,
        };
        assert_eq!(routing.feedback_to, looped, "algorithm {number}");
        for (source, target) in edges(&routing) {
            assert!(source > target, "algorithm {number}: {source}>{target}");
            assert!(!routing.is_carrier(source), "algorithm {number}");
        }
        // Every operator is heard: it is a carrier or it modulates another.
        for n in 1..=6u8 {
            let modulates = routing.modulators.iter().any(|m| m & (1 << (n - 1)) != 0);
            assert!(
                routing.is_carrier(n) ^ modulates,
                "algorithm {number} op {n}"
            );
        }
    }
    let of = |n| edges(&FmRouting::algorithm(n).unwrap());
    assert_eq!(of(1), [(2, 1), (4, 3), (5, 4), (6, 5)]);
    assert_eq!(of(1), of(2));
    assert_eq!(of(5), [(2, 1), (4, 3), (6, 5)]);
    assert_eq!(of(16), [(2, 1), (3, 1), (5, 1), (4, 3), (6, 5)]);
    assert_eq!(of(18), [(2, 1), (3, 1), (4, 1), (5, 4), (6, 5)]);
    assert_eq!(of(22), [(2, 1), (6, 3), (6, 4), (6, 5)]);
    assert!(of(32).is_empty());
    assert!(FmRouting::algorithm(0).is_err());
    assert_eq!(
        FmRouting::algorithm(33).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert!(!FmRouting::algorithm(1).unwrap().is_carrier(0));
}

#[test]
fn errors_point_at_the_value() {
    use ErrorCode::*;
    at("fm a algorithm=33\nop 1\nend", 1, 16, InvalidArgument);
    at("fm a algorithm=x\nop 1\nend", 1, 16, Syntax);
    at("fm a colour=1\nop 1\nend", 1, 6, Syntax);
    at("fm a feedback=1 feedback=2\nop 1\nend", 1, 17, Syntax);
    at("fm a\n  op 7\nend", 2, 6, InvalidArgument);
    at("fm a\n  op 1\n  op 1\nend", 3, 6, Syntax);
    at("fm a\n  op 1 level=100\nend", 2, 14, InvalidArgument);
    at(
        "fm a\n  op 1 rates=99,99,100,99\nend",
        2,
        20,
        InvalidArgument,
    );
    at("fm a\n  op 1 rates=99,99,99\nend", 2, 14, Syntax);
    at("fm a\n  op 1 ratio=0\nend", 2, 14, InvalidArgument);
    at("fm a\n  op 1 ratio=32\nend", 2, 14, InvalidArgument);
    at("fm a\n  op 1 ratio=1,100\nend", 2, 16, InvalidArgument);
    at("fm a\n  op 1 fixed=50\nend", 2, 14, InvalidArgument);
    at("fm a\n  op 1 ratio=1 fixed=10\nend", 2, 16, Syntax);
    at("fm a\n  op 1 curves=-lin,up\nend", 2, 20, Syntax);
    at("fm a\n  op 1 scaling=39,0\nend", 2, 16, Syntax);
    at("fm a\n  op 1 level\nend", 2, 8, Syntax);
    at("fm a\n  lfo wave=noise\n  op 1\nend", 2, 12, Syntax);
    at("fm a\n  lfo beats=0,4\n  op 1\nend", 2, 13, InvalidArgument);
    at("fm a\n  lfo\n  lfo\n  op 1\nend", 3, 3, Syntax);
    at(
        "fm a\n  pitch levels=50,50,50,100\n  op 1\nend",
        2,
        25,
        InvalidArgument,
    );
    at("fm a\n  pitch\n  pitch\n  op 1\nend", 3, 3, Syntax);
    at("fm a\n  filter cutoff=500\n  op 1\nend", 2, 3, Syntax);
    at("fm a\n  op 1\nend now", 3, 5, Syntax);
    at("fm a\n  op 1\nend\nop 2", 4, 1, Syntax);
    at("fm a\n  op 1", 1, 1, Syntax);
    at("", 1, 1, Syntax);
    at("instrument a wave=saw", 1, 1, Syntax);
    at("fm a.b\nop 1\nend", 1, 5, Syntax);
    at("fm\nop 1\nend", 1, 3, Syntax);
    // Algorithm 1's carriers are operators 1 and 3.
    at("fm a\n  op 2\n  op 6\nend", 1, 1, InvalidArgument);
    at("fm a voices=16 unison=3\nop 1\nend", 1, 23, LimitExceeded);
    at("fm a glide=40\nop 1\nend", 1, 6, InvalidArgument);
    at("fm a legato=on voices=2\nop 1\nend", 1, 6, InvalidArgument);
    assert!(FmPatch::parse("fm a glide=40 legato=on voices=1\nop 1\nend").is_ok());
    assert!(FmPatch::parse("fm a glide=0 legato=off\nop 1\nend").is_ok());

    let long = format!("fm a\nop 1\nend\n;{}", "x".repeat(MAX_FM_TEXT_BYTES));
    at(&long, 1, 1, LimitExceeded);
    let e: audiobox::Error = "fm a\nop 9\nend".parse::<FmPatch>().unwrap_err();
    assert_eq!(e.code, InvalidArgument);
    assert!(e.message.starts_with("line 2, column 4"), "{}", e.message);
}

#[test]
fn typed_values_are_validated() {
    let invalid = |change: fn(&mut Fm)| {
        let mut voice = Fm::default();
        change(&mut voice);
        voice.validate().unwrap_err().code
    };
    use ErrorCode::*;
    assert_eq!(invalid(|v| v.algorithm = 0), InvalidArgument);
    assert_eq!(invalid(|v| v.feedback = 8), InvalidArgument);
    assert_eq!(invalid(|v| v.transpose = 25), InvalidArgument);
    assert_eq!(invalid(|v| v.operators[0] = None), InvalidArgument);
    assert_eq!(
        invalid(|v| v.pitch_envelope.levels[2] = 100),
        InvalidArgument
    );
    assert_eq!(invalid(|v| v.lfo.speed = 100), InvalidArgument);
    assert_eq!(invalid(|v| v.lfo.pitch_sensitivity = 8), InvalidArgument);
    assert_eq!(invalid(|v| v.lfo.beats = Some((65, 1))), InvalidArgument);
    assert_eq!(invalid(|v| v.voices = 17), InvalidArgument);
    assert_eq!(invalid(|v| v.unison = 9), InvalidArgument);
    assert_eq!(invalid(|v| v.unison = 5), LimitExceeded);
    assert_eq!(invalid(|v| v.pan = 101), InvalidArgument);
    assert_eq!(invalid(|v| v.glide_ms = 10), InvalidArgument);
    assert_eq!(invalid(|v| v.legato = true), InvalidArgument);

    let operator = |change: fn(&mut FmOperator)| {
        let mut op = FmOperator::default();
        change(&mut op);
        op.validate().is_err()
    };
    assert!(operator(|o| o.level = 100));
    assert!(operator(|o| o.detune = -8));
    assert!(operator(|o| o.velocity = 8));
    assert!(operator(|o| o.rate_scaling = 8));
    assert!(operator(|o| o.amplitude_sensitivity = 4));
    assert!(operator(|o| o.scaling.break_point = 100));
    assert!(operator(|o| o.envelope.rates[0] = 100));
    assert!(operator(|o| o.frequency = FmFrequency::Ratio {
        coarse: 32,
        fine: 0
    }));
    assert!(operator(
        |o| o.frequency = FmFrequency::Fixed { decade: 4, fine: 0 }
    ));

    let mut patch = FmPatch::parse("fm a\nop 1\nend").unwrap();
    patch.volume = 16;
    assert!(patch.validate().is_err());
    patch.volume = 15;
    patch.name = "two words".into();
    assert!(patch.validate().is_err());
}

#[test]
fn frequencies_convert_as_documented() {
    let ratio = |coarse, fine| FmFrequency::Ratio { coarse, fine }.ratio().unwrap();
    assert_eq!(ratio(0, 0), 0.5);
    assert_eq!(ratio(1, 0), 1.0);
    assert_eq!(ratio(31, 99), 61.69);
    assert_eq!(ratio(2, 50), 3.0);
    let hz = |decade, fine| FmFrequency::Fixed { decade, fine }.fixed_hz().unwrap();
    assert!((hz(0, 0) - 1.0).abs() < 1e-9);
    assert!((hz(3, 0) - 1000.0).abs() < 1e-6);
    assert!((hz(3, 99) - 9772.372).abs() < 1e-2);
}
