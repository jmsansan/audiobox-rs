#![cfg(feature = "synth")]
//! Velocity sensitivity, keyboard level scaling and rate scaling of FM operators,
//! checked against the tables and formulas in `docs/fm-voice.md`.
use audiobox::synth::*;

const RATE: u32 = 48000;

fn mono(text: &str) -> Vec<f32> {
    let options = RenderOptions {
        gain: 1.0,
        ..RenderOptions::default()
    };
    let score = Score::parse(text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let audio = score.render_with(RATE, &options).unwrap();
    audio.channel_data(0).unwrap().to_vec()
}
/// Peak over 10 ms starting at `seconds`.
fn at(samples: &[f32], seconds: f64) -> f64 {
    let start = (seconds * RATE as f64) as usize;
    samples[start..start + 480]
        .iter()
        .fold(0.0, |m, &x| m.max(x.abs() as f64))
}
/// A table written in the contract as `NAME = 1 2 3 ...` over several lines.
fn table(name: &str) -> Vec<i32> {
    let doc = include_str!("../docs/fm-voice.md");
    let start = doc.find(&format!("\n{name} = ")).expect("table") + name.len() + 4;
    doc[start..]
        .lines()
        .take_while(|line| !line.trim().is_empty() && !line.starts_with("```"))
        .flat_map(|line| line.split_whitespace())
        .map(|n| n.parse().unwrap())
        .collect()
}
fn scale(level: i32) -> i32 {
    if level >= 20 {
        28 + level
    } else {
        [
            0, 5, 9, 13, 17, 20, 23, 25, 27, 29, 31, 33, 35, 37, 39, 41, 42, 43, 45, 46,
        ][level as usize]
    }
}
/// Peak of a carrier at envelope level 99 whose 0..=127 output level is `out`, with
/// a velocity offset in steps.
fn carrier(out: i32, offset: i32) -> f64 {
    let base = (32 * out.min(127) + offset).max(0);
    let steps = (64 * 63 + base - 4256).max(16);
    if steps <= 16 {
        0.0
    } else {
        FM_CARRIER_PEAK * 2f64.powf((steps - 3840) as f64 / 256.0)
    }
}
fn velocity_offset(sensitivity: i32, velocity: i32) -> i32 {
    let v = table("V");
    assert_eq!(v.len(), 64);
    ((sensitivity * (v[(velocity >> 1) as usize] - 239) + 7) >> 3) << 4
}

#[test]
fn velocity_changes_an_operator_by_its_sensitivity() {
    for sensitivity in [0, 1, 4, 7] {
        for velocity in [1, 40, 64, 100, 127] {
            let samples = mono(&format!(
                "tempo 60\nfm s\nop 1 fixed=1000 level=90 velocity={sensitivity}\nend\nchannel a=@s c2!{velocity}"
            ));
            let expected = carrier(scale(90), velocity_offset(sensitivity, velocity));
            assert!(
                (at(&samples, 1.0) - expected).abs() < expected * 1e-3 + 1e-6,
                "sensitivity {sensitivity} velocity {velocity}: {} vs {expected}",
                at(&samples, 1.0)
            );
        }
    }
    // Sensitivity 7 at full velocity exceeds full scale by 224 steps.
    assert_eq!(velocity_offset(7, 127), 224);
    assert_eq!(velocity_offset(0, 1), 0);
    let loud = mono("tempo 60\nfm s\nop 1 fixed=1000 velocity=7\nend\nchannel a=@s c2");
    assert!((at(&loud, 1.0) - 0.5 * 2f64.powf(224.0 / 256.0)).abs() < 1e-3);

    // On a modulator, velocity moves the sidebands and leaves the carrier's level.
    let side = |velocity: u8| {
        let samples = mono(&format!(
            "tempo 60\nfm s algorithm=1\nop 1 fixed=1000\nop 2 fixed=100 level=70 velocity=7\nend\nchannel a=@s c1!{velocity}"
        ));
        let body = &samples[RATE as usize..2 * RATE as usize];
        let component = |hz: f64| {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &x) in body.iter().enumerate() {
                let angle = std::f64::consts::TAU * hz * i as f64 / RATE as f64;
                re += x as f64 * angle.cos();
                im += x as f64 * angle.sin();
            }
            2.0 * (re * re + im * im).sqrt() / body.len() as f64
        };
        (component(1100.0), at(&samples, 1.0))
    };
    let levels: Vec<(f64, f64)> = [20, 60, 90, 127].map(side).to_vec();
    assert!(
        levels.windows(2).all(|w| w[1].0 > w[0].0 * 1.3),
        "{levels:?}"
    );
    assert!(
        levels.iter().all(|l| (l.1 - 0.5).abs() < 0.01),
        "{levels:?}"
    );
}

fn level_scaling(note: i32, break_point: i32, depth: i32, curve: &str) -> i32 {
    let e = table("E");
    assert_eq!(e.len(), 33);
    let group = (note - break_point - 17).abs() / 3;
    let amount = if curve.ends_with("lin") {
        (group * depth * 329) >> 12
    } else {
        (e[group.min(32) as usize] * depth * 329) >> 15
    };
    if curve.starts_with('-') {
        -amount
    } else {
        amount
    }
}

