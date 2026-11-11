#![cfg(feature = "synth")]
//! The FM LFO: waveforms, delay, pitch and amplitude depth, key sync and tempo sync,
//! checked against the formulas in `docs/fm-voice.md`.
use audiobox::synth::*;

const RATE: u32 = 48000;

fn options() -> RenderOptions {
    RenderOptions {
        gain: 1.0,
        ..RenderOptions::default()
    }
}
fn mono(text: &str) -> Vec<f32> {
    let score = Score::parse(text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let audio = score.render_with(RATE, &options()).unwrap();
    audio.channel_data(0).unwrap().to_vec()
}
/// Peak of each millisecond: the envelope of a 1 kHz carrier.
fn envelope(samples: &[f32]) -> Vec<f64> {
    samples
        .chunks_exact(48)
        .map(|ms| ms.iter().fold(0.0f64, |m, &x| m.max(x.abs() as f64)))
        .collect()
}
fn hz(speed: u32) -> f64 {
    let sr = if speed == 0 { 1 } else { (165 * speed) >> 6 };
    let sr = if sr < 160 {
        sr * 11
    } else {
        sr * (11 + ((sr - 160) >> 4))
    };
    sr as f64 * 25_190_424.0 / 4_294_967_296.0
}
fn wave(name: &str, phase: f64) -> f64 {
    let p = phase - phase.floor();
    match name {
        "triangle" if p < 0.5 => 2.0 * p,
        "triangle" => 2.0 - 2.0 * p,
        "saw-down" => (1.5 - p) % 1.0,
        "saw-up" => (p + 0.5) % 1.0,
        "square" if p < 0.5 => 1.0,
        "square" => 0.0,
        "sine" => 0.5 + 0.5 * (std::f64::consts::TAU * p).sin(),
        _ => unreachable!(),
    }
}
/// A 1 kHz carrier whose level the LFO moves fully (`amp=99`, sensitivity 3).
fn tremolo(lfo: &str, notes: &str) -> Vec<f64> {
    envelope(&mono(&format!(
        "tempo 60\nfm s\nlfo {lfo} amp=99\nop 1 fixed=1000 amp-sensitivity=3\nend\nchannel a=@s q8 {notes}"
    )))
}

#[test]
fn each_waveform_follows_its_shape_from_half_a_cycle_in() {
    assert!((hz(0) - 0.0645).abs() < 1e-3 && (hz(35) - 5.806).abs() < 1e-2);
    assert!((hz(99) - 23.93).abs() < 1e-2);
    for name in ["triangle", "saw-down", "saw-up", "square", "sine"] {
        for speed in [10, 30] {
            let measured = tremolo(&format!("wave={name} speed={speed}"), "c1");
            let expected =
                |ms: f64| 0.5 * (1.0 - 0.99 * wave(name, 0.5 + (ms + 0.5) / 1000.0 * hz(speed)));
            let mut checked = 0;
            for (ms, &got) in measured.iter().enumerate().take(2000).skip(5) {
                let want = expected(ms as f64);
                let edge = (expected(ms as f64 - 3.0) - want).abs() > 0.05
                    || (expected(ms as f64 + 3.0) - want).abs() > 0.05;
                if !edge {
                    assert!(
                        (got - want).abs() < 0.012,
                        "{name} speed {speed} at {ms} ms: {got} vs {want}"
                    );
                    checked += 1;
                }
            }
            assert!(checked > 1700, "{name}: {checked}");
        }
    }
}

#[test]
fn sample_and_hold_steps_once_per_cycle_and_repeats_for_a_seed() {
    let run = |seed: u32| tremolo(&format!("wave=sample-hold speed=30 seed={seed}"), "c1");
    let (a, again, b) = (run(1), run(1), run(2));
    assert_eq!(a, again);
    assert_ne!(a, b);
    // The first step comes after half a cycle; each later one lasts a whole cycle.
    let period = 1000.0 / hz(30);
    let mut values = Vec::new();
    for cycle in 0..12 {
        let start = (period * (cycle as f64 + 0.5)) as usize + 3;
        let end = (period * (cycle as f64 + 1.5)) as usize - 3;
        let (low, high) = a[start..end]
            .iter()
            .fold((1.0f64, 0.0f64), |(l, h), &x| (l.min(x), h.max(x)));
        assert!(high - low < 1e-3, "cycle {cycle}: {low}..{high}");
        // Levels are k/256 of the full dip.
        let dip = (1.0 - low / 0.5) / 0.99 * 256.0;
        assert!((dip - dip.round()).abs() < 0.05 && (1.0..=256.0).contains(&dip.round()));
        values.push(dip.round() as i32);
    }
    values.sort();
    values.dedup();
    assert!(values.len() > 8, "{values:?}");
}

#[test]
fn delay_holds_then_ramps_the_depth() {
    // delay=60: a = 39 -> (16 + 7) << 3 = 184; hold 85.25/184 s, ramp 85.25/128 s.
    let (hold, ramp) = (85.25 / 184.0, 85.25 / 128.0);
    let measured = tremolo("wave=square speed=60 delay=60", "c1");
    let low = |from: f64, to: f64| {
        measured[(from * 1000.0) as usize..(to * 1000.0) as usize]
            .iter()
            .fold(1.0f64, |m, &x| m.min(x))
    };
    assert!(low(0.01, hold - 0.01) > 0.4999);
    let half = hold + ramp / 2.0;
    let dip = 1.0 - low(half - 0.03, half + 0.03) / 0.5;
    assert!((dip - 0.495).abs() < 0.06, "{dip}");
    assert!((low(hold + ramp + 0.05, hold + ramp + 0.3) - 0.005).abs() < 1e-3);
    // No delay: full depth from the first cycle.
    let plain = tremolo("wave=square speed=60", "c1");
    assert!(plain[..200].iter().fold(1.0f64, |m, &x| m.min(x)) < 0.006);
    // Depth scales with the amount and with each operator's sensitivity.
    for (amp, sensitivity) in [(99, 1), (50, 3), (30, 2), (99, 0)] {
        let samples = envelope(&mono(&format!(
            "tempo 60\nfm s\nlfo wave=square speed=60 amp={amp}\nop 1 fixed=1000 amp-sensitivity={sensitivity}\nend\nchannel a=@s q8 c1"
        )));
        let low = samples[10..1000].iter().fold(1.0f64, |m, &x| m.min(x));
        let expected = 0.5 * (1.0 - amp as f64 * 0.01 * sensitivity as f64 / 3.0);
        assert!(
            (low - expected).abs() < 1e-3,
            "amp {amp} sensitivity {sensitivity}"
        );
    }
}

fn crossings(samples: &[f32], from: f64, seconds: f64) -> f64 {
    let start = (from * RATE as f64) as usize;
    let slice = &samples[start..start + (seconds * RATE as f64) as usize];
    slice
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count() as f64
        / seconds
}

#[test]
fn pitch_depth_and_sensitivity_set_the_deviation() {
    let table = [0.0, 10.0, 20.0, 33.0, 55.0, 92.0, 153.0, 255.0];
    let half = 0.5 / hz(10);
    for (depth, sensitivity) in [(50, 7), (99, 3), (20, 5), (99, 0)] {
        let samples = mono(&format!(
            "tempo 60\nfm s\nlfo wave=square speed=10 pitch={depth} pitch-sensitivity={sensitivity}\nop 1\nend\nchannel a=@s q8 o4 a1"
        ));
        let octaves = ((depth * 165) >> 6) as f64 * table[sensitivity] / 65536.0;
        // Synced, the square starts in its low half.
        let low = crossings(&samples, 0.02, half - 0.04);
        let high = crossings(&samples, half + 0.02, half - 0.04);
        assert!(
            (low - 440.0 * 2f64.powf(-octaves)).abs() < 5.0,
            "{depth}/{sensitivity}: {low}"
        );
        assert!(
            (high - 440.0 * 2f64.powf(octaves)).abs() < 5.0,
            "{depth}/{sensitivity}: {high}"
        );
    }
    // A fixed-frequency operator ignores it.
    let fixed = mono(
        "tempo 60\nfm s\nlfo wave=square speed=10 pitch=99 pitch-sensitivity=7\nop 1 fixed=1000\nend\nchannel a=@s q8 c1",
    );
    assert!((crossings(&fixed, 0.02, 0.2) - 1000.0).abs() < 6.0);
    assert!((crossings(&fixed, half + 0.02, 0.2) - 1000.0).abs() < 6.0);
}

#[test]
fn amplitude_modulation_on_a_modulator_moves_the_tone_not_the_level() {
    let samples = mono(
        "tempo 60\nfm s algorithm=1\nlfo wave=square speed=10 amp=99\nop 1 fixed=1000\nop 2 fixed=100 level=75 amp-sensitivity=3\nend\nchannel a=@s q8 c1",
    );
    let half = 0.5 / hz(10);
    let side = |from: f64| {
        let start = (from * RATE as f64) as usize;
        let body = &samples[start..start + RATE as usize / 5];
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &x) in body.iter().enumerate() {
            let angle = std::f64::consts::TAU * 1100.0 * i as f64 / RATE as f64;
            re += x as f64 * angle.cos();
            im += x as f64 * angle.sin();
        }
        2.0 * (re * re + im * im).sqrt() / body.len() as f64
    };
    let (open, closed) = (side(0.03), side(half + 0.03));
    assert!(open > 0.1 && closed < open * 0.02, "{open} then {closed}");
    let levels = envelope(&samples);
    assert!(levels[20..2000].iter().all(|l| (l - 0.5).abs() < 0.03));
}

