#![cfg(feature = "synth")]
use audiobox::synth::*;
use audiobox::{Audio, ErrorCode};
use std::f64::consts::TAU;

const RATE: u32 = 48000;

fn options() -> RenderOptions {
    RenderOptions {
        gain: 1.0,
        ..RenderOptions::default()
    }
}
fn mono(text: &str) -> Vec<f32> {
    let audio = Score::parse(text)
        .unwrap_or_else(|e| panic!("{e}\n{text}"))
        .render_with(RATE, &options())
        .unwrap();
    audio.channel_data(0).unwrap().to_vec()
}
fn stereo(text: &str) -> Audio {
    Score::parse(text)
        .unwrap()
        .render_stereo_with(RATE, &options())
        .unwrap()
}
/// Amplitude of the component at `hz`, with a Hann window.
fn level(samples: &[f32], hz: f64) -> f64 {
    let n = samples.len() as f64;
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &x) in samples.iter().enumerate() {
        let window = 0.5 - 0.5 * (TAU * i as f64 / n).cos();
        let angle = TAU * hz * i as f64 / RATE as f64;
        re += x as f64 * window * angle.cos();
        im += x as f64 * window * angle.sin();
    }
    4.0 * (re * re + im * im).sqrt() / n
}
fn peak(samples: &[f32]) -> f64 {
    samples.iter().fold(0.0, |m, &x| m.max(x.abs() as f64))
}
fn bessel(order: i32, x: f64) -> f64 {
    let mut sum = 0.0;
    let mut term = (1..=order).fold(1.0, |t, k| t * x / 2.0 / k as f64);
    for k in 0..40 {
        sum += term;
        term *= -(x / 2.0) * (x / 2.0) / ((k + 1) * (k + 1 + order)) as f64;
    }
    sum
}
/// Linear gain of an operator at envelope level 99 with this output level.
fn gain(output_level: i32) -> f64 {
    2f64.powf((64 * 63 + 32 * (28 + output_level) - 4256 - 3840) as f64 / 256.0)
}

#[test]
fn one_carrier_is_a_sine_at_half_scale() {
    let samples = mono("tempo 60\nfm s\nop 1\nend\nchannel a=@s o4 a1");
    let body = &samples[RATE as usize..2 * RATE as usize];
    assert!((level(body, 440.0) - FM_CARRIER_PEAK).abs() < 1e-4);
    assert!((peak(body) - FM_CARRIER_PEAK).abs() < 1e-4);
    for hz in [220.0, 880.0, 1320.0, 3000.0] {
        assert!(level(body, hz) < 1e-6, "{hz}");
    }
    // Frequency follows the ratio, its fine part, transpose and the octave.
    let at = |patch: &str, hz: f64| {
        let samples = mono(&format!("tempo 60\n{patch}\nchannel a=@s o4 a1"));
        let body = &samples[RATE as usize..2 * RATE as usize];
        assert!(
            (level(body, hz) - FM_CARRIER_PEAK).abs() < 1e-3,
            "{patch} at {hz}"
        );
    };
    at("fm s\nop 1 ratio=2\nend", 880.0);
    at("fm s\nop 1 ratio=0.5\nend", 220.0);
    at("fm s\nop 1 ratio=1,50\nend", 660.0);
    at("fm s transpose=12\nop 1\nend", 880.0);
    at("fm s transpose=-12\nop 1 ratio=3\nend", 660.0);
    at("fm s\nop 1 fixed=1000\nend", 1000.0);
    at("fm s transpose=7\nop 1 fixed=100,50\nend", 316.227766);
    at(
        "fm s\nop 1 detune=7\nend",
        440.0 * 2f64.powf(7.0 * 12606.0 / 16_777_216.0),
    );
}

#[test]
fn sidebands_follow_the_bessel_functions() {
    // sin(wc t + I sin(wm t)) has components J_n(I) at wc +- n wm.
    for (modulator, ratio) in [(50, 100.0), (71, 100.0), (85, 100.0), (99, 10.0)] {
        let index = 2.0 * TAU * gain(modulator);
        let fixed = if ratio == 100.0 { "100" } else { "10" };
        let samples = mono(&format!(
            "tempo 60\nfm s algorithm=1\nop 1 fixed=1000\nop 2 fixed={fixed} level={modulator}\nend\nchannel a=@s c1"
        ));
        let body = &samples[RATE as usize..3 * RATE as usize];
        for order in 0..6 {
            let expected = FM_CARRIER_PEAK * bessel(order, index).abs();
            for side in [-1.0, 1.0] {
                let hz = 1000.0 + side * order as f64 * ratio;
                assert!(
                    (level(body, hz) - expected).abs() < 2e-4,
                    "level {modulator} index {index:.3} order {order}: {} vs {expected}",
                    level(body, hz)
                );
            }
        }
    }
    assert!((2.0 * TAU * gain(99) - 4.0 * std::f64::consts::PI).abs() < 1e-12);
}

