//! Finite envelopes, score integration and deterministic drum rendering.
#![cfg(feature = "synth")]
use audiobox::ErrorCode;
use audiobox::synth::*;

const RATE: u32 = 48_000;
const KINDS: [DrumKind; 7] = [
    DrumKind::Kick,
    DrumKind::Snare,
    DrumKind::ClosedHat,
    DrumKind::OpenHat,
    DrumKind::Tom,
    DrumKind::Clap,
    DrumKind::Crash,
];
fn frames(ms: u32, rate: u32) -> usize {
    (ms as u64 * rate as u64 + 500) as usize / 1000
}
fn samples(drum: Drum, velocity: u8) -> Vec<f32> {
    drum.render(RATE, velocity)
        .unwrap()
        .channel_data(0)
        .unwrap()
        .to_vec()
}
fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}
fn options() -> RenderOptions {
    RenderOptions {
        gain: 1.0,
        ..Default::default()
    }
}
fn score_for(drum: Drum, length: Length, velocity: u8) -> Score {
    let mut score = Score::new(120, 48).unwrap();
    let i = score.add_instrument("hit", Instrument::drum(drum)).unwrap();
    score
        .channel("drum")
        .unwrap()
        .instrument(i)
        .unwrap()
        .note_with_velocity(Pitch::C4, length, velocity)
        .unwrap();
    score
}
fn fnv(samples: &[f32]) -> u64 {
    samples
        .iter()
        .flat_map(|v| v.to_bits().to_le_bytes())
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ byte as u64).wrapping_mul(0x100000001b3)
        })
}

#[test]
fn every_preset_has_finite_length_headroom_and_a_decaying_envelope() {
    for kit in [DrumKit::Chiptune, DrumKit::Electronic] {
        for kind in KINDS {
            let drum = Drum::new(kind, kit);
            let out = samples(drum, 127);
            assert_eq!(out.len(), frames(drum.duration_ms(), RATE));
            assert_eq!(out[0], 0.0);
            assert!(out.iter().all(|v| v.is_finite() && v.abs() < 1.5));
            let n = out.len();
            assert!(
                rms(&out[..n / 3]) > rms(&out[n * 2 / 3..]) * 2.0,
                "decay {kind:?}/{kit:?}"
            );
            assert!(out[n - 1].abs() < 0.001, "finite tail {kind:?}/{kit:?}");
            assert_eq!(out, samples(drum, 127), "fixed settings {kind:?}/{kit:?}");
            assert!(samples(drum, 0).iter().all(|&v| v == 0.0));
        }
    }
}

#[test]
fn snare_components_have_separate_finite_envelopes_and_clap_has_bursts() {
    let base = Drum::new(DrumKind::Snare, DrumKit::Electronic);
    let tone = Drum {
        tone_ms: 60,
        noise_ms: 250,
        noise_percent: 0,
        click_percent: 0,
        ..base
    };
    let noise = Drum {
        tone_ms: 60,
        noise_ms: 250,
        noise_percent: 100,
        click_percent: 0,
        ..base
    };
    let mixed = Drum {
        noise_percent: 50,
        ..tone
    };
    let a = samples(tone, 127);
    let b = samples(noise, 127);
    let m = samples(mixed, 127);
    assert_eq!(a.len(), frames(60, RATE));
    assert_eq!(b.len(), frames(250, RATE));
    assert_eq!(m.len(), b.len());
    for n in 0..m.len() {
        let expected = (a.get(n).copied().unwrap_or(0.0) as f64 + b[n] as f64) * 0.5;
        assert!(
            (m[n] as f64 - expected).abs() < 5e-8,
            "independent envelope frame {n}"
        );
    }
    let clap = samples(Drum::new(DrumKind::Clap, DrumKit::Electronic), 127);
    for ms in [0, 10, 20] {
        assert!(rms(&clap[frames(ms + 1, RATE)..frames(ms + 4, RATE)]) > 0.005);
        assert!(
            clap[frames(ms + 8, RATE)..frames(ms + 10, RATE)]
                .iter()
                .all(|&s| s == 0.0)
        );
    }
    assert!(rms(&clap[frames(31, RATE)..frames(50, RATE)]) > 0.005);
}

fn frequency(data: &[f32]) -> f64 {
    let crossings: Vec<f64> = data
        .windows(2)
        .enumerate()
        .filter_map(|(n, p)| {
            let (a, b) = (p[0] as f64, p[1] as f64);
            (a < 0.0 && b >= 0.0).then(|| n as f64 + a / (a - b))
        })
        .collect();
    assert!(crossings.len() > 2);
    (crossings.len() - 1) as f64 * RATE as f64 / (crossings.last().unwrap() - crossings[0])
}