#[test]
fn keyboard_level_scaling_follows_each_curve_on_both_sides() {
    // Octave 1..7 C notes are MIDI 24, 36, ... 96; the break point 39 is MIDI 60.
    for curve in ["-lin", "-exp", "+exp", "+lin"] {
        for (octave, note) in [
            (1, 24),
            (2, 36),
            (3, 48),
            (4, 60),
            (5, 72),
            (6, 84),
            (7, 96),
        ] {
            for (level, depth) in [(70, 40), (85, 99)] {
                let samples = mono(&format!(
                    "tempo 60\nfm s\nop 1 fixed=1000 level={level} scaling=39,{depth},{depth} curves={curve},{curve}\nend\nchannel a=@s o{octave} c2"
                ));
                let out = scale(level) + level_scaling(note, 39, depth, curve);
                let expected = carrier(out, 0);
                assert!(
                    (at(&samples, 1.0) - expected).abs() < expected * 1e-3 + 1e-6,
                    "{curve} note {note} level {level} depth {depth}: {} vs {expected}",
                    at(&samples, 1.0)
                );
            }
        }
    }
    // The two sides are independent, and the level clamps at the top.
    let side = |scaling: &str, octave: u8| {
        at(
            &mono(&format!(
                "tempo 60\nfm s\nop 1 fixed=1000 level=80 {scaling}\nend\nchannel a=@s o{octave} c2"
            )),
            1.0,
        )
    };
    let flat = side("", 4);
    assert_eq!(side("scaling=39,50,0", 7), flat);
    assert!(side("scaling=39,50,0", 1) < flat * 0.5);
    assert_eq!(side("scaling=39,0,50", 1), flat);
    assert!(side("scaling=39,0,50", 7) < flat * 0.5);
    assert!((side("scaling=39,0,99 curves=-lin,+lin", 7) - carrier(127, 0)).abs() < 1e-3);
    // A break point moves the curve; transpose moves the note it is measured from.
    assert!(side("scaling=75,0,50", 7) > side("scaling=39,0,50", 7) * 2.0);
    let transposed = at(
        &mono(
            "tempo 60\nfm s transpose=12\nop 1 fixed=1000 level=80 scaling=39,0,50\nend\nchannel a=@s o5 c2",
        ),
        1.0,
    );
    assert_eq!(transposed, side("scaling=39,0,50", 6));
    // The flat zone is centred four semitones under the break point's own note.
    assert_eq!(level_scaling(56, 39, 99, "-lin"), 0);
    assert_eq!(level_scaling(60, 39, 99, "-lin"), -7);
    assert_eq!(level_scaling(96, 39, 99, "-lin"), -103);
}

#[test]
fn rate_scaling_shortens_envelopes_on_higher_notes() {
    let slope = |q: i32| 0.2819 * (1.0 + 0.25 * (q & 3) as f64) * 2f64.powi(q >> 2);
    let base = (40 * 41) >> 6;
    for sensitivity in [0, 3, 7] {
        for (octave, note) in [(1, 24), (4, 60), (7, 96)] {
            let samples = mono(&format!(
                "tempo 60\nfm s\nop 1 fixed=1000 rates=99,99,99,40 rate-scaling={sensitivity}\nend\nchannel a=@s q8 o{octave} c4"
            ));
            let delta = (sensitivity * (note / 3 - 7).clamp(0, 31)) >> 3;
            let expected = slope((base + delta).min(63));
            let span = (30.0 / expected).min(1.0);
            let measured = 20.0 * (at(&samples, 1.02) / at(&samples, 1.02 + span)).log10() / span;
            assert!(
                (measured - expected).abs() < expected * 0.02,
                "sensitivity {sensitivity} note {note}: {measured} vs {expected} dB/s"
            );
        }
    }
}

#[test]
fn a_legato_note_keeps_the_levels_of_the_note_that_attacked() {
    let line = |notes: &str| {
        mono(&format!(
            "tempo 60\nfm s voices=1 legato=on\nop 1 fixed=1000 velocity=7 scaling=39,0,60\nend\nchannel a=@s q8 {notes}"
        ))
    };
    let tied = line("o4 l2 c!127 >c!20");
    assert!((at(&tied, 3.0) - at(&tied, 1.0)).abs() < 1e-6);
    // Without legato the second note takes its own velocity and scaling.
    let separate = mono(
        "tempo 60\nfm s voices=1\nop 1 fixed=1000 velocity=7 scaling=39,0,60\nend\nchannel a=@s q8 o4 l2 c!127 >c!20",
    );
    assert!(at(&separate, 3.0) < at(&separate, 1.0) * 0.2);
    // Patches that leave all three at their defaults are untouched by velocity.
    let plain = |v: u8| {
        mono(&format!(
            "tempo 60\nfm s\nop 1\nend\nchannel a=@s o4 a2!{v}"
        ))
    };
    assert_eq!(plain(10), plain(127));
}
