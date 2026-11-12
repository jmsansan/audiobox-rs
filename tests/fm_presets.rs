#![cfg(feature = "synth")]
//! The built-in FM presets: they parse, override cleanly, stay bounded and are pinned.
use audiobox::ErrorCode;
use audiobox::synth::*;

const RATE: u32 = 32000;

fn render(text: &str) -> Vec<f32> {
    let options = RenderOptions {
        gain: 1.0,
        ..RenderOptions::default()
    };
    let score = Score::parse(text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let audio = score.render_with(RATE, &options).unwrap();
    audio.channel_data(0).unwrap().to_vec()
}
fn note(name: &str, notes: &str) -> Vec<f32> {
    render(&format!(
        "tempo 120\nfm p preset={name}\nend\nchannel a=@p q8 {notes}"
    ))
}
fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |m, &x| m.max(x.abs()))
}
/// Power-weighted mean frequency of 2048 Hann-windowed samples from `from` seconds.
fn centroid(samples: &[f32], from: f64) -> f64 {
    let start = (RATE as f64 * from) as usize;
    let body = &samples[start..start + 2048];
    let n = body.len();
    let (mut weighted, mut total) = (0.0, 0.0);
    for bin in 1..n / 2 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &x) in body.iter().enumerate() {
            let window = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            let angle = std::f64::consts::TAU * (bin * i % n) as f64 / n as f64;
            re += x as f64 * window * angle.cos();
            im += x as f64 * window * angle.sin();
        }
        weighted += (re * re + im * im) * bin as f64;
        total += re * re + im * im;
    }
    weighted / total.max(1e-30) * RATE as f64 / n as f64
}

#[test]
fn every_preset_is_a_valid_patch_under_its_own_name() {
    assert_eq!(FM_PRESETS.len(), 11);
    for name in FM_PRESETS {
        let patch = FmPatch::preset(name).unwrap();
        assert_eq!(patch.name, name);
        patch.validate().unwrap();
        assert_eq!(FmPatch::parse(&patch.to_string()).unwrap(), patch);
        // Every preset is documented.
        let doc = include_str!("../docs/fm-presets.md");
        assert!(
            doc.contains(&format!("### `{name}`")),
            "{name} is missing from the guide"
        );
    }
    assert!(FmPatch::preset("piano").is_none());
    assert!(FmPatch::preset("").is_none());
}

#[test]
fn a_preset_is_a_starting_point_that_lines_override() {
    let base = FmPatch::preset("epiano").unwrap();
    let same = FmPatch::parse("fm keys preset=epiano\nend").unwrap();
    assert_eq!(
        (same.voice, same.volume, same.name.as_str()),
        (base.voice, base.volume, "keys")
    );

    // Each override changes only what it names.
    let changed = FmPatch::parse(
        "fm keys feedback=6 preset=epiano volume=12\n  lfo amp=40\n  pitch levels=52,50,50,50\n  op 2 level=70\n  op 4 ratio=2 velocity=3\nend",
    )
    .unwrap();
    let mut expected = base.voice;
    expected.feedback = 6;
    expected.lfo.amplitude_depth = 40;
    expected.pitch_envelope.levels = [52, 50, 50, 50];
    expected.operators[1].as_mut().unwrap().level = 70;
    let op4 = expected.operators[3].as_mut().unwrap();
    op4.frequency = FmFrequency::Ratio { coarse: 2, fine: 0 };
    op4.velocity = 3;
    assert_eq!(changed.voice, expected);
    assert_eq!(changed.volume, 12);
    // The LFO keeps the preset's other settings.
    assert_eq!(changed.voice.lfo.wave, FmLfoWave::Sine);
    assert_eq!(changed.voice.lfo.speed, base.voice.lfo.speed);

    // An operator the preset leaves off starts from the defaults; one it has can be
    // silenced; the algorithm can change.
    let marimba =
        FmPatch::parse("fm m preset=marimba\n  op 5 ratio=2 level=80\n  op 4 level=0\nend")
            .unwrap();
    assert_eq!(
        marimba.voice.operators[4].unwrap().envelope,
        FmEnvelope::OPERATOR
    );
    assert_eq!(marimba.voice.operators[3].unwrap().level, 0);
    assert!(FmPatch::parse("fm b preset=bell algorithm=32\nend").is_ok());

    let at = |text: &str, line: usize, column: usize, code: ErrorCode| {
        let e = FmPatch::parse(text).unwrap_err();
        assert_eq!((e.line, e.column, e.code), (line, column, code), "{e}");
    };
    at("fm a preset=piano\nend", 1, 13, ErrorCode::InvalidArgument);
    at(
        "fm a preset=epiano preset=bell\nend",
        1,
        20,
        ErrorCode::Syntax,
    );
    at(
        "fm a preset=epiano\n  op 2 level=100\nend",
        2,
        14,
        ErrorCode::InvalidArgument,
    );
    at(
        "fm a preset=epiano\n  op 2 level=50\n  op 2 level=60\nend",
        3,
        6,
        ErrorCode::Syntax,
    );
    // Mono presets keep their rule when the voice count is raised.
    at(
        "fm a preset=bass-round voices=4\nend",
        1,
        1,
        ErrorCode::InvalidArgument,
    );
    let message = FmPatch::parse("fm a preset=x\nend").unwrap_err().message;
    assert!(
        message.contains("epiano") && message.contains("metal"),
        "{message}"
    );

    // In a score, with other instruments.
    let score = Score::parse(
        "tempo 120\nfm keys preset=epiano\n  op 2 level=50\nend\ninstrument sq wave=pulse\nchannel a=@keys {c e g}2\nchannel b=@sq c2",
    )
    .unwrap();
    assert_eq!(
        score.instruments()[0].1.fm.unwrap().operators[1]
            .unwrap()
            .level,
        50
    );
    assert_eq!(score.instruments()[0].1.volume, base.volume);
}