// An independent, slow renderer built from the documented algorithm table.
struct Chart {
    edges: Vec<(usize, usize)>,
    carriers: Vec<usize>,
    feedback: (usize, usize),
}
fn chart() -> Vec<Chart> {
    let doc = include_str!("../docs/fm-voice.md");
    let mut rows = Vec::new();
    for line in doc.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() != 6 || cells[1].parse::<usize>().is_err() {
            continue;
        }
        let mut edges = Vec::new();
        for chain in cells[2]
            .split(',')
            .map(str::trim)
            .filter(|c| c.contains('>'))
        {
            let groups: Vec<Vec<usize>> = chain
                .split('>')
                .map(|g| g.split('+').map(|n| n.parse().unwrap()).collect())
                .collect();
            for pair in groups.windows(2) {
                for &source in &pair[0] {
                    for &target in &pair[1] {
                        edges.push((source, target));
                    }
                }
            }
        }
        let feedback = match cells[4].split_once(" into ") {
            Some((from, to)) => (from.parse().unwrap(), to.parse().unwrap()),
            None => (cells[4].parse().unwrap(), cells[4].parse().unwrap()),
        };
        rows.push(Chart {
            edges,
            carriers: cells[3]
                .split(',')
                .map(|n| n.trim().parse().unwrap())
                .collect(),
            feedback,
        });
    }
    assert_eq!(rows.len(), 32);
    rows
}
const HZ: [f64; 6] = [440.0, 1000.0, 660.0, 100.0, 1320.0, 10.0];
const LEVELS: [i32; 6] = [99, 80, 92, 76, 85, 70];
fn chart_patch(algorithm: usize, feedback: u8, off: Option<usize>) -> String {
    let source = [
        "ratio=1",
        "fixed=1000",
        "ratio=1,50",
        "fixed=100",
        "ratio=3",
        "fixed=10",
    ];
    let mut text = format!("tempo 60\nfm s algorithm={algorithm} feedback={feedback}\n");
    for op in (1..=6).filter(|&op| Some(op) != off) {
        text += &format!("op {op} {} level={}\n", source[op - 1], LEVELS[op - 1]);
    }
    text + "end\nchannel a=@s o4 a1"
}
fn reference(row: &Chart, feedback: u8, off: Option<usize>, frames: usize) -> Vec<f64> {
    let on = |op: usize| Some(op) != off;
    let mut levels = [0.0f64; 6];
    let mut phases = [0.0f64; 6];
    let mut memory = [0.0f64; 2];
    let scale = if feedback == 0 {
        0.0
    } else {
        2f64.powi(feedback as i32 - 8)
    };
    let mut out = Vec::with_capacity(frames);
    for _ in 0..frames {
        let mut outputs = [0.0f64; 6];
        let mut sum = 0.0;
        for op in (1..=6).rev().filter(|&op| on(op)) {
            // Default envelope: rate 99 up to the level, then hold.
            let target = (64 * 63 + 32 * (28 + LEVELS[op - 1]) - 4256) as f64;
            let l = &mut levels[op - 1];
            if *l < target {
                *l = l.max(1716.0);
                *l +=
                    (17.0 - (*l / 256.0).floor()) * 49096.0 * 7.0 * 32768.0 / 16384.0 / RATE as f64;
                *l = l.min(target);
            }
            let mut modulation: f64 = row
                .edges
                .iter()
                .filter(|(_, target)| *target == op)
                .map(|(source, _)| outputs[source - 1])
                .sum();
            if row.feedback.1 == op {
                modulation += (memory[0] + memory[1]) * 0.5 * scale;
            }
            let y = (TAU * (phases[op - 1] + modulation)).sin()
                * 2f64.powf((*l - 3840.0) / 256.0)
                * 2.0;
            outputs[op - 1] = y;
            phases[op - 1] += HZ[op - 1] / RATE as f64;
            if row.carriers.contains(&op) {
                sum += y;
            }
        }
        memory = [memory[1], outputs[row.feedback.0 - 1]];
        out.push(sum * FM_CARRIER_PEAK / 2.0);
    }
    out
}

