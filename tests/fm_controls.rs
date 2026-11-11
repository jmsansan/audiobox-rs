#![cfg(feature = "synth")]
//! Held-note controls on FM instruments: operator level, feedback and LFO depths.
use audiobox::ErrorCode;
use audiobox::synth::*;

const RATE: u32 = 48000;
const SECOND: u64 = RATE as u64;

fn options() -> RenderOptions {
    RenderOptions {
        gain: 1.0,
        ..RenderOptions::default()
    }
}
fn target() -> ControlTarget {
    ControlTarget::Instrument {
        track: 0,
        instrument: 0,
    }
}
fn render(text: &str, events: &[ControlEvent]) -> audiobox::Result<Vec<f32>> {
    let score = Score::parse(text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let mut synth = Synth::new(&score, RATE, &options())?;
    synth.prepare_controls(events, 16)?;
    let mut out = vec![0.0f32; synth.max_frames().unwrap() as usize];
    let frames = synth.fill(&mut out);
    out.truncate(frames);
    Ok(out)
}
fn step(frame: u64, parameter: ControlParameter, value: f64) -> ControlEvent {
    ControlEvent::new(frame, target(), parameter, value)
}
fn ramp(frame: u64, parameter: ControlParameter, value: f64, frames: u64) -> ControlEvent {
    ControlEvent {
        ramp_frames: frames,
        ..step(frame, parameter, value)
    }
}
/// Peak over 10 ms starting at `seconds`.
fn at(samples: &[f32], seconds: f64) -> f64 {
    let start = (seconds * RATE as f64) as usize;
    samples[start..start + 480]
        .iter()
        .fold(0.0f64, |m, &x| m.max(x.abs() as f64))
}
fn component(samples: &[f32], from: f64, hz: f64) -> f64 {
    let start = (from * RATE as f64) as usize;
    let body = &samples[start..start + RATE as usize / 4];
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &x) in body.iter().enumerate() {
        let angle = std::f64::consts::TAU * hz * i as f64 / RATE as f64;
        re += x as f64 * angle.cos();
        im += x as f64 * angle.sin();
    }
    2.0 * (re * re + im * im).sqrt() / body.len() as f64
}
/// Carrier peak for an output level, as the contract's level formula gives it.
fn carrier(level: f64) -> f64 {
    let scale = |l: f64| if l >= 20.0 { 28.0 + l } else { unreachable!() };
    0.5 * 2f64.powf(32.0 * (scale(level) - 127.0) / 256.0)
}

const TONE: &str = "tempo 60\nfm s\nop 1 fixed=1000\nend\nchannel a=@s q8 c1";

#[test]
fn an_operator_level_steps_and_ramps_in_the_patch_units() {
    use ControlParameter::FmOperator1Level as Level;
    let samples = render(
        TONE,
        &[
            step(SECOND, Level, 80.0),
            ramp(2 * SECOND, Level, 60.0, SECOND),
        ],
    )
    .unwrap();
    assert!((at(&samples, 0.5) - 0.5).abs() < 1e-4);
    assert!((at(&samples, 1.5) - carrier(80.0)).abs() < 1e-4);
    // Half way through a linear ramp the level is 70; fractions interpolate.
    assert!((at(&samples, 2.495) - carrier(70.0)).abs() < carrier(70.0) * 0.01);
    assert!((at(&samples, 2.745) - carrier(65.0)).abs() < carrier(65.0) * 0.01);
    assert!((at(&samples, 3.5) - carrier(60.0)).abs() < 1e-4);
    // A ramp has no step at either end: the envelope moves by less than 0.1 % per ms.
    let envelope: Vec<f64> = (0..1200)
        .map(|i| at(&samples, 1.9 + i as f64 * 0.001))
        .collect();
    assert!(
        envelope
            .windows(2)
            .all(|w| (w[1] / w[0] - 1.0).abs() < 2e-3)
    );

    // The patch's own value changes nothing.
    assert_eq!(
        render(TONE, &[step(0, Level, 99.0)]).unwrap(),
        render(TONE, &[]).unwrap()
    );
    // Velocity and keyboard scaling still apply underneath the control.
    let shaped = "tempo 60\nfm s\nop 1 fixed=1000 level=90 velocity=7 scaling=39,0,40\nend\nchannel a=@s q8 o6 c1!60";
    let (plain, lowered) = (
        render(shaped, &[]).unwrap(),
        render(shaped, &[step(0, Level, 70.0)]).unwrap(),
    );
    let expected = 2f64.powf(32.0 * (98.0 - 118.0) / 256.0);
    assert!((at(&lowered, 1.0) / at(&plain, 1.0) - expected).abs() < 1e-3);
}