#[test]
fn kick_pitch_sweeps_to_its_base_and_tom_notes_transpose() {
    let kick = Drum {
        pitch_hz: 100,
        sweep_cents: 2400,
        sweep_ms: 200,
        tone_ms: 500,
        noise_percent: 0,
        click_percent: 0,
        ..Drum::new(DrumKind::Kick, DrumKit::Electronic)
    };
    let out = samples(kick, 127);
    let early = frequency(&out[frames(5, RATE)..frames(40, RATE)]);
    let middle = frequency(&out[frames(90, RATE)..frames(140, RATE)]);
    let late = frequency(&out[frames(250, RATE)..frames(400, RATE)]);
    assert!((300.0..400.0).contains(&early), "early pitch {early}");
    assert!((160.0..230.0).contains(&middle), "middle pitch {middle}");
    assert!((late - 100.0).abs() < 0.1, "base pitch {late}");
    let tom = Drum {
        pitch_hz: 130,
        sweep_cents: 0,
        click_percent: 0,
        noise_percent: 0,
        ..Drum::new(DrumKind::Tom, DrumKit::Electronic)
    };
    let base = score_for(tom, Length::QUARTER, 127)
        .render_with(RATE, &options())
        .unwrap();
    assert!(
        (frequency(&base.channel_data(0).unwrap()[frames(50, RATE)..frames(300, RATE)]) - 130.0)
            .abs()
            < 0.1
    );
    // Test the upper pitch separately so a sum cannot hide a missing component.
    let mut upper = Score::new(120, 48).unwrap();
    let i = upper.add_instrument("tom", Instrument::drum(tom)).unwrap();
    upper
        .channel("a")
        .unwrap()
        .instrument(i)
        .unwrap()
        .note(Pitch::new(72).unwrap(), Length::QUARTER)
        .unwrap();
    let out = upper.render_with(RATE, &options()).unwrap();
    assert!(
        (frequency(&out.channel_data(0).unwrap()[frames(50, RATE)..frames(300, RATE)]) - 260.0)
            .abs()
            < 0.1
    );
}

#[test]
fn velocity_is_per_note_linear_and_text_matches_the_builder() {
    let text = "tempo 120\ninstrument k drum=kick kit=electronic drum-pitch=80 drum-sweep=1200,50\nchannel a = @k c8!96 c16!0 c8!127 c16";
    let parsed = Score::parse(text).unwrap();
    let drum = Drum {
        pitch_hz: 80,
        sweep_cents: 1200,
        sweep_ms: 50,
        ..Drum::new(DrumKind::Kick, DrumKit::Electronic)
    };
    let mut built = Score::new(120, 48).unwrap();
    let i = built.add_instrument("k", Instrument::drum(drum)).unwrap();
    built
        .channel("a")
        .unwrap()
        .instrument(i)
        .unwrap()
        .note_with_velocity(Pitch::C4, Length::EIGHTH, 96)
        .unwrap()
        .note_with_velocity(Pitch::C4, Length::SIXTEENTH, 0)
        .unwrap()
        .note(Pitch::C4, Length::EIGHTH)
        .unwrap()
        .note(Pitch::C4, Length::SIXTEENTH)
        .unwrap();
    assert_eq!(parsed, built);
    assert_eq!(
        parsed.channels()[0]
            .notes()
            .iter()
            .map(|n| n.velocity)
            .collect::<Vec<_>>(),
        [96, 0, 127, 127]
    );
    let full = samples(drum, 127);
    let soft = samples(drum, 64);
    for (&a, &b) in full.iter().zip(&soft) {
        assert!((b as f64 - a as f64 * 64.0 / 127.0).abs() < 5e-8);
    }
    let melodic: Score = "instrument p wave=triangle\nchannel a=@p c8!64"
        .parse()
        .unwrap();
    let default: Score = "instrument p wave=triangle\nchannel a=@p c8"
        .parse()
        .unwrap();
    let a = melodic.render_with(RATE, &options()).unwrap();
    let b = default.render_with(RATE, &options()).unwrap();
    for (&a, &b) in a
        .channel_data(0)
        .unwrap()
        .iter()
        .zip(b.channel_data(0).unwrap())
    {
        assert!((a as f64 - b as f64 * 64.0 / 127.0).abs() < 5e-8);
    }
}