#[test]
fn every_algorithm_matches_a_reference_built_from_the_documented_table() {
    let frames = 4000;
    for (index, row) in chart().iter().enumerate() {
        let algorithm = index + 1;
        for (feedback, off) in [(0, None), (5, None), (7, None), (5, Some(6)), (5, Some(2))] {
            let audible = row.carriers.iter().any(|&c| Some(c) != off);
            if !audible {
                continue;
            }
            let samples = mono(&chart_patch(algorithm, feedback, off));
            let expected = reference(row, feedback, off, frames);
            for (i, (&got, want)) in samples.iter().zip(&expected).enumerate() {
                assert!(
                    (got as f64 - want).abs() < 2e-5,
                    "algorithm {algorithm} feedback {feedback} off {off:?} frame {i}: {got} vs {want}"
                );
            }
            assert!(samples.iter().all(|x| x.is_finite()));
        }
        // The table under test is the one in the crate.
        let routing = FmRouting::algorithm(algorithm as u8).unwrap();
        assert_eq!(
            (routing.feedback_from as usize, routing.feedback_to as usize),
            row.feedback
        );
        for op in 1..=6u8 {
            assert_eq!(
                routing.is_carrier(op),
                row.carriers.contains(&(op as usize))
            );
            for source in 1..=6usize {
                let edge = routing.modulators[op as usize - 1] & (1 << (source - 1)) != 0;
                assert_eq!(edge, row.edges.contains(&(source, op as usize)));
            }
        }
    }
}

#[test]
fn a_modulator_switched_off_removes_only_its_sidebands() {
    // Algorithm 5: 2>1, 4>3, 6>5. Carriers at 1, 3 and 5 kHz-ish, modulators far apart.
    let patch = |skip: usize| {
        let lines = [
            "op 1 fixed=1000",
            "op 2 fixed=100 level=75",
            "op 3 fixed=1000,30",
            "op 4 fixed=10,50 level=75",
            "op 5 fixed=1000,60",
            "op 6 fixed=100,25 level=75",
        ];
        let body: Vec<&str> = lines
            .iter()
            .enumerate()
            .filter(|(i, _)| i + 1 != skip)
            .map(|(_, l)| *l)
            .collect();
        format!(
            "tempo 60\nfm s algorithm=5\n{}\nend\nchannel a=@s c1",
            body.join("\n")
        )
    };
    let hz = |decade: f64, fine: f64| 10f64.powf(decade + fine / 100.0);
    let pairs = [
        (hz(3.0, 0.0), hz(2.0, 0.0)),
        (hz(3.0, 30.0), hz(1.0, 50.0)),
        (hz(3.0, 60.0), hz(2.0, 25.0)),
    ];
    let measure = |skip: usize| {
        let samples = mono(&patch(skip));
        let body = samples[RATE as usize..3 * RATE as usize].to_vec();
        pairs.map(|(c, m)| (level(&body, c), level(&body, c + m)))
    };
    let all = measure(0);
    for (stack, modulator) in [2, 4, 6].into_iter().enumerate() {
        let without = measure(modulator);
        for other in 0..3 {
            if other == stack {
                assert!((without[other].0 - FM_CARRIER_PEAK).abs() < 1e-3);
                assert!(without[other].1 < 1e-5 && all[other].1 > 0.05);
            } else {
                assert!((without[other].0 - all[other].0).abs() < 1e-5);
                assert!((without[other].1 - all[other].1).abs() < 1e-5);
            }
        }
    }
}

#[test]
fn feedback_adds_harmonics_and_stays_bounded() {
    let harmonics = |feedback: u8| {
        let samples = mono(&format!(
            "tempo 60\nfm s algorithm=32 feedback={feedback}\nop 6 fixed=100\nend\nchannel a=@s c1"
        ));
        let body = samples[RATE as usize..3 * RATE as usize].to_vec();
        assert!(peak(&body) <= FM_CARRIER_PEAK + 1e-6);
        let power = body.iter().map(|&x| x as f64 * x as f64).sum::<f64>() / body.len() as f64;
        let fundamental = level(&body, 100.0);
        (
            fundamental,
            (power - fundamental * fundamental / 2.0).max(0.0),
        )
    };
    let mut last = harmonics(0);
    assert!((last.0 - FM_CARRIER_PEAK).abs() < 1e-4 && last.1 < 1e-5);
    // Up to 5 the tone gains harmonics steadily; 6 and 7 at full level are noise-like.
    for feedback in 1..=7 {
        let now = harmonics(feedback);
        if feedback <= 5 {
            assert!(
                now.1 > last.1 * 1.5 && now.0 < last.0,
                "feedback {feedback}: {now:?} after {last:?}"
            );
        }
        assert!(now.1 > 1e-7);
        last = now;
    }
    // A loop through several operators (4 into 6, 5 into 6) is rendered too.
    for (algorithm, ops) in [
        (4, "op 4\nop 5 level=80\nop 6 level=80"),
        (6, "op 5\nop 6 level=80"),
    ] {
        let with = |feedback: u8| {
            mono(&format!(
                "tempo 60\nfm s algorithm={algorithm} feedback={feedback}\n{ops}\nend\nchannel a=@s o3 a1"
            ))
        };
        let (dry, wet) = (with(0), with(7));
        assert!(wet.iter().all(|x| x.is_finite() && x.abs() <= 0.5001));
        assert!(dry.iter().zip(&wet).any(|(a, b)| (a - b).abs() > 0.01));
    }
}