#[test]
fn key_sync_restarts_the_lfo_and_free_running_follows_the_clock() {
    // Synced: both notes start the LFO, the delay and the carrier alike.
    let synced = mono(
        "tempo 60\nfm s\nlfo wave=triangle speed=23 delay=30 amp=99\nop 1 fixed=1000 amp-sensitivity=3\nend\nchannel a=@s q8 l4 c c",
    );
    let worst = (400..40000)
        .map(|i| (synced[i] - synced[i + 48000]).abs())
        .fold(0.0f32, f32::max);
    assert!(worst == 0.0, "{worst}");
    // Free: the level at a given time does not depend on when the note started.
    let free = |notes: &str| tremolo("wave=triangle speed=23 sync=off", notes);
    let (early, late) = (free("l4 c2"), free("l4 r8 c4."));
    for ms in (520..1900).step_by(7) {
        assert!((early[ms] - late[ms]).abs() < 0.01, "{ms} ms");
    }
    let restarted = tremolo("wave=triangle speed=23", "l4 r8 c4.");
    assert!((520..1900).any(|ms| (early[ms] - restarted[ms]).abs() > 0.1));
    // Free-running starts the cycle at frame zero, not half a cycle in.
    let expected = 0.5 * (1.0 - 0.99 * wave("triangle", 0.3005 * hz(23)));
    assert!((early[300] - expected).abs() < 0.012);
}