#[test]
fn gate_does_not_truncate_a_drum_and_its_final_tail_fits_max_frames() {
    let drum = Drum::new(DrumKind::Crash, DrumKit::Electronic);
    let score: Score = "tempo 999\ninstrument c drum=crash\nchannel a = @c q1 c%1"
        .parse()
        .unwrap();
    let mut synth = Synth::new(&score, RATE, &options()).unwrap();
    let max = synth.max_frames().unwrap();
    let out = score.render_with(RATE, &options()).unwrap();
    let reference = samples(drum, 127);
    assert_eq!(out.channel_data(0).unwrap(), reference);
    assert_eq!(out.frames(), reference.len());
    assert!(out.frames() as u64 <= max);
    let mut buffer = vec![0.0; max as usize];
    assert_eq!(synth.fill(&mut buffer), reference.len());
    assert!(synth.is_finished());
    assert_eq!(synth.fill(&mut buffer), 0);
}

#[test]
fn retrigger_resets_the_hit_and_closed_hat_cuts_an_open_hat_on_its_channel() {
    let score: Score = "tempo 120\ninstrument o drum=open-hat\ninstrument c drum=closed-hat\nchannel a = @o c8 @c c8".parse().unwrap();
    let out = score.render_with(RATE, &options()).unwrap();
    let data = out.channel_data(0).unwrap();
    let open = samples(Drum::new(DrumKind::OpenHat, DrumKit::Electronic), 127);
    let closed = samples(Drum::new(DrumKind::ClosedHat, DrumKit::Electronic), 127);
    let at = frames(250, RATE);
    assert_eq!(&data[..at], &open[..at]);
    assert_eq!(&data[at..at + closed.len()], &closed);
    assert!(data[at + closed.len()..].iter().all(|&v| v == 0.0));
    let score: Score = "tempo 120\ninstrument s drum=snare\nchannel a = @s c8 c8"
        .parse()
        .unwrap();
    let out = score.render_with(RATE, &options()).unwrap();
    let n = samples(Drum::new(DrumKind::Snare, DrumKit::Electronic), 127).len();
    assert_eq!(
        &out.channel_data(0).unwrap()[..n],
        &out.channel_data(0).unwrap()[at..at + n]
    );
}

#[test]
fn loop_points_do_not_invent_drum_attacks_for_gate_carries() {
    // The second channel puts a loop marker inside a long, already-finished hit.
    let text =
        "tempo 120\ninstrument h drum=closed-hat\nchannel drum = @h c2\nchannel grid = r4 L r4";
    let score: Score = text.parse().unwrap();
    let opts = RenderOptions {
        loops: Some(2),
        ..options()
    };
    let out = score.render_with(RATE, &opts).unwrap();
    let hit = samples(Drum::new(DrumKind::ClosedHat, DrumKit::Electronic), 127);
    let data = out.channel_data(0).unwrap();
    assert_eq!(&data[..hit.len()], &hit);
    assert!(data[hit.len()..].iter().all(|&v| v == 0.0));
}

fn pattern(kit: &str) -> String {
    format!(
        "tempo 120
instrument k drum=kick kit={kit}
instrument s drum=snare kit={kit} drum-seed=123
instrument h drum=closed-hat kit={kit}
instrument o drum=open-hat kit={kit}
instrument t drum=tom kit={kit}
instrument c drum=clap kit={kit}
instrument x drum=crash kit={kit}
channel kick = @k L [c8!127 r8 c8!80 r8]2
channel snare = @s L [r4 c4!100]2
channel hats = L [@h c8!90 c8!50 @o c8!80 r8]2
channel tom = @t L r2 c8 d8 e8 g8
channel clap = @c L r2 c4!90 r4
channel crash = @x L c1!90"
    )
}

#[test]
fn both_kits_are_pinned_chunk_invariant_and_resettable() {
    let mut hashes = Vec::new();
    for kit in ["chiptune", "electronic"] {
        let score: Score = pattern(kit).parse().unwrap();
        let opts = RenderOptions {
            loops: Some(1),
            gain: 0.15,
            ..Default::default()
        };
        let expected = score.render_with(RATE, &opts).unwrap();
        let data = expected.channel_data(0).unwrap();
        let mut synth = Synth::new(&score, RATE, &opts).unwrap();
        let mut chunked = Vec::new();
        let mut buffer = [0.0; 511];
        for n in 0usize.. {
            let frames = [1, 17, 255, 2, 511, 37][n % 6];
            let got = synth.fill(&mut buffer[..frames]);
            chunked.extend_from_slice(&buffer[..got]);
            if got == 0 {
                break;
            }
        }
        assert_eq!(data, chunked);
        synth.reset();
        assert_eq!(synth.fill(&mut buffer), buffer.len());
        assert_eq!(&buffer[..], &data[..buffer.len()]);
        hashes.push(fnv(data));
    }
    assert_eq!(
        hashes,
        [245_343_399_786_689_487, 17_178_961_377_690_921_222]
    );
}