/// Peak in a short window starting at `seconds`.
fn at(samples: &[f32], seconds: f64) -> f64 {
    let start = (seconds * RATE as f64) as usize;
    peak(&samples[start..start + 480])
}
fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}
fn decay_slope(rate: i32) -> f64 {
    let q = (rate * 41) >> 6;
    0.2819 * (1.0 + 0.25 * (q & 3) as f64) * 2f64.powi(q >> 2)
}

#[test]
fn falling_stages_are_linear_in_decibels_at_the_documented_slope() {
    // A fixed 1 kHz carrier; the gate is one second at tempo 60 (quarter, full gate).
    for rate in [30, 40, 50, 57] {
        let samples = mono(&format!(
            "tempo 60\nfm s\nop 1 fixed=1000 rates=99,99,99,{rate}\nend\nchannel a=@s q8 c4"
        ));
        let slope = decay_slope(rate);
        let span = (20.0 / slope).min(1.0);
        let measured = db(at(&samples, 1.0 + 0.2 * span) / at(&samples, 1.0 + 0.2 * span + span));
        assert!(
            (measured / span - slope).abs() < slope * 0.02,
            "rate {rate}: {} vs {slope} dB/s",
            measured / span
        );
        // The tail ends once the carrier reaches its last level (90 dB down).
        let tail = (samples.len() as f64 / RATE as f64 - 1.0) * slope;
        assert!(
            (85.0..95.0).contains(&tail),
            "rate {rate}: {tail} dB of tail"
        );
    }
}

#[test]
fn stages_reach_their_levels_and_note_off_releases_from_the_current_level() {
    let patch = "fm s\nop 1 fixed=1000 rates=99,45,40,50 levels=99,80,60,0\nend";
    // Held four seconds: full scale, then 80, then 60.
    let held = mono(&format!("tempo 60\n{patch}\nchannel a=@s q8 c1"));
    let target = |l: i32| {
        FM_CARRIER_PEAK * 2f64.powf((64 * ((28 + l) >> 1) + 4064 - 4256 - 3840) as f64 / 256.0)
    };
    assert!(at(&held, 0.002) > FM_CARRIER_PEAK * 0.94);
    assert!((at(&held, 3.9) - target(60)).abs() < target(60) * 0.01);
    // Level 99 to 80 is 576 steps at rate 45; 80 to 60 is another 640 at rate 40.
    let seconds = |steps: f64, rate: i32| steps * 6.0206 / 256.0 / decay_slope(rate);
    let (first, second) = (seconds(576.0, 45), seconds(640.0, 40));
    assert!((at(&held, first) - target(80)).abs() < target(80) * 0.03);
    assert!(at(&held, first * 0.5) > target(80) * 1.5);
    assert!((at(&held, first + second + 0.02) - target(60)).abs() < target(60) * 0.02);
    assert!(at(&held, first + second * 0.5) > target(60) * 1.5);

    // Note-off in the middle of the first decay: no jump, and the release slope applies.
    let short = mono(&format!("tempo 60\n{patch}\nchannel a=@s q8 c16"));
    let off = 0.25;
    assert!(off < first, "the note must end inside the first decay");
    let before = at(&short, off - 0.011);
    let after = at(&short, off);
    assert!(
        after < before && after > before * 0.9,
        "{before} then {after}"
    );
    let measured = db(at(&short, off + 0.05) / at(&short, off + 0.25)) / 0.2;
    assert!((measured - decay_slope(50)).abs() < decay_slope(50) * 0.02);
}

#[test]
fn rising_stages_jump_then_slow_towards_the_target() {
    let samples = mono(
        "tempo 60\nfm s\nop 1 fixed=1000 rates=35,99,99,99 levels=99,99,99,0\nend\nchannel a=@s q8 c1",
    );
    // The first samples are already at the jump level, about 50 dB down.
    let jump = FM_CARRIER_PEAK * 2f64.powf((1716.0 - 3840.0) / 256.0);
    assert!(peak(&samples[..96]) > jump * 0.9 && peak(&samples[..96]) < jump * 1.5);
    let envelope: Vec<f64> = (0..300).map(|i| at(&samples, i as f64 * 0.01)).collect();
    assert!(envelope.windows(2).all(|w| w[1] >= w[0] * 0.999));
    assert!((envelope[299] - FM_CARRIER_PEAK).abs() < 1e-3);
    // In decibels the attack is fastest at the start: 11 times the decay slope at the
    // jump, twice at full scale.
    let slope = decay_slope(35);
    let early = db(envelope[2] / envelope[1]) / 0.01;
    assert!((9.0..11.2).contains(&(early / slope)), "{}", early / slope);
    let full = envelope
        .iter()
        .position(|&l| l > FM_CARRIER_PEAK * 0.999)
        .unwrap();
    let late = db(envelope[full - 1] / envelope[full - 3]) / 0.02;
    assert!((1.8..3.2).contains(&(late / slope)), "{}", late / slope);
}