#[test]
fn a_modulator_level_moves_the_tone() {
    let patch = "tempo 60\nfm s algorithm=1\nop 1 fixed=1000\nop 2 fixed=100 level=40\nend\nchannel a=@s q8 c1";
    let samples = render(
        patch,
        &[ramp(
            SECOND,
            ControlParameter::FmOperator2Level,
            72.0,
            2 * SECOND,
        )],
    )
    .unwrap();
    let sides: Vec<f64> = [0.5, 1.5, 2.2, 3.2]
        .map(|t| component(&samples, t, 1100.0))
        .to_vec();
    assert!(sides.windows(2).all(|w| w[1] > w[0] * 1.5), "{sides:?}");
    assert!((0..3900).all(|ms| at(&samples, ms as f64 / 1000.0 + 0.01) < 0.5001));
    // Per-note targets override the instrument lane for that note only.
    let two = "tempo 60\nfm s algorithm=1\nop 1 fixed=1000\nop 2 fixed=100 level=40\nend\nchannel a=@s q8 l2 c c";
    let note = ControlEvent::new(
        SECOND * 2 + 10,
        ControlTarget::Note { track: 0, note: 1 },
        ControlParameter::FmOperator2Level,
        72.0,
    );
    let samples = render(two, &[note]).unwrap();
    assert!(component(&samples, 2.5, 1100.0) > component(&samples, 0.5, 1100.0) * 5.0);
}

#[test]
fn feedback_and_lfo_depths_follow_their_controls() {
    use ControlParameter::{FmFeedback, FmLfoAmplitudeDepth, FmLfoPitchDepth};
    let saw = "tempo 60\nfm s algorithm=32 feedback=3\nop 6 fixed=100\nend\nchannel a=@s q8 c1";
    assert_eq!(
        render(saw, &[step(0, FmFeedback, 3.0)]).unwrap(),
        render(saw, &[]).unwrap()
    );
    let swept = render(
        saw,
        &[
            step(0, FmFeedback, 0.0),
            ramp(SECOND, FmFeedback, 5.0, 2 * SECOND),
        ],
    )
    .unwrap();
    let second: Vec<f64> = [0.5, 1.7, 2.4, 3.3]
        .map(|t| component(&swept, t, 200.0))
        .to_vec();
    assert!(
        second[0] < 1e-6 && second.windows(2).all(|w| w[1] > w[0]),
        "{second:?}"
    );
    let wild = render(saw, &[ramp(0, FmFeedback, 7.0, 3 * SECOND)]).unwrap();
    assert!(wild.iter().all(|x| x.is_finite() && x.abs() <= 0.5001));

    // Tremolo depth: a square LFO dips the carrier by amount/100 at sensitivity 3.
    let tremolo = "tempo 60\nfm s\nlfo wave=square speed=60 amp=20\nop 1 fixed=1000 amp-sensitivity=3\nend\nchannel a=@s q8 c1";
    let samples = render(
        tremolo,
        &[
            step(SECOND, FmLfoAmplitudeDepth, 70.0),
            step(2 * SECOND, FmLfoAmplitudeDepth, 0.0),
        ],
    )
    .unwrap();
    let low = |from: f64| {
        (0..200)
            .map(|i| at(&samples, from + i as f64 * 0.002))
            .fold(1.0f64, f64::min)
    };
    assert!((low(0.2) - 0.5 * 0.8).abs() < 2e-3);
    assert!((low(1.2) - 0.5 * 0.3).abs() < 2e-3);
    assert!((low(2.2) - 0.5).abs() < 2e-3);

    // Vibrato depth: the deviation is depth * 165/64 * S / 65536 octaves.
    let vibrato = "tempo 60\nfm s\nlfo wave=square speed=10 pitch=10 pitch-sensitivity=7\nop 1\nend\nchannel a=@s q8 o4 a1";
    let samples = render(vibrato, &[step(0, FmLfoPitchDepth, 40.0)]).unwrap();
    let crossings = samples[960..12960]
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count() as f64
        / 0.25;
    let octaves = 40.0 * 165.0 / 64.0 * 255.0 / 65536.0;
    assert!(
        (crossings - 440.0 * 2f64.powf(-octaves)).abs() < 5.0,
        "{crossings}"
    );
}