#[test]
fn seed_variations_and_low_rate_short_envelopes_are_bounded() {
    for kit in [DrumKit::Chiptune, DrumKit::Electronic] {
        let d = Drum::new(DrumKind::Snare, kit);
        assert_ne!(samples(d, 127), samples(Drum { seed: 99, ..d }, 127));
        for rate in [1000, 1650, 11_025, 44_100] {
            for kind in KINDS {
                let drum = Drum {
                    tone_ms: 1,
                    noise_ms: 1,
                    ..Drum::new(kind, kit)
                };
                let out = drum.render(rate, 127).unwrap();
                assert!(out.channel_data(0).unwrap().iter().all(|v| v.is_finite()));
                assert!(out.frames().abs_diff(frames(drum.duration_ms(), rate)) <= 1);
            }
        }
    }
}

#[test]
fn a_channel_can_switch_from_drums_to_melody_and_back() {
    let score: Score = "tempo 120\ninstrument s drum=snare\ninstrument p wave=triangle\nchannel a=@s c16 @p a16 @s c16".parse().unwrap();
    let out = score.render_with(RATE, &options()).unwrap();
    let data = out.channel_data(0).unwrap();
    let interval = frames(125, RATE);
    assert_eq!(&data[..interval], &data[interval * 2..interval * 3]);
    assert!(rms(&data[interval..interval * 2]) > 0.2);
    assert!(
        rms(&data[interval * 3..]) > 0.001,
        "the final drum keeps its tail"
    );
}

#[test]
fn parser_errors_and_builder_validation_cover_velocity_and_drum_settings() {
    for (text, code) in [
        ("instrument k drum=unknown", ErrorCode::Syntax),
        ("instrument k kit=chiptune", ErrorCode::InvalidArgument),
        (
            "instrument k drum=kick adsr=1,2,3,4",
            ErrorCode::InvalidArgument,
        ),
        (
            "instrument k drum=kick wave=triangle",
            ErrorCode::InvalidArgument,
        ),
        (
            "instrument k drum=kick drum-pitch=0",
            ErrorCode::InvalidArgument,
        ),
        (
            "instrument k drum=kick drum-mix=101",
            ErrorCode::InvalidArgument,
        ),
        ("instrument k drum=kick drum-sweep=20", ErrorCode::Syntax),
        (
            "instrument k drum=kick\nchannel a=@k c8!128",
            ErrorCode::InvalidArgument,
        ),
        (
            "instrument k drum=kick\nchannel a=@k c8!",
            ErrorCode::Syntax,
        ),
        (
            "instrument k drum=kick\nchannel a=@k r8!80",
            ErrorCode::Syntax,
        ),
        (
            "instrument k drum=kick\nchannel a=@k c8!80&c8!90",
            ErrorCode::InvalidArgument,
        ),
    ] {
        let e = Score::parse(text).unwrap_err();
        assert_eq!(e.code, code, "{text}: {e}");
        assert!(e.line > 0 && e.column > 0);
    }
    let base = Drum::new(DrumKind::Kick, DrumKit::Electronic);
    assert!(
        Drum {
            noise_ms: 10_001,
            ..base
        }
        .render(RATE, 127)
        .is_err()
    );
    assert!(base.render(RATE, 128).is_err());
    let mut score = Score::new(120, 48).unwrap();
    assert!(
        score
            .add_instrument(
                "bad",
                Instrument {
                    envelope: Envelope {
                        attack_ms: 1,
                        ..Default::default()
                    },
                    ..Instrument::drum(base)
                }
            )
            .is_err()
    );
    let i = score.add_instrument("k", Instrument::drum(base)).unwrap();
    assert!(
        score
            .channel("a")
            .unwrap()
            .instrument(i)
            .unwrap()
            .note_with_velocity(Pitch::C4, Length::QUARTER, 128)
            .is_err()
    );
    let tied = Score::parse("instrument k drum=kick\nchannel a=@k c8!80&c8").unwrap();
    assert_eq!(tied.channels()[0].notes().len(), 1);
    assert_eq!(tied.channels()[0].notes()[0].velocity, 80);
}