#[test]
fn a_modulator_envelope_moves_the_spectrum() {
    // Bell: the modulator falls to silence, so the tone decays towards a sine.
    let bell = mono(
        "tempo 60\nfm s algorithm=1\nop 1 fixed=1000\nop 2 fixed=100,50 level=85 rates=99,45,99,99 levels=99,0,0,0\nend\nchannel a=@s q8 c1",
    );
    let side = 1000.0 + 10f64.powf(2.5);
    let window = |start: f64| &bell[(start * RATE as f64) as usize..][..RATE as usize / 2];
    let (early, late) = (window(0.0), window(3.0));
    assert!(level(early, side) > 0.05);
    assert!(level(late, side) < 1e-5);
    assert!((level(late, 1000.0) - FM_CARRIER_PEAK).abs() < 1e-3);
    assert!(level(early, 1000.0) < level(late, 1000.0) * 0.9);

    // Brass: the modulator rises slowly, so the attack brightens.
    let brass = mono(
        "tempo 60\nfm s algorithm=1\nop 1 fixed=1000\nop 2 fixed=100,50 level=60 rates=20,99,99,99\nend\nchannel a=@s q8 c1",
    );
    let window = |start: f64| &brass[(start * RATE as f64) as usize..][..RATE as usize / 4];
    let sides: Vec<f64> = [0.0, 0.3, 0.6, 3.5]
        .map(|t| level(window(t), side))
        .to_vec();
    assert!(
        sides[..3].windows(2).all(|w| w[1] > w[0] * 1.2),
        "{sides:?}"
    );
    assert!(sides[3] >= sides[2], "{sides:?}");
}

#[test]
fn the_pitch_envelope_follows_its_levels_and_rates() {
    // Levels 50 -> 66 is +16/32 octave; rate 50 is 41/21.3 octaves per second.
    let samples = mono(
        "tempo 60\nfm s\npitch rates=50,99,99,50 levels=66,66,66,50\nop 1\nend\nchannel a=@s q8 o4 a1",
    );
    let hz = |seconds: f64| {
        // Zero crossings over 20 ms.
        let start = (seconds * RATE as f64) as usize;
        let slice = &samples[start..start + 960];
        let crossings = slice
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        crossings as f64 / 0.02
    };
    let rise = 0.5 / (41.0 / 21.3);
    assert!((hz(0.0) - 440.0).abs() < 60.0);
    let half = 440.0 * 2f64.powf(0.25);
    assert!(
        (hz(rise / 2.0 - 0.01) - half).abs() < 30.0,
        "{}",
        hz(rise / 2.0 - 0.01)
    );
    let top = 440.0 * 2f64.powf(0.5);
    assert!((hz(rise + 0.05) - top).abs() < 30.0);
    assert!((hz(3.5) - top).abs() < 30.0);
    // Fixed operators ignore it.
    let fixed = mono(
        "tempo 60\nfm s\npitch rates=50,99,99,50 levels=99,99,99,50\nop 1 fixed=1000\nend\nchannel a=@s q8 c1",
    );
    assert!((level(&fixed[RATE as usize..2 * RATE as usize], 1000.0) - 0.5).abs() < 1e-3);
}

const SONG: &str = "tempo 140
fm keys algorithm=5 feedback=5 voices=6 unison=2 unison-detune=9 spread=70 pan=-20
  pitch rates=80,80,80,80 levels=52,50,50,50
  op 1 ratio=1 rates=96,45,35,60 levels=99,80,0,0
  op 2 ratio=14 level=62 rates=95,60,50,70 levels=99,60,0,0
  op 3 ratio=1 detune=3 rates=95,40,30,55 levels=99,90,0,0
  op 4 ratio=1 level=80 rates=95,45,30,55 levels=99,85,0,0
  op 5 ratio=2 detune=-4 level=90 rates=90,40,30,55 levels=99,90,0,0
  op 6 ratio=3,50 level=74 rates=95,50,30,55 levels=99,85,0,0
end
fm bass algorithm=2 feedback=6 voices=1 glide=40 legato=on key-sync=off
  op 1 ratio=0.5 rates=99,50,40,70 levels=99,85,70,0
  op 2 ratio=1 level=78 rates=99,60,40,70 levels=99,70,50,0
end
instrument lead wave=saw unison=3 detune=12 voices=2 adsr=5,40,70,60
channel keys=@keys o4 l8 {c e g}4 {d f a}!80 r {e g b}2 L [{c e g} r]2
channel bass=@bass o2 l8 c c e g c d e g L [c d]2
channel lead=@lead o5 l4 r c e g";