#[test]
fn unsupported_targets_fail_at_preparation() {
    use ControlParameter::*;
    let reject = |text: &str, parameter: ControlParameter, value: f64| {
        let e = render(text, &[step(0, parameter, value)]).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidArgument, "{parameter:?}");
    };
    // An operator the patch has switched off, and values out of range.
    reject(TONE, FmOperator2Level, 50.0);
    reject(TONE, FmOperator1Level, 99.5);
    reject(TONE, FmOperator1Level, -1.0);
    reject(TONE, FmFeedback, 7.5);
    // LFO depths the patch leaves at zero, or tremolo that reaches no operator.
    reject(TONE, FmLfoPitchDepth, 10.0);
    reject(TONE, FmLfoAmplitudeDepth, 10.0);
    reject(
        "tempo 60\nfm s\nlfo amp=30\nop 1\nend\nchannel a=@s c1",
        FmLfoAmplitudeDepth,
        10.0,
    );
    // FM controls on other instruments, and filter controls on FM.
    let saw = "tempo 60\ninstrument s wave=saw synthwave=on filter=lowpass lfo-pitch=5000,10\nchannel a=@s c1";
    for parameter in [
        FmOperator1Level,
        FmFeedback,
        FmLfoPitchDepth,
        FmLfoAmplitudeDepth,
    ] {
        reject(saw, parameter, 1.0);
        reject(
            "tempo 60\ninstrument s wave=pulse\nchannel a=@s c1",
            parameter,
            1.0,
        );
    }
    for parameter in [
        CutoffHz,
        ResonanceQ,
        PitchLfoDepth,
        CutoffLfoDepth,
        AmplitudeLfoDepth,
        PanLfoDepth,
    ] {
        reject(TONE, parameter, 1.0);
    }
    assert!(render(saw, &[step(0, PitchLfoDepth, 20.0)]).is_ok());

    for (name, parameter) in [
        ("op1-level", FmOperator1Level),
        ("op6-level", FmOperator6Level),
        ("feedback", FmFeedback),
        ("lfo-pitch", FmLfoPitchDepth),
        ("lfo-amp", FmLfoAmplitudeDepth),
    ] {
        assert_eq!(name.parse::<ControlParameter>().unwrap(), parameter);
    }
    assert!("op7-level".parse::<ControlParameter>().is_err());
    assert!("algorithm".parse::<ControlParameter>().is_err());
    assert_eq!(
        ControlParameter::fm_operator_level(3),
        Some(FmOperator3Level)
    );
    assert_eq!(ControlParameter::fm_operator_level(0), None);
    assert_eq!(ControlParameter::fm_operator_level(7), None);
}

#[test]
fn controlled_rendering_replays_and_ignores_chunks() {
    use ControlParameter::*;
    let score = Score::parse(
        "tempo 120\nfm keys algorithm=5 feedback=2 voices=4\nlfo wave=sine speed=40 pitch=8 amp=20\nop 1 velocity=3\nop 2 ratio=3 level=60 amp-sensitivity=2\nop 3 detune=3\nop 4 level=70\nend\nchannel k=@keys o4 l4 {c e g}!90 {d f a}!70 {e g b} {c e g}",
    )
    .unwrap();
    let events = [
        ramp(0, FmOperator2Level, 90.0, SECOND),
        ramp(1000, FmFeedback, 6.0, SECOND),
        ramp(SECOND / 2, FmLfoPitchDepth, 40.0, SECOND / 2),
        step(SECOND, FmLfoAmplitudeDepth, 80.0),
        ControlEvent {
            curve: ControlCurve::Exponential,
            ..ramp(SECOND, FmOperator4Level, 20.0, SECOND / 2)
        },
    ];
    let mut synth = Synth::new_stereo(&score, RATE, &options()).unwrap();
    synth.prepare_controls(&events, 8).unwrap();
    let frames = synth.max_frames().unwrap() as usize;
    let mut whole = vec![0.0f32; frames * 2];
    let written = synth.fill(&mut whole);
    whole.truncate(written * 2);
    assert!(whole.iter().all(|x| x.is_finite()));
    synth.reset();
    let mut chunked = vec![0.0f32; whole.len()];
    let mut done = 0;
    while done < written {
        let end = (done + 257).min(written);
        done += synth.fill(&mut chunked[done * 2..end * 2]);
    }
    assert!(chunked == whole);
    let mut plain = Synth::new_stereo(&score, RATE, &options()).unwrap();
    let mut uncontrolled = vec![0.0f32; whole.len()];
    plain.fill(&mut uncontrolled);
    assert!(uncontrolled != whole);
}