#[test]
fn a_beat_synced_lfo_follows_the_tempo() {
    // One cycle per quarter note at 120 BPM is 2 Hz, whatever the speed says.
    let measured = envelope(&mono(
        "tempo 120\nfm s\nlfo wave=square beats=1,1 speed=99 amp=99\nop 1 fixed=1000 amp-sensitivity=3\nend\nchannel a=@s q8 c1",
    ));
    for (ms, &got) in measured.iter().enumerate().take(1900).skip(5) {
        let low_half = (ms / 250) % 2 == 0;
        if ms % 250 > 3 && ms % 250 < 247 {
            let want = if low_half { 0.5 } else { 0.005 };
            assert!((got - want).abs() < 1e-3, "{ms} ms: {got}");
        }
    }
    // The prepared path's musical clock gives the same cycle at a constant tempo.
    let patch = FmPatch::parse(
        "fm s\nlfo wave=triangle beats=3,2 amp=99 pitch=20\nop 1 fixed=1000 amp-sensitivity=3\nend",
    )
    .unwrap();
    let map = TempoMap::new(
        RATE,
        48,
        &[TempoChange {
            tick: 0,
            bpm: 120.0,
        }],
    )
    .unwrap();
    let score = FrameScore {
        instruments: vec![Instrument::fm(patch.voice)],
        tracks: vec![vec![FrameNote::new(
            12000,
            96000,
            Pitch::new(60).unwrap(),
            0,
        )]],
        end_frame: 96000,
    };
    let mut prepared = Synth::from_frames(&score, &map, &options()).unwrap();
    let mut out = vec![0.0f32; 96000];
    assert_eq!(prepared.fill(&mut out), 96000);
    let text = mono(
        "tempo 120\nfm s\nlfo wave=triangle beats=3,2 amp=99 pitch=20\nop 1 fixed=1000 amp-sensitivity=3\nend\nchannel a=@s q8 r8 c1^8",
    );
    let (a, b) = (envelope(&out), envelope(&text));
    for ms in 255..1990 {
        assert!(
            (a[ms] - b[ms]).abs() < 2e-3,
            "{ms} ms: {} vs {}",
            a[ms],
            b[ms]
        );
    }
    assert!(a[255..1990].iter().fold(1.0f64, |m, &x| m.min(x)) < 0.02);
}