#[test]
fn rendering_does_not_depend_on_chunks_and_replays_after_reset() {
    let score = Score::parse(SONG).unwrap();
    let whole = score.render_stereo_with(RATE, &options()).unwrap();
    let frames = whole.frames();
    let interleaved: Vec<f32> = (0..frames)
        .flat_map(|i| [0, 1].map(|c| whole.channel_data(c).unwrap()[i]))
        .collect();
    assert!(peak(&interleaved) > 0.2);
    assert!(interleaved.iter().all(|x| x.is_finite()));
    for chunk in [1usize, 7, 64, 511, 4096] {
        let mut synth = Synth::new_stereo(&score, RATE, &options()).unwrap();
        let mut out = vec![0.0f32; interleaved.len()];
        let mut written = 0;
        while written < frames {
            let end = (written + chunk).min(frames);
            let n = synth.fill(&mut out[written * 2..end * 2]);
            assert!(n > 0);
            written += n;
        }
        assert_eq!(synth.fill(&mut [0.0; 2]), 0);
        assert!(out == interleaved, "chunk {chunk}");
        synth.reset();
        let mut again = vec![0.0f32; interleaved.len()];
        assert_eq!(synth.fill(&mut again), frames);
        assert!(again == interleaved);
    }
    // Mono is the mean of the two sides.
    let mono = score.render_with(RATE, &options()).unwrap();
    let (l, r) = (
        whole.channel_data(0).unwrap(),
        whole.channel_data(1).unwrap(),
    );
    for (i, &m) in mono.channel_data(0).unwrap().iter().enumerate() {
        assert!((m - (l[i] + r[i]) * 0.5).abs() < 1e-6);
    }
    // Loops replay the carried chord and stay finite.
    let looped = score
        .render_stereo_with(
            RATE,
            &RenderOptions {
                loops: Some(2),
                ..options()
            },
        )
        .unwrap();
    assert!(looped.frames() > frames);
}

#[test]
fn voices_pan_unison_and_volume_behave_like_the_other_instruments() {
    let note = |header: &str| {
        stereo(&format!(
            "tempo 60\nfm s {header}\nop 1 fixed=1000\nend\nchannel a=@s c2"
        ))
    };
    let body = |audio: &Audio, c: usize| {
        audio.channel_data(c).unwrap()[RATE as usize..][..RATE as usize].to_vec()
    };
    let centre = note("");
    assert!((peak(&body(&centre, 0)) - 0.5).abs() < 1e-4);
    assert_eq!(body(&centre, 0), body(&centre, 1));
    let left = note("pan=-100");
    assert!((peak(&body(&left, 0)) - 0.5).abs() < 1e-4);
    assert_eq!(peak(&body(&left, 1)), 0.0);
    let half = note("pan=50");
    assert!((peak(&body(&half, 0)) - 0.25).abs() < 1e-4);
    assert!((peak(&body(&half, 1)) - 0.5).abs() < 1e-4);
    // Unison copies are normalized: without detune or spread they equal one copy.
    let four = note("unison=4");
    for (a, b) in body(&four, 0).iter().zip(&body(&centre, 0)) {
        assert!((a - b).abs() < 1e-6);
    }
    let spread = note("unison=2 spread=100");
    assert!((peak(&body(&spread, 0)) - 0.25).abs() < 1e-4);
    let quiet = note("volume=5");
    assert!((peak(&body(&quiet, 0)) - 0.5 / 3.0).abs() < 1e-4);
    // Velocity does not scale an FM voice; channel volume does.
    let line = |notes: &str| {
        mono(&format!(
            "tempo 60\nfm s\nop 1 fixed=1000\nend\nchannel a=@s {notes}"
        ))
    };
    assert_eq!(line("c2!20"), line("c2"));
    assert!((peak(&line("v5 c2")) - 0.5 / 3.0).abs() < 1e-4);

    // A chord sums its voices; more notes than voices steal the oldest.
    let chord = mono("tempo 60\nfm s voices=3\nop 1\nend\nchannel a=@s o4 {c e g}2");
    let body = &chord[RATE as usize..][..RATE as usize];
    for hz in [261.6256, 329.6276, 391.9954] {
        assert!((level(body, hz) - 0.5).abs() < 2e-3, "{hz}");
    }
    let stolen = mono("tempo 60\nfm s voices=1\nop 1\nend\nchannel a=@s q8 o4 l1 c e");
    let tail = &stolen[(RATE as f64 * 4.2) as usize..][..RATE as usize / 2];
    assert!(level(tail, 261.6256) < 1e-4 && (level(tail, 329.6276) - 0.5).abs() < 2e-3);
}