#[test]
fn presets_stay_bounded_end_in_silence_and_respond_to_velocity() {
    for name in FM_PRESETS {
        let register = if name.starts_with("bass") { "o2" } else { "o4" };
        let loud = note(name, &format!("{register} c1!127"));
        let soft = note(name, &format!("{register} c1!50"));
        assert!(loud.iter().all(|x| x.is_finite()), "{name}");
        let (high, low) = (peak(&loud), peak(&soft));
        assert!((0.25..=1.0).contains(&high), "{name}: peak {high}");
        assert!(
            peak(&loud[loud.len() - 16..]) < 0.01,
            "{name} ends in silence"
        );
        assert!(
            loud.len() < RATE as usize * 12,
            "{name}: {} frames",
            loud.len()
        );
        if name == "organ" {
            assert_eq!(loud, soft, "an organ has no touch response");
        } else {
            assert!(low < high * 0.9, "{name}: {low} against {high}");
        }
        // The top of the keyboard stays usable.
        let top = note(name, "o7 c2!127");
        assert!(
            top.iter().all(|x| x.is_finite()) && peak(&top) <= 1.2,
            "{name} at C7"
        );
    }
    // Where the guide says a harder note is brighter, the centroid shows it.
    for (name, from) in [
        ("epiano", 0.02),
        ("bell", 0.02),
        ("clav", 0.02),
        ("metal", 0.02),
        ("bass-hard", 0.02),
        ("brass", 0.6),
    ] {
        let register = if name.starts_with("bass") { "o2" } else { "o4" };
        let loud = centroid(&note(name, &format!("{register} c1!127")), from);
        let soft = centroid(&note(name, &format!("{register} c1!50")), from);
        assert!(loud > soft * 1.3, "{name}: {soft:.0} Hz then {loud:.0} Hz");
    }
}

#[test]
fn preset_samples_are_pinned() {
    let pinned: [u64; 11] = [
        0x126522103b4e163e,
        0xc23aaa0b71b363ef,
        0x5bd04b534bb6402c,
        0xa1affdfca416b995,
        0x44b7c1899293c469,
        0x77f27247b9821005,
        0xa38743c4e8257b62,
        0x03c53f4c19ac66e2,
        0xfdb0bd441125274d,
        0x20625eaf61d8c9f3,
        0x93b2626bbff59961,
    ];
    let mut got = Vec::new();
    for name in FM_PRESETS {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for notes in ["o2 c4!60 r8", "o4 c4!127 r8", "o6 {c e}4!90 r8"] {
            if notes.contains('{') && FmPatch::preset(name).unwrap().voice.voices < 2 {
                continue;
            }
            for sample in note(name, notes) {
                for byte in sample.to_bits().to_le_bytes() {
                    hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
                }
            }
        }
        got.push(hash);
    }
    let show = |hashes: &[u64]| {
        hashes
            .iter()
            .map(|h| format!("{h:#018x}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    assert_eq!(show(&got), show(&pinned));
}