#[test]
fn lfo_rendering_does_not_depend_on_chunks() {
    let score = Score::parse(
        "tempo 132
fm keys algorithm=5 feedback=4 voices=4
  lfo wave=sine speed=38 delay=40 pitch=12 amp=45 pitch-sensitivity=4
  op 1 rates=96,40,30,60 levels=99,80,0,0 velocity=3 amp-sensitivity=1
  op 2 ratio=7 level=70 velocity=6 amp-sensitivity=3
  op 3 detune=4
  op 4 level=78 amp-sensitivity=2
end
fm noise algorithm=1 voices=2
  lfo wave=sample-hold speed=70 sync=off pitch=60 pitch-sensitivity=6 seed=9
  op 1
  op 2 ratio=2 level=70
end
fm sweep algorithm=2 feedback=5 voices=1 glide=50 legato=on
  lfo wave=saw-up beats=1,2 amp=80 sync=off
  op 1 ratio=0.5
  op 2 level=80 amp-sensitivity=3
end
channel k=@keys o4 l4 {c e g}!100 {d f a}!60 L [{e g b} {c e g}]2
channel n=@noise o5 l8 c e g e L [c g]4
channel s=@sweep o2 l8 c d e f L [g e]4",
    )
    .unwrap();
    let whole = score.render_stereo_with(RATE, &options()).unwrap();
    let frames = whole.frames();
    let interleaved: Vec<f32> = (0..frames)
        .flat_map(|i| [0, 1].map(|c| whole.channel_data(c).unwrap()[i]))
        .collect();
    assert!(interleaved.iter().all(|x| x.is_finite()));
    for chunk in [1usize, 97, 1024] {
        let mut synth = Synth::new_stereo(&score, RATE, &options()).unwrap();
        let mut out = vec![0.0f32; interleaved.len()];
        let mut written = 0;
        while written < frames {
            let end = (written + chunk).min(frames);
            written += synth.fill(&mut out[written * 2..end * 2]);
        }
        assert!(out == interleaved, "chunk {chunk}");
        synth.reset();
        let mut again = vec![0.0f32; interleaved.len()];
        synth.fill(&mut again);
        assert!(again == interleaved);
    }
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for sample in &interleaved {
        for byte in sample.to_bits().to_le_bytes() {
            hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    assert_eq!(hash, 0x01891408c34b4ccd, "{hash:#018x}");
}