#[test]
fn glide_legato_and_key_sync_keep_their_state() {
    // Legato keeps the envelope: the second note does not attack again.
    let patch = "op 1 rates=60,55,99,60 levels=99,50,50,0\nend\nchannel a=@s q8 o4 l2 c g";
    let legato = mono(&format!(
        "tempo 60\nfm s voices=1 legato=on glide=100\n{patch}"
    ));
    let retrigger = mono(&format!("tempo 60\nfm s voices=1\n{patch}"));
    let low = FM_CARRIER_PEAK * 2f64.powf((64.0 * 39.0 + 4064.0 - 4256.0 - 3840.0) / 256.0);
    assert!((at(&legato, 2.4) - low).abs() < low * 0.05);
    assert!(at(&retrigger, 2.2) > low * 2.0);
    // The glide reaches the new pitch after its 100 ms.
    let body = &legato[(RATE as f64 * 2.5) as usize..][..RATE as usize];
    assert!(level(body, 391.9954) > low * 0.9);
    assert!(level(body, 261.6256) < low * 0.01);

    // With key sync off the phase carries over from the slot's previous note.
    let sync = |mode: &str| {
        mono(&format!(
            "tempo 61\nfm s voices=1 key-sync={mode}\nop 1 fixed=100,37\nend\nchannel a=@s q8 l4 c c"
        ))
    };
    let second = (RATE as f64 * 60.0 / 61.0) as usize + 200;
    let (on, off) = (sync("on"), sync("off"));
    assert_eq!(on[200], on[second]);
    assert_ne!(off[200], off[second]);
    assert_eq!(on[..second - 400], off[..second - 400]);
}

#[test]
fn tails_are_bounded_and_an_operator_above_the_limit_is_silent() {
    // A carrier whose last level is not silence is cut when it gets there.
    let ringing = mono(
        "tempo 60\nfm s\nop 1 fixed=1000 rates=99,99,99,60 levels=99,99,99,70\nend\nchannel a=@s c4",
    );
    assert!(ringing.len() < RATE as usize * 2);
    assert!(peak(&ringing[ringing.len() - 48..]) > 0.01);
    // The slowest release is cut after a minute.
    let slow = Score::parse("tempo 60\nfm s\nop 1 rates=99,99,99,0\nend\nchannel a=@s c4").unwrap();
    let synth = Synth::new(&slow, 8000, &options()).unwrap();
    assert_eq!(synth.max_frames(), Some(8000 + 60 * 8000 + 1));
    let audio = slow.render_with(8000, &options()).unwrap();
    assert!(audio.frames() <= 61 * 8000 + 1 && audio.frames() >= 60 * 8000);

    // 0.45 of 8 kHz is 3.6 kHz: the 5 kHz operator is silent, as carrier or modulator.
    let render = |text: &str| {
        let score = Score::parse(text).unwrap();
        score
            .render_with(8000, &options())
            .unwrap()
            .channel_data(0)
            .unwrap()
            .to_vec()
    };
    let high = render("tempo 60\nfm s algorithm=32\nop 1 fixed=1000,70\nend\nchannel a=@s c4");
    assert_eq!(peak(&high), 0.0);
    let modulated =
        render("tempo 60\nfm s\nop 1 fixed=100\nop 2 fixed=1000,70\nend\nchannel a=@s c4");
    let plain = render("tempo 60\nfm s\nop 1 fixed=100\nend\nchannel a=@s c4");
    assert_eq!(modulated, plain);
}

#[test]
fn score_text_accepts_fm_blocks_with_positions() {
    let score = Score::parse(SONG).unwrap();
    let (name, keys) = &score.instruments()[0];
    assert_eq!(name, "keys");
    let voice = keys.fm.unwrap();
    assert_eq!((voice.algorithm, voice.voices, voice.unison), (5, 6, 2));
    assert_eq!(score.instruments()[1].1.fm.unwrap().glide_ms, 40);
    assert!(score.instruments()[2].1.fm.is_none());
    let patch =
        FmPatch::parse(&SONG.lines().skip(1).take(9).collect::<Vec<_>>().join("\n")).unwrap();
    assert_eq!(patch.voice, voice);

    let error = |text: &str| Score::parse(text).unwrap_err();
    let e = error("tempo 120\nfm a\n  op 1 level=100\nend\nchannel c=@a c");
    assert_eq!(
        (e.line, e.column, e.code),
        (3, 14, ErrorCode::InvalidArgument)
    );
    let e = error("tempo 120\n\nfm a\n  op 1\nchannel c=@a c");
    assert_eq!((e.line, e.column, e.code), (5, 1, ErrorCode::Syntax));
    let e = error("fm a\n  op 1\n");
    assert_eq!((e.line, e.column, e.code), (1, 1, ErrorCode::Syntax));
    let e = error("instrument a wave=saw\nfm a\nop 1\nend");
    assert_eq!((e.line, e.code), (2, ErrorCode::InvalidArgument));
    let e = error("fm a\nop 2\nend");
    assert_eq!(
        (e.line, e.column, e.code),
        (1, 1, ErrorCode::InvalidArgument)
    );
    // A comment and a blank line inside the block.
    assert!(Score::parse("fm a ; one sine\n\n  op 1 ; carrier\nend\nchannel c=@a c").is_ok());
}

