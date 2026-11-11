#![cfg(feature = "sfx")]
//! FM sound effects: one-shot rendering, the text form and seeded presets.
use audiobox::ErrorCode;
use audiobox::sfx::{FmEffect, FmPreset, MAX_DURATION};
use audiobox::synth::FmPatch;

const RATE: u32 = 44_100;

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |m, &x| m.max(x.abs()))
}
fn hash(audio: &audiobox::Audio) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for channel in 0..audio.channels() {
        for sample in audio.channel_data(channel).unwrap() {
            for byte in sample.to_bits().to_le_bytes() {
                hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    hash
}
fn tone() -> FmEffect {
    FmEffect::new(FmPatch::parse("fm tone\nop 1 fixed=1000 rates=99,99,99,60\nend").unwrap())
}

#[test]
fn a_patch_renders_as_one_note_with_its_release() {
    let effect = tone();
    assert_eq!(
        (effect.note, effect.velocity, effect.gate, effect.volume),
        (60, 127, 0.2, 0.5)
    );
    let audio = effect.render(RATE).unwrap();
    assert_eq!((audio.channels(), audio.sample_rate()), (1, RATE));
    let samples = audio.channel_data(0).unwrap();
    // 0.2 s of gate, then a release of 90 dB at rate 60.
    let gate = (0.2 * RATE as f64) as usize;
    assert!(samples.len() > gate + RATE as usize / 10 && samples.len() < gate + RATE as usize);
    // Half scale times the volume of 0.5.
    assert!((peak(&samples[1000..gate]) - 0.25).abs() < 1e-3);
    assert!(peak(&samples[samples.len() - 20..]) < 1e-3);
    assert_eq!(hash(&effect.render(RATE).unwrap()), hash(&audio));

    let loud = FmEffect {
        volume: 1.0,
        ..tone()
    };
    assert!((peak(loud.render(RATE).unwrap().channel_data(0).unwrap()) - 0.5).abs() < 1e-3);
    let long = FmEffect {
        gate: 1.0,
        ..tone()
    };
    assert!(long.render(RATE).unwrap().frames() > RATE as usize);

    // Stereo keeps the patch's pan.
    let panned = FmEffect::new(FmPatch::parse("fm tone pan=-100\nop 1 fixed=1000\nend").unwrap());
    let stereo = panned.render_stereo(RATE).unwrap();
    assert_eq!(stereo.channels(), 2);
    assert!(peak(stereo.channel_data(0).unwrap()) > 0.2);
    assert_eq!(peak(stereo.channel_data(1).unwrap()), 0.0);

    // A release slower than the limit is cut at ten seconds with a fade.
    let endless = FmEffect::new(FmPatch::parse("fm tone\nop 1 rates=99,99,99,5\nend").unwrap());
    let audio = endless.render(8000).unwrap();
    assert_eq!(audio.frames(), (MAX_DURATION * 8000.0) as usize);
    let samples = audio.channel_data(0).unwrap();
    assert!(samples[samples.len() - 1].abs() < 1e-3);
    assert!(peak(&samples[samples.len() - 400..samples.len() - 200]) > 0.01);
}

#[test]
fn note_velocity_and_slide_shape_the_sound() {
    let crossings = |samples: &[f32], from: usize, frames: usize| {
        let slice = &samples[from..from + frames];
        slice
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count() as f64
            * RATE as f64
            / frames as f64
    };
    let patch = FmPatch::parse("fm tone\nop 1 velocity=7\nend").unwrap();
    let render = |effect: &FmEffect| {
        effect
            .render(RATE)
            .unwrap()
            .channel_data(0)
            .unwrap()
            .to_vec()
    };
    let a4 = FmEffect {
        note: 69,
        gate: 0.5,
        ..FmEffect::new(patch.clone())
    };
    let samples = render(&a4);
    assert!((crossings(&samples, 2000, 8820) - 440.0).abs() < 6.0);
    let soft = render(&FmEffect {
        velocity: 40,
        ..a4.clone()
    });
    assert!(peak(&soft) < peak(&samples) * 0.5);

    // An octave down over the gate, held for the release.
    let falling = FmEffect {
        slide: -24.0,
        ..a4.clone()
    };
    let samples = render(&falling);
    assert!((crossings(&samples, 441, 2205) - 440.0 * 2f64.powf(-0.035 * 2.0)).abs() < 25.0);
    assert!((crossings(&samples, 19000, 2900) - 232.0).abs() < 15.0);
    let rising = render(&FmEffect {
        slide: 24.0,
        ..a4.clone()
    });
    assert!((crossings(&rising, 19000, 2900) - 832.0).abs() < 40.0);

    let invalid = |effect: FmEffect| effect.render(RATE).unwrap_err().code;
    assert_eq!(
        invalid(FmEffect {
            slide: 49.0,
            ..a4.clone()
        }),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        invalid(FmEffect {
            gate: 0.0,
            ..a4.clone()
        }),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        invalid(FmEffect {
            gate: 10.5,
            ..a4.clone()
        }),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        invalid(FmEffect {
            volume: 1.5,
            ..a4.clone()
        }),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        invalid(FmEffect {
            note: 128,
            ..a4.clone()
        }),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        invalid(FmEffect {
            velocity: 200,
            ..a4
        }),
        ErrorCode::InvalidArgument
    );
}

#[test]
fn text_round_trips_and_errors_carry_positions() {
    let text = "; a short bell\nnote=84 velocity=100 gate=0.15 slide=-8 volume=0.6\nfm bell algorithm=1\n  op 1 ratio=1 rates=99,40,40,45 levels=99,0,0,0\n  op 2 ratio=3,50 level=75\nend\n";
    let effect: FmEffect = text.parse().unwrap();
    assert_eq!(
        (
            effect.note,
            effect.velocity,
            effect.gate,
            effect.slide,
            effect.volume
        ),
        (84, 100, 0.15, -8.0, 0.6)
    );
    assert_eq!(effect.patch.name, "bell");
    let written = effect.to_string();
    assert_eq!(written.parse::<FmEffect>().unwrap(), effect);
    assert_eq!(written.parse::<FmEffect>().unwrap().to_string(), written);
    // The header is optional, and may span lines.
    let bare: FmEffect = "fm a\nop 1\nend".parse().unwrap();
    assert_eq!(
        bare,
        FmEffect::new(FmPatch::parse("fm a\nop 1\nend").unwrap())
    );
    assert!(
        "note=70\ngate=0.3\n\nfm a\nop 1\nend"
            .parse::<FmEffect>()
            .is_ok()
    );

    let at = |text: &str, line: usize, column: usize, code: ErrorCode| {
        let e = FmEffect::parse(text).unwrap_err();
        assert_eq!((e.line, e.column, e.code), (line, column, code), "{e}");
    };
    at(
        "note=200\nfm a\nop 1\nend",
        1,
        6,
        ErrorCode::InvalidArgument,
    );
    at("note=60 gate=x\nfm a\nop 1\nend", 1, 14, ErrorCode::Syntax);
    at("note=60.5\nfm a\nop 1\nend", 1, 6, ErrorCode::Syntax);
    at("note=60 note=61\nfm a\nop 1\nend", 1, 9, ErrorCode::Syntax);
    at("pitch=60\nfm a\nop 1\nend", 1, 1, ErrorCode::Syntax);
    at("loud\nfm a\nop 1\nend", 1, 1, ErrorCode::Syntax);
    // Faults inside the block keep the line numbers of the whole text.
    at(
        "note=60\n\nfm a\n  op 1 level=100\nend",
        4,
        14,
        ErrorCode::InvalidArgument,
    );
    at("note=60\nfm a\n  op 1", 2, 1, ErrorCode::Syntax);
    at("note=60", 1, 1, ErrorCode::Syntax);
    at(
        "gate=1 slide=30\nfm a\nop 1\nend",
        1,
        1,
        ErrorCode::InvalidArgument,
    );
}

#[test]
fn presets_are_seeded_bounded_and_pinned() {
    let pinned = [
        (FmPreset::Laser, 0x9d5f0ee3cf4b84c6u64),
        (FmPreset::MetalHit, 0xbbf229da4494ee74),
        (FmPreset::Chime, 0x266fd90ca09e0eec),
        (FmPreset::Alarm, 0x5455b8ed1c738557),
    ];
    let mut hashes = Vec::new();
    for kind in FmPreset::ALL {
        let mut seen = Vec::new();
        for seed in 0..24 {
            let effect = FmEffect::preset(kind, seed);
            effect
                .validate()
                .unwrap_or_else(|e| panic!("{kind:?} {seed}: {e}"));
            assert_eq!(FmEffect::preset(kind, seed), effect);
            assert_eq!(effect.to_string().parse::<FmEffect>().unwrap(), effect);
            let audio = effect.render(RATE).unwrap();
            let samples = audio.channel_data(0).unwrap();
            assert!(samples.iter().all(|x| x.is_finite()));
            assert!(audio.frames() as f64 <= MAX_DURATION * RATE as f64);
            assert!(audio.frames() > RATE as usize / 50, "{kind:?} {seed}");
            let level = peak(samples);
            assert!(level > 0.05 && level <= 1.0, "{kind:?} {seed}: {level}");
            assert!(
                peak(&samples[samples.len() - 10..]) < 0.02,
                "{kind:?} {seed}"
            );
            seen.push(effect);
        }
        seen.dedup();
        assert!(seen.len() > 20, "{kind:?} varies with the seed");
        hashes.push((kind, hash(&FmEffect::preset(kind, 1).render(RATE).unwrap())));
    }
    let got: Vec<String> = hashes.iter().map(|(_, h)| format!("{h:#018x}")).collect();
    let want: Vec<String> = pinned.iter().map(|(_, h)| format!("{h:#018x}")).collect();
    assert_eq!(got, want);
}