#[test]
fn features_of_later_issues_are_rejected_by_name() {
    for (line, feature) in [
        ("op 1 amp-sensitivity=1", "amplitude sensitivity"),
        ("lfo pitch=10\nop 1", "LFO"),
        ("lfo amp=10\nop 1", "LFO"),
    ] {
        let text = format!("fm a\n{line}\nend\nchannel c=@a c");
        let e = Score::parse(&text).unwrap_err();
        assert_eq!((e.line, e.code), (1, ErrorCode::InvalidArgument), "{line}");
        assert!(
            e.message.contains(feature) && e.message.contains("yet"),
            "{}",
            e.message
        );
        // The patch itself is valid; only rendering it is not available.
        assert!(FmPatch::parse(&format!("fm a\n{line}\nend")).is_ok());
    }
    // An LFO line that changes nothing audible is accepted.
    assert!(
        Score::parse(
            "fm a\nlfo wave=sine speed=50 delay=20\nop 1 scaling=50,0,0 curves=+lin,-exp\nend"
        )
        .is_ok()
    );

    let invalid = |change: fn(&mut Instrument)| {
        let mut instrument = Instrument::fm(Fm::default());
        change(&mut instrument);
        instrument.validate().unwrap_err().code
    };
    assert!(Instrument::fm(Fm::default()).validate().is_ok());
    assert_eq!(
        invalid(|i| i.synthwave = Some(Synthwave::default())),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        invalid(|i| i.waveform = Waveform::Sawtooth),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        invalid(|i| i.envelope.release_ms = 10),
        ErrorCode::InvalidArgument
    );
    assert_eq!(invalid(|i| i.slide = 100), ErrorCode::InvalidArgument);
    assert_eq!(
        invalid(|i| i.fm.as_mut().unwrap().algorithm = 40),
        ErrorCode::InvalidArgument
    );
}

#[test]
fn held_note_controls_reach_fm_voices() {
    let score =
        Score::parse("tempo 60\nfm s\nop 1\nop 2 fixed=1000 level=0\nend\nchannel a=@s o4 a1")
            .unwrap();
    let target = ControlTarget::Instrument {
        track: 0,
        instrument: 0,
    };
    let render = |events: &[ControlEvent]| {
        let mut synth = Synth::new_stereo(&score, RATE, &options()).unwrap();
        synth.prepare_controls(events, 8)?;
        let mut out = vec![0.0f32; 2 * 4 * RATE as usize];
        synth.fill(&mut out);
        let side = |c: usize| out.iter().skip(c).step_by(2).copied().collect::<Vec<f32>>();
        audiobox::Result::Ok((side(0), side(1)))
    };
    let second = |samples: &[f32], n: usize| samples[n * RATE as usize..][..RATE as usize].to_vec();
    let (l, r) = render(&[
        ControlEvent::new(RATE as u64, target, ControlParameter::Expression, 0.5),
        ControlEvent::new(2 * RATE as u64, target, ControlParameter::Pan, 1.0),
        ControlEvent::new(
            3 * RATE as u64,
            target,
            ControlParameter::PitchCents,
            1200.0,
        ),
    ])
    .unwrap();
    assert!((peak(&second(&l, 0)) - 0.5).abs() < 1e-4);
    assert!((peak(&second(&l, 1)) - 0.25).abs() < 1e-4);
    assert_eq!(peak(&second(&l, 2)), 0.0);
    assert!((peak(&second(&r, 2)) - 0.25).abs() < 1e-4);
    assert!((level(&second(&r, 3), 880.0) - 0.25).abs() < 1e-3);
    for parameter in [
        ControlParameter::CutoffHz,
        ControlParameter::ResonanceQ,
        ControlParameter::PitchLfoDepth,
        ControlParameter::AmplitudeLfoDepth,
    ] {
        let e = render(&[ControlEvent::new(0, target, parameter, 1.0)]).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidArgument);
    }
}

#[test]
fn fm_samples_have_a_cross_platform_pinned_hash() {
    let audio = Score::parse(SONG)
        .unwrap()
        .render_stereo_with(RATE, &options())
        .unwrap();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for channel in 0..2 {
        for sample in audio.channel_data(channel).unwrap() {
            for byte in sample.to_bits().to_le_bytes() {
                hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    assert_eq!(hash, 0x6ce585a9631ad164, "{hash:#018x}");
}
