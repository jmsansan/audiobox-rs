//! Procedural voices: contract, determinism, DSP behaviour, profiles, text
//! planning and musical notes.
#![cfg(feature = "voice")]
use audiobox::ErrorCode;
use audiobox::synth::{Duty, NoiseMode, Waveform};
use audiobox::voice::*;

const LINE: &str = "¿Qué tal, viajero? ¡Vamos!";

fn options(rate: u32, channels: usize, seed: u64) -> VoiceOptions {
    VoiceOptions {
        sample_rate: rate,
        channels,
        seed,
        ..VoiceOptions::default()
    }
}

fn fnv(samples: &[f32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for s in samples {
        for byte in s.to_bits().to_le_bytes() {
            hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

fn whole(voice: &PreparedUtterance) -> Vec<f32> {
    let mut out = vec![0.0; voice.frames() as usize * voice.channels()];
    assert_eq!(voice.synth().fill(&mut out) as u64, voice.frames());
    out
}

fn chunked(voice: &PreparedUtterance, sizes: &[usize]) -> Vec<f32> {
    let mut synth = voice.synth();
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let frames = sizes[i % sizes.len()];
        i += 1;
        let mut buf = vec![0.0; frames * voice.channels()];
        let n = synth.fill(&mut buf);
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n * voice.channels()]);
    }
    out
}

fn speak(preset: VoicePreset, rate: u32, channels: usize, seed: u64) -> PreparedUtterance {
    let profile = VoiceProfile::preset(preset);
    let plan = plan_gibberish(LINE, &profile, &GibberishOptions::default()).unwrap();
    PreparedUtterance::new(&profile, &plan, &options(rate, channels, seed)).unwrap()
}

/// Frequency from the mean interval of rising zero crossings.
fn pitch(samples: &[f32], rate: u32) -> f64 {
    let crossings: Vec<usize> = (1..samples.len())
        .filter(|&i| samples[i - 1] < 0.0 && samples[i] >= 0.0)
        .collect();
    let span = (crossings[crossings.len() - 1] - crossings[0]) as f64;
    rate as f64 * (crossings.len() - 1) as f64 / span
}

/// Frequency from the strongest normalised autocorrelation lag in `lo..=hi` Hz.
fn autocorr_pitch(samples: &[f32], rate: u32, lo: f64, hi: f64) -> f64 {
    let (min_lag, max_lag) = ((rate as f64 / hi) as usize, (rate as f64 / lo) as usize);
    let n = samples.len() - max_lag;
    let score = |lag: usize| {
        let (mut xy, mut xx, mut yy) = (0.0, 0.0, 0.0);
        for i in 0..n {
            let (x, y) = (samples[i] as f64, samples[i + lag] as f64);
            xy += x * y;
            xx += x * x;
            yy += y * y;
        }
        xy / (xx * yy).sqrt()
    };
    let best = (min_lag..=max_lag)
        .max_by(|&a, &b| score(a).total_cmp(&score(b)))
        .unwrap();
    // Parabolic refinement around the best lag.
    let (l, c, r) = (score(best - 1), score(best), score(best + 1));
    let shift = 0.5 * (l - r) / (l - 2.0 * c + r);
    rate as f64 / (best as f64 + shift)
}
/// Energy of `samples` at `hz` (Goertzel).
fn energy(samples: &[f32], hz: f64, rate: u32) -> f64 {
    let w = std::f64::consts::TAU * hz / rate as f64;
    let (mut s1, mut s2) = (0.0, 0.0);
    for &x in samples {
        let s = x as f64 + 2.0 * w.cos() * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2
}

#[test]
fn offline_and_any_chunking_give_identical_samples() {
    for preset in VoicePreset::ALL {
        for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
            for channels in [1, 2] {
                let voice = speak(preset, rate, channels, 7);
                let reference = whole(&voice);
                assert_eq!(reference.len() as u64, voice.frames() * channels as u64);
                for sizes in [&[1][..], &[3, 1, 7, 64, 5], &[512], &[1 << 22]] {
                    assert_eq!(
                        chunked(&voice, sizes),
                        reference,
                        "{preset} {rate} {channels} {sizes:?}"
                    );
                }
                if channels == 2 {
                    assert!(reference.chunks_exact(2).all(|f| f[0] == f[1]));
                }
                let audio = voice.render().unwrap();
                assert_eq!(
                    audio.channel_data(0).unwrap(),
                    &reference
                        .iter()
                        .step_by(channels)
                        .copied()
                        .collect::<Vec<_>>()[..]
                );
            }
        }
    }
}

#[test]
fn reset_and_seek_replay_exactly() {
    let voice = speak(VoicePreset::AiryBreathy, 48_000, 1, 3);
    let reference = whole(&voice);
    let mut synth = voice.synth();
    let mut buf = vec![0.0; 1000];
    synth.fill(&mut buf);
    synth.reset();
    assert_eq!(synth.position(), 0);
    let mut again = vec![0.0; reference.len()];
    synth.fill(&mut again);
    assert_eq!(again, reference);
    assert!(synth.is_finished());
    assert_eq!(synth.fill(&mut buf), 0);
    for at in [
        0,
        1,
        4321,
        reference.len() as u64 - 1,
        reference.len() as u64,
    ] {
        synth.seek(at).unwrap();
        let mut rest = vec![0.0; reference.len() - at as usize];
        synth.fill(&mut rest);
        assert_eq!(&rest[..], &reference[at as usize..]);
    }
    // Past the end moves to the end; forward seeks continue without a reset.
    synth.seek(u64::MAX).unwrap();
    assert!(synth.is_finished());
    assert_eq!(synth.fill(&mut buf), 0);
    synth.seek(10).unwrap();
    synth.seek(4000).unwrap();
    let mut rest = vec![0.0; reference.len() - 4000];
    synth.fill(&mut rest);
    assert_eq!(&rest[..], &reference[4000..]);
}

#[test]
fn fill_leaves_the_unwritten_suffix_untouched() {
    let voice = speak(VoicePreset::Robot, 48_000, 2, 0);
    let mut synth = voice.synth();
    let mut buf = vec![9.0f32; 7];
    assert_eq!(synth.fill(&mut buf), 3);
    assert_eq!(buf[6], 9.0);
    // At the end, a larger buffer receives only the remaining frames.
    synth.seek(voice.frames() - 2).unwrap();
    let mut buf = vec![9.0f32; 10];
    assert_eq!(synth.fill(&mut buf), 2);
    assert!(buf[4..].iter().all(|&s| s == 9.0));
}

#[test]
fn renders_are_independent_of_other_voices_and_render_order() {
    let a = speak(VoicePreset::SmallCreature, 48_000, 1, 1);
    let b = speak(VoicePreset::LargeCreature, 48_000, 1, 2);
    let (ra, rb) = (whole(&a), whole(&b));
    let (mut sa, mut sb) = (a.synth(), b.synth());
    let (mut oa, mut ob) = (Vec::new(), Vec::new());
    let mut buf = vec![0.0; 333];
    loop {
        let nb = sb.fill(&mut buf);
        ob.extend_from_slice(&buf[..nb]);
        let na = sa.fill(&mut buf[..100]);
        oa.extend_from_slice(&buf[..na]);
        if na == 0 && nb == 0 {
            break;
        }
    }
    assert_eq!(oa, ra);
    assert_eq!(ob, rb);
    // Seeds change only the seeded noise and wander.
    let c = speak(VoicePreset::SmallCreature, 48_000, 1, 99);
    assert_ne!(whole(&c), ra);
    assert_eq!(c.frames(), a.frames());
}

/// Pinned fingerprints: native, Rust 1.85 and WebAssembly must agree. Render
/// version 1, profile format 1, planner 1, 48 kHz mono, seed 7.
#[test]
fn pinned_fingerprints() {
    let got: Vec<(VoicePreset, u64)> = VoicePreset::ALL
        .into_iter()
        .map(|p| (p, fnv(&whole(&speak(p, 48_000, 1, 7)))))
        .collect();
    let expected = [
        (VoicePreset::Robot, 15103278241328193975),
        (VoicePreset::SmallCreature, 7432562417897243475),
        (VoicePreset::LargeCreature, 15801126200802294746),
        (VoicePreset::SoftRounded, 13667161877272381674),
        (VoicePreset::AiryBreathy, 6811615140581585016),
    ];
    assert_eq!(got, expected);
    let notes = [
        VocalNote::new(0, 9000, 220.0, Vowel::A).with_onset(Onset::Plosive),
        VocalNote::new(9000, 9000, 330.0, Vowel::O).legato(),
        VocalNote::new(20_000, 5000, 247.0, Vowel::I).glide_to(262.0),
    ];
    let robot = VoiceProfile::robot(Waveform::Triangle);
    let a = PreparedUtterance::from_notes(&robot, &notes, &options(44_100, 1, 7)).unwrap();
    let b = PreparedUtterance::from_notes(&VoiceProfile::default(), &notes, &options(44_100, 1, 7))
        .unwrap();
    assert_eq!(
        (fnv(&whole(&a)), fnv(&whole(&b))),
        (3752286465004129888, 1120722267650861161)
    );
}

#[test]
fn timing_rounds_absolute_boundaries_once() {
    let profile = VoiceProfile::robot(Waveform::Triangle);
    // 44101 Hz: 1 ms is 44.101 frames, so per-event rounding would drift.
    let events = vec![VocalEvent::vowel(Vowel::A, 1_000, 200.0); 1000];
    let voice = PreparedUtterance::new(
        &profile,
        &UtterancePlan::new(events),
        &options(44_101, 1, 0),
    )
    .unwrap();
    assert_eq!(voice.body_frames(), 44_101);
    let release = (profile.release * 44.101f64).ceil() as u64;
    let ringout = (0.04f64 * 44_101.0).ceil() as u64;
    assert_eq!(voice.frames(), 44_101 + release + ringout);
    let samples = whole(&voice);
    assert_eq!(*samples.last().unwrap(), 0.0);
    // A gesture shorter than half a frame is skipped.
    let tiny = UtterancePlan::new(vec![VocalEvent::vowel(Vowel::A, 5, 200.0)]);
    let voice = PreparedUtterance::new(&profile, &tiny, &options(48_000, 1, 0)).unwrap();
    assert_eq!(voice.frames(), 0);
    assert!(voice.render().unwrap().frames() == 0);
}

#[test]
fn empty_and_invalid_inputs_have_stable_errors() {
    let profile = VoiceProfile::default();
    let empty = PreparedUtterance::new(
        &profile,
        &UtterancePlan::default(),
        &VoiceOptions::default(),
    )
    .unwrap();
    assert_eq!((empty.frames(), empty.body_frames()), (0, 0));
    assert_eq!(empty.synth().fill(&mut [0.0; 8]), 0);
    let plan = |e| UtterancePlan::new(vec![e]);
    let ok = VocalEvent::vowel(Vowel::A, 100_000, 200.0);
    let code = |p: &VoiceProfile, plan: &UtterancePlan, o: &VoiceOptions| {
        PreparedUtterance::new(p, plan, o).unwrap_err().code
    };
    let d = VoiceOptions::default();
    for bad in [f64::NAN, f64::INFINITY, -1.0, 10.0, 5000.0] {
        assert_eq!(
            code(&profile, &plan(VocalEvent::vowel(Vowel::A, 1000, bad)), &d),
            ErrorCode::InvalidArgument
        );
    }
    assert_eq!(
        code(&profile, &plan(ok.with_level(1.5)), &d),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        code(&profile, &plan(ok.with_level(f64::NAN)), &d),
        ErrorCode::InvalidArgument
    );
    // Non-voiced gestures ignore pitch.
    PreparedUtterance::new(&profile, &plan(VocalEvent::rest(1000)), &d).unwrap();
    // Pitch near Nyquist with modulation.
    assert_eq!(
        code(
            &profile,
            &plan(VocalEvent::vowel(Vowel::A, 1000, 3600.0)),
            &options(8_000, 1, 0)
        ),
        ErrorCode::InvalidArgument
    );
    for rate in [7_999, 192_001] {
        assert_eq!(
            code(&profile, &plan(ok), &options(rate, 1, 0)),
            ErrorCode::InvalidArgument
        );
    }
    assert_eq!(
        code(&profile, &plan(ok), &options(48_000, 3, 0)),
        ErrorCode::InvalidArgument
    );
    let raised = VoiceOptions {
        limits: VoiceLimits {
            max_events: MAX_EVENTS + 1,
            ..VoiceLimits::default()
        },
        ..d
    };
    assert_eq!(
        code(&profile, &plan(ok), &raised),
        ErrorCode::InvalidArgument
    );
    let few = VoiceOptions {
        limits: VoiceLimits {
            max_events: 2,
            ..VoiceLimits::default()
        },
        ..d
    };
    assert_eq!(
        code(&profile, &UtterancePlan::new(vec![ok; 3]), &few),
        ErrorCode::LimitExceeded
    );
    let short = VoiceOptions {
        limits: VoiceLimits {
            max_seconds: 0.2,
            ..VoiceLimits::default()
        },
        ..d
    };
    assert_eq!(code(&profile, &plan(ok), &short), ErrorCode::LimitExceeded);
    let long = UtterancePlan::new(vec![VocalEvent::vowel(Vowel::A, u32::MAX, 200.0); 30]);
    assert_eq!(code(&profile, &long, &d), ErrorCode::LimitExceeded);
    let broken = VoiceProfile {
        open: 2.0,
        ..profile
    };
    assert_eq!(code(&broken, &plan(ok), &d), ErrorCode::InvalidArgument);
}

#[test]
fn extreme_accepted_profiles_stay_finite_and_bounded() {
    let edges = |p: VoiceProfile| {
        let mut out = Vec::new();
        for resonance in [0.5, 2.0] {
            for bandwidth in [0.25, 4.0] {
                for (open, tilt, breath) in [(0.3, 0.0, 0.0), (0.9, 1.0, 1.0)] {
                    out.push(VoiceProfile {
                        resonance,
                        bandwidth,
                        open,
                        tilt,
                        breath,
                        vibrato: 100.0,
                        vibrato_rate: 12.0,
                        jitter: 50.0,
                        shimmer: 0.5,
                        gain: 2.0,
                        attack: 1.0,
                        release: 1.0,
                        transition: 1.0,
                        fricative: 12_000.0,
                        ..p
                    });
                }
            }
        }
        out
    };
    let mut profiles = edges(VoiceProfile::default());
    profiles.extend(edges(VoiceProfile {
        mode: VoiceMode::Formant(VoiceSource::Saw),
        ..VoiceProfile::default()
    }));
    for wave in [
        Waveform::Pulse(Duty::Eighth),
        Waveform::Sawtooth,
        Waveform::Noise(NoiseMode::Short),
    ] {
        profiles.extend(edges(VoiceProfile::robot(wave)));
    }
    let mut events = Vec::new();
    for (i, vowel) in Vowel::ALL.into_iter().enumerate() {
        let a = [
            Articulation::Breath,
            Articulation::Fricative,
            Articulation::Closure,
            Articulation::Breath,
        ][i % 4];
        events.push(VocalEvent::vowel(vowel, 3_000, 30.0).glide_to(1500.0));
        events.push(VocalEvent::gesture(a, vowel, 2_000));
        events.push(VocalEvent::vowel(vowel, 40_000, 1500.0).glide_to(30.0));
    }
    let plan = UtterancePlan::new(events);
    for rate in [8_000, 48_000, 192_000] {
        for p in &profiles {
            let top = 0.44 * rate as f64 / 2f64.powf(150.0 / 1200.0);
            let mut plan = plan.clone();
            if top < 1500.0 {
                // Keep pitches valid at 8 kHz.
                plan = UtterancePlan::new(
                    plan.events()
                        .iter()
                        .map(|e| VocalEvent {
                            hz: [e.hz[0].min(top), e.hz[1].min(top)],
                            ..*e
                        })
                        .collect(),
                );
            }
            let voice = PreparedUtterance::new(p, &plan, &options(rate, 1, 5)).unwrap();
            let mut synth = voice.synth();
            let mut out = vec![0.0; voice.frames() as usize];
            synth.fill(&mut out);
            assert!(
                out.iter().all(|s| s.is_finite() && s.abs() < 1.0),
                "{p:?} at {rate}"
            );
            assert!(synth.peak_before_guard().is_finite());
        }
    }
}

#[test]
fn robot_pitch_follows_the_events() {
    let profile = VoiceProfile::robot(Waveform::Triangle);
    let plan = UtterancePlan::new(vec![
        VocalEvent::vowel(Vowel::A, 500_000, 200.0),
        VocalEvent::vowel(Vowel::A, 500_000, 300.0),
        VocalEvent::vowel(Vowel::A, 1_000_000, 200.0).glide_to(400.0),
    ]);
    let voice = PreparedUtterance::new(&profile, &plan, &options(48_000, 1, 0)).unwrap();
    let s = whole(&voice);
    let near = |a: f64, b: f64| (a / b - 1.0).abs() < 0.005;
    assert!(near(pitch(&s[2_400..24_000], 48_000), 200.0));
    assert!(near(pitch(&s[26_400..48_000], 48_000), 300.0));
    // A geometric glide is at the geometric mean halfway: 200·√2 Hz.
    let mid = 48_000 + 24_000;
    assert!(near(
        pitch(&s[mid - 480..mid + 480], 48_000),
        200.0 * 2f64.sqrt()
    ));
    // The envelope reaches full level within the attack and the body holds it.
    let attack = (profile.attack * 48.0) as usize;
    let peak_early = s[..attack / 2].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let peak_body = s[attack * 4..24_000]
        .iter()
        .fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak_early < peak_body * 0.7);
    // Silence returns after the tail.
    assert_eq!(s[s.len() - 1], 0.0);
}

#[test]
fn transitions_have_no_large_steps() {
    // A smooth voice moving between vowels, gestures and pitches never jumps by
    // more than a small fraction of full scale between adjacent samples.
    let profile = VoiceProfile::default();
    let mut events = Vec::new();
    for vowel in Vowel::ALL {
        events.push(VocalEvent::vowel(vowel, 70_000, 180.0).glide_to(240.0));
        events.push(VocalEvent::gesture(Articulation::Closure, vowel, 5_000));
        events.push(VocalEvent::vowel(vowel, 3_000, 400.0));
        events.push(VocalEvent::gesture(Articulation::Breath, vowel, 20_000));
    }
    let voice = PreparedUtterance::new(
        &profile,
        &UtterancePlan::new(events),
        &options(48_000, 1, 0),
    )
    .unwrap();
    let s = whole(&voice);
    let worst = s
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    let peak = s.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(worst < 0.5 * peak, "step {worst} with peak {peak}");
    assert!(s[0].abs() < 1e-6);
}

#[test]
fn vowels_put_energy_at_their_formants() {
    // Breath through the resonances shows the vowel's envelope without harmonics.
    let profile = VoiceProfile {
        tilt: 0.0,
        ..VoiceProfile::default()
    };
    let render = |vowel| {
        let plan = UtterancePlan::new(vec![VocalEvent::gesture(
            Articulation::Breath,
            vowel,
            1_000_000,
        )]);
        let voice = PreparedUtterance::new(&profile, &plan, &options(48_000, 1, 0)).unwrap();
        whole(&voice)[10_000..40_000].to_vec()
    };
    let (i, u) = (render(Vowel::I), render(Vowel::U));
    // I has F2 at 2250 Hz; U has F2 at 750 Hz.
    let ratio = |s: &[f32]| energy(s, 2250.0, 48_000) / energy(s, 750.0, 48_000);
    assert!(
        ratio(&i) > 10.0 * ratio(&u),
        "{} vs {}",
        ratio(&i),
        ratio(&u)
    );
    let (a, iy) = (render(Vowel::A), i);
    // A has F1 at 800 Hz; I has F1 at 300 Hz.
    let low = |s: &[f32]| energy(s, 800.0, 48_000) / energy(s, 300.0, 48_000);
    assert!(low(&a) > 4.0 * low(&iy));
}

#[test]
fn pitch_and_resonance_are_independent() {
    let render = |profile: VoiceProfile, hz: f64| {
        let plan = UtterancePlan::new(vec![VocalEvent::vowel(Vowel::A, 500_000, hz)]);
        let voice = PreparedUtterance::new(&profile, &plan, &options(48_000, 1, 0)).unwrap();
        whole(&voice)[4_800..19_200].to_vec()
    };
    let base = VoiceProfile {
        vibrato: 0.0,
        jitter: 0.0,
        shimmer: 0.0,
        breath: 0.0,
        mode: VoiceMode::Formant(VoiceSource::Saw),
        ..VoiceProfile::default()
    };
    let small = VoiceProfile {
        resonance: 1.5,
        ..base
    };
    // Spectral centroid over harmonic bins.
    let centroid = |s: &[f32], f0: f64| {
        let (mut num, mut den) = (0.0, 0.0);
        for k in 1..=40 {
            let e = energy(s, f0 * k as f64, 48_000);
            num += e * f0 * k as f64;
            den += e;
        }
        num / den
    };
    let (a, b, c) = (
        render(base, 150.0),
        render(small, 150.0),
        render(base, 300.0),
    );
    // Resonance moves the spectral envelope, not the fundamental.
    assert!((centroid(&b, 150.0) / centroid(&a, 150.0)) > 1.25);
    // Doubling the pitch keeps the spectral envelope, so the centroid barely moves.
    let pitch_shift = centroid(&c, 300.0) / centroid(&a, 150.0);
    let resonance_shift = centroid(&b, 150.0) / centroid(&a, 150.0);
    assert!(resonance_shift > pitch_shift);
    assert!((pitch_shift - 1.0).abs() < 0.25, "{pitch_shift}");
    // And resonance leaves the fundamental alone.
    let strongest_low = |s: &[f32]| energy(s, 150.0, 48_000) / energy(s, 75.0, 48_000);
    assert!(strongest_low(&b) > 10.0);
}

#[test]
fn high_formants_drop_out_at_low_rates() {
    let profile = VoiceProfile {
        resonance: 2.0,
        ..VoiceProfile::default()
    };
    let plan = UtterancePlan::new(vec![VocalEvent::vowel(Vowel::A, 1000, 200.0)]);
    let at = |rate| {
        PreparedUtterance::new(&profile, &plan, &options(rate, 1, 0))
            .unwrap()
            .active_formants()
    };
    assert_eq!(at(8_000), [true, false, false, false]);
    assert_eq!(at(48_000), [true; 4]);
    let robot = PreparedUtterance::new(
        &VoiceProfile::robot(Waveform::Triangle),
        &plan,
        &options(48_000, 1, 0),
    )
    .unwrap();
    assert_eq!(robot.active_formants(), [false; 4]);
}

#[test]
fn profile_text_round_trips_exactly() {
    for preset in VoicePreset::ALL {
        let p = VoiceProfile::preset(preset);
        let text = p.to_string();
        assert_eq!(VoiceProfile::parse(&text).unwrap(), p, "{text}");
        assert_eq!(preset.id().parse::<VoicePreset>().unwrap(), preset);
        assert_eq!(
            VoiceProfile::parse(&format!("version=1 preset={preset}")).unwrap(),
            p
        );
        p.validate(8_000).unwrap();
        p.validate(192_000).unwrap();
    }
    for wave in [
        Waveform::Pulse(Duty::Eighth),
        Waveform::Pulse(Duty::ThreeQuarters),
        Waveform::Triangle,
        Waveform::Sawtooth,
        Waveform::Noise(NoiseMode::Long),
        Waveform::Noise(NoiseMode::Short),
    ] {
        let p = VoiceProfile::robot(wave);
        assert_eq!(VoiceProfile::parse(&p.to_string()).unwrap(), p);
    }
    // Neighbouring values, negative zero and subnormals survive bit for bit.
    let odd = VoiceProfile {
        breath: f64::from_bits(1),
        tilt: -0.0,
        range: 0.1 + 0.2,
        pitch: f64::from_bits(200f64.to_bits() + 1),
        ..VoiceProfile::default()
    };
    let back = VoiceProfile::parse(&odd.to_string()).unwrap();
    assert_eq!(back.breath.to_bits(), 1);
    assert_eq!(back.tilt.to_bits(), (-0.0f64).to_bits());
    assert_eq!(back.range.to_bits(), (0.1f64 + 0.2).to_bits());
    assert_eq!(back.pitch.to_bits(), odd.pitch.to_bits());
    // Overrides apply after the preset wherever they appear.
    let p = VoiceProfile::parse("# a comment\npitch=300 version=1\n  preset=robot wave=triangle")
        .unwrap();
    assert_eq!(p.pitch, 300.0);
    assert_eq!(p.mode, VoiceMode::Robot(Waveform::Triangle));
    assert_eq!(p.attack, VoiceProfile::preset(VoicePreset::Robot).attack);
}

#[test]
fn profile_text_errors_are_positioned() {
    let err = |text: &str| VoiceProfile::parse(text).unwrap_err();
    let e = err("pitch=200");
    assert_eq!((e.code, e.line), (ErrorCode::Syntax, 1));
    let e = err("version=2");
    assert_eq!(
        (e.code, e.line, e.column),
        (ErrorCode::InvalidArgument, 1, 9)
    );
    let e = err("version=1\n  pitch=2000");
    assert_eq!(
        (e.code, e.line, e.column),
        (ErrorCode::InvalidArgument, 2, 9)
    );
    let e = err("version=1\npitch=abc");
    assert_eq!((e.code, e.line, e.column), (ErrorCode::Syntax, 2, 7));
    let e = err("version=1 colour=red");
    assert_eq!((e.code, e.column), (ErrorCode::Syntax, 11));
    let e = err("version=1 gain=1 gain=1");
    assert_eq!((e.code, e.column), (ErrorCode::Syntax, 18));
    assert_eq!(err("version=1 pitch").code, ErrorCode::Syntax);
    assert_eq!(
        err("version=1 preset=ghost").code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        err("version=1 mode=robot source=saw").code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        err("version=1 mode=formant wave=pulse").code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        err("version=1 mode=robot wave=saw duty=25").code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        err("version=1 mode=robot wave=pulse noise=long").code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(err("version=1 mode=tape").code, ErrorCode::Syntax);
    assert_eq!(err("version=1 gain=NaN").code, ErrorCode::InvalidArgument);
    assert_eq!(err("version=1 gain=inf").code, ErrorCode::InvalidArgument);
    assert_eq!(
        err(&format!("version=1 {}", " ".repeat(16 * 1024))).code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        "version=1 gain=9".parse::<VoiceProfile>().unwrap_err().code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn gibberish_markers_are_valid_sorted_spans() {
    let texts = [
        LINE,
        "Hola, ¿cómo estás? ¡Muy bien!!! ¿¡Qué!? Ñandú… ok.",
        "e\u{301}xito cafe\u{301} 👋🏽 hola👨‍👩‍👧 n\u{303}",
        "Aaaaaaaaaaaaah!!!! Nooooo...",
        "  \t\n mucho   espacio \n ",
        "l'été d'août — co-op 3.5",
        "Привет, мир! Γειά σου. こんにちは、世界。안녕하세요?",
        "",
        "?!...,,;;",
        "👋🏽🎉",
    ];
    let profile = VoiceProfile::default();
    for text in texts {
        let plan = plan_gibberish(text, &profile, &GibberishOptions::default()).unwrap();
        let has_letters = text.chars().any(|c| c.is_alphanumeric());
        assert_eq!(plan.events().is_empty(), !has_letters, "{text:?}");
        let voice = PreparedUtterance::new(&profile, &plan, &options(48_000, 1, 0)).unwrap();
        let mut last = (0usize, MarkerKind::Phrase);
        let mut last_frame = 0;
        for (m, p) in plan.markers().iter().zip(voice.markers()) {
            assert!(text.is_char_boundary(m.bytes.start) && text.is_char_boundary(m.bytes.end));
            assert!(m.bytes.start < m.bytes.end && !m.events.is_empty());
            assert!((m.bytes.start, m.kind) >= last, "{text:?} unsorted");
            last = (m.bytes.start, m.kind);
            assert!(p.frames.start < p.frames.end && p.frames.end <= voice.body_frames());
            assert!(p.frames.start >= last_frame);
            last_frame = p.frames.start;
            if m.kind == MarkerKind::Syllable {
                let first = text[m.bytes.clone()].chars().next().unwrap();
                assert!(!first.is_whitespace() && first != '\u{301}');
            }
        }
        // Words are nested in phrases, syllables in words.
        for w in plan.markers().iter().filter(|m| m.kind == MarkerKind::Word) {
            assert!(plan.markers().iter().any(|p| p.kind == MarkerKind::Phrase
                && p.bytes.start <= w.bytes.start
                && w.bytes.end <= p.bytes.end));
        }
    }
    let plan = plan_gibberish("e\u{301}xito", &profile, &GibberishOptions::default()).unwrap();
    let syllables: Vec<&str> = plan
        .markers()
        .iter()
        .filter(|m| m.kind == MarkerKind::Syllable)
        .map(|m| &plan.text().unwrap()[m.bytes.clone()])
        .collect();
    assert_eq!(syllables, ["e\u{301}xi", "to"]);
    let plan = plan_gibberish(LINE, &profile, &GibberishOptions::default()).unwrap();
    let phrases: Vec<&str> = plan
        .markers()
        .iter()
        .filter(|m| m.kind == MarkerKind::Phrase)
        .map(|m| &LINE[m.bytes.clone()])
        .collect();
    assert_eq!(phrases, ["¿Qué tal, viajero?", "¡Vamos!"]);
}

#[test]
fn gibberish_follows_its_timing_and_contour_rules() {
    let profile = VoiceProfile::default();
    let rigid = GibberishOptions {
        delivery: Delivery::Rigid,
        ..GibberishOptions::default()
    };
    let plan = |text: &str, o: &GibberishOptions| plan_gibberish(text, &profile, o).unwrap();
    let voiced = |p: &UtteranceKit| {
        p.0.events()
            .iter()
            .filter(|e| e.articulation == Articulation::Voiced)
            .copied()
            .collect::<Vec<_>>()
    };
    struct UtteranceKit(UtterancePlan);
    let rise = 2f64.powf(0.7 * profile.range / 12.0);
    let q = UtteranceKit(plan("bababa?", &rigid));
    let last = *voiced(&q).last().unwrap();
    let start = profile.pitch * 2f64.powf(-0.1 * profile.range / 12.0);
    assert!((last.hz[0] / start - 1.0).abs() < 1e-9);
    assert!((last.hz[1] / (profile.pitch * rise) - 1.0).abs() < 1e-9);
    let s = UtteranceKit(plan("bababa.", &rigid));
    let last = *voiced(&s).last().unwrap();
    assert!(last.hz[1] < last.hz[0]);
    let e = UtteranceKit(plan("bababa!", &rigid));
    let last = *voiced(&e).last().unwrap();
    assert!(last.hz[0] > profile.pitch && last.level == 1.0);
    // Rigid syllables: all but the last at base pitch and full level.
    for ev in &voiced(&s)[..1] {
        assert_eq!((ev.hz, ev.level), ([profile.pitch; 2], 1.0));
    }
    // Pauses: word gap 45 ms, comma 220 ms, phrase 380 ms, none at the end.
    let rests = |p: &UtterancePlan| {
        p.events()
            .iter()
            .filter(|e| e.articulation == Articulation::Closure && e.duration_us >= 40_000)
            .map(|e| e.duration_us)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        rests(&plan("ma ma, ma. ma", &rigid)),
        [45_000, 220_000, 380_000]
    );
    assert_eq!(rests(&plan("ma.", &rigid)), Vec::<u32>::new());
    // Opening marks set the contour when no closing mark follows.
    let open = UtteranceKit(plan("¿bababa", &rigid));
    assert_eq!(
        voiced(&open).last().unwrap().hz,
        voiced(&q).last().unwrap().hz
    );
    // Repeated punctuation counts once; a question mark wins.
    let mixed = UtteranceKit(plan("bababa?!!", &rigid));
    assert_eq!(
        voiced(&mixed).last().unwrap().hz,
        voiced(&q).last().unwrap().hz
    );
    assert_eq!(plan("bababa?!!", &rigid).events().len(), q.0.events().len());
}

#[test]
fn expressive_delivery_stays_within_its_bounds() {
    let profile = VoiceProfile::preset(VoicePreset::SmallCreature);
    let text = "Las palabras largas y cortas suenan distintas, con ritmo propio. ¿Verdad?";
    let rigid = plan_gibberish(
        text,
        &profile,
        &GibberishOptions {
            delivery: Delivery::Rigid,
            ..GibberishOptions::default()
        },
    )
    .unwrap();
    let expressive = plan_gibberish(text, &profile, &GibberishOptions::default()).unwrap();
    // Same syllables and markers; different timing, level and pitch.
    assert_eq!(rigid.markers().len(), expressive.markers().len());
    let vow = |p: &UtterancePlan| {
        p.events()
            .iter()
            .map(|e| (e.articulation, e.vowel))
            .collect::<Vec<_>>()
    };
    assert_eq!(vow(&rigid), vow(&expressive));
    assert_ne!(rigid.events(), expressive.events());
    let (lo, hi) = (
        profile.pitch * 2f64.powf(-profile.range / 12.0),
        profile.pitch * 2f64.powf(profile.range / 12.0),
    );
    for e in expressive.events() {
        if e.articulation == Articulation::Voiced {
            assert!(
                e.hz.iter()
                    .all(|&h| h >= lo * 0.999_999 && h <= hi * 1.000_001)
            );
            assert!((0.72..=1.0).contains(&e.level));
        }
    }
    let total = |p: &UtterancePlan| p.duration_us() as f64;
    assert!((total(&expressive) / total(&rigid) - 1.0).abs() < 0.2);
    // Seeds: reproducible, and independent of other plans.
    let again = plan_gibberish(text, &profile, &GibberishOptions::default()).unwrap();
    let _ = plan_gibberish("otra cosa", &profile, &GibberishOptions::default()).unwrap();
    assert_eq!(again, expressive);
    let other = plan_gibberish(
        text,
        &profile,
        &GibberishOptions {
            seed: 5,
            ..GibberishOptions::default()
        },
    )
    .unwrap();
    assert_ne!(other, expressive);
    // The same word gets the same syllables within one seed.
    let rep = plan_gibberish(
        "hola hola",
        &profile,
        &GibberishOptions {
            delivery: Delivery::Rigid,
            ..GibberishOptions::default()
        },
    )
    .unwrap();
    let ev = rep.events();
    let half = (ev.len() - 1) / 2;
    assert_eq!(
        vow(&UtterancePlan::new(ev[..half].to_vec())),
        vow(&UtterancePlan::new(ev[half + 1..].to_vec()))
    );
}

#[test]
fn gibberish_limits() {
    let profile = VoiceProfile::default();
    let limited = |l: VoiceLimits| GibberishOptions {
        limits: l,
        ..GibberishOptions::default()
    };
    let text = "palabra ".repeat(100);
    let code = |o: GibberishOptions| plan_gibberish(&text, &profile, &o).unwrap_err().code;
    assert_eq!(
        code(limited(VoiceLimits {
            max_text_bytes: 10,
            ..VoiceLimits::default()
        })),
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        code(limited(VoiceLimits {
            max_events: 10,
            ..VoiceLimits::default()
        })),
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        code(limited(VoiceLimits {
            max_markers: 10,
            ..VoiceLimits::default()
        })),
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        code(GibberishOptions {
            syllable_us: 10,
            ..GibberishOptions::default()
        }),
        ErrorCode::InvalidArgument
    );
    let big = "a".repeat(MAX_TEXT_BYTES + 1);
    assert_eq!(
        plan_gibberish(&big, &profile, &GibberishOptions::default())
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    // The largest text within the byte limit plans or reports a limit, never panics.
    let words = "ma ".repeat(MAX_TEXT_BYTES / 3);
    match plan_gibberish(&words, &profile, &GibberishOptions::default()) {
        Ok(plan) => assert!(plan.events().len() <= MAX_EVENTS),
        Err(e) => assert_eq!(e.code, ErrorCode::LimitExceeded),
    }
}

#[test]
fn notes_start_end_and_sustain_where_authored() {
    let profile = VoiceProfile {
        vibrato: 0.0,
        jitter: 0.0,
        shimmer: 0.0,
        breath: 0.0,
        ..VoiceProfile::default()
    };
    let rate = 48_000;
    let notes = [
        VocalNote::new(4_800, 24_000, 220.0, Vowel::A),
        VocalNote::new(28_800, 24_000, 330.0, Vowel::A).legato(),
        VocalNote::new(52_800, 24_000, 220.0, Vowel::O).with_velocity(0.5),
    ];
    let voice = PreparedUtterance::from_notes(&profile, &notes, &options(rate, 1, 0)).unwrap();
    assert_eq!(voice.body_frames(), 76_800);
    let s = whole(&voice);
    assert!(
        s[..4_800].iter().all(|&x| x == 0.0),
        "silence before the first note"
    );
    assert!(s[4_800..4_900].iter().any(|&x| x != 0.0));
    let rms =
        |a: &[f32]| (a.iter().map(|&x| x as f64 * x as f64).sum::<f64>() / a.len() as f64).sqrt();
    // Legato: no dip at the join; re-articulated third note dips.
    let join = 28_800;
    assert!(rms(&s[join - 480..join + 480]) > 0.6 * rms(&s[join - 4_800..join - 2_400]));
    assert!(rms(&s[52_800 + 450..52_800 + 650]) < 0.5 * rms(&s[52_800 - 2_400..52_800 - 1_200]));
    // Velocity halves the level of the same vowel at the same pitch.
    let same = [
        VocalNote::new(0, 24_000, 220.0, Vowel::O),
        VocalNote::new(24_000, 24_000, 220.0, Vowel::O).with_velocity(0.5),
    ];
    let v = whole(&PreparedUtterance::from_notes(&profile, &same, &options(rate, 1, 0)).unwrap());
    let ratio = rms(&v[30_000..46_000]) / rms(&v[6_000..22_000]);
    assert!((ratio - 0.5).abs() < 0.03, "{ratio}");
    // Pitch accuracy of the sustained part, after the legato glide.
    let glide = (profile.glide * 48.0) as usize;
    assert!(
        (autocorr_pitch(&s[join + glide + 2_400..join + 20_000], rate, 200.0, 500.0) / 330.0 - 1.0)
            .abs()
            < 0.01
    );
    // Release and tail after the last note.
    assert_eq!(
        voice.frames(),
        76_800 + (profile.release * 48.0).ceil() as u64 + 1_920
    );
    assert_eq!(*s.last().unwrap(), 0.0);
}

#[test]
fn note_rules_and_errors() {
    let profile = VoiceProfile::default();
    let o = options(48_000, 1, 0);
    let err = |notes: &[VocalNote]| {
        PreparedUtterance::from_notes(&profile, notes, &o)
            .unwrap_err()
            .code
    };
    assert_eq!(
        err(&[
            VocalNote::new(100, 100, 220.0, Vowel::A),
            VocalNote::new(150, 100, 220.0, Vowel::A)
        ]),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        err(&[
            VocalNote::new(100, 100, 220.0, Vowel::A),
            VocalNote::new(50, 10, 220.0, Vowel::A)
        ]),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        err(&[VocalNote::new(0, 100, 220.0, Vowel::A).with_velocity(2.0)]),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        err(&[VocalNote::new(0, 100, f64::NAN, Vowel::A)]),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        err(&[VocalNote::new(u64::MAX, 10, 220.0, Vowel::A)]),
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        err(&[VocalNote::new(0, 48_000 * 200, 220.0, Vowel::A)]),
        ErrorCode::LimitExceeded
    );
    // Zero-length, very short and rapid notes are accepted and reproducible.
    let rapid: Vec<VocalNote> = (0..50)
        .map(|i| {
            VocalNote::new(
                i * 300,
                if i % 7 == 0 { 0 } else { 300 },
                200.0 + i as f64 * 10.0,
                Vowel::ALL[i as usize % 5],
            )
            .with_onset(Onset::ALL_FOR_TEST[i as usize % 4])
        })
        .collect();
    let a = PreparedUtterance::from_notes(&profile, &rapid, &o).unwrap();
    assert_eq!(whole(&a), chunked(&a, &[17, 1, 400]));
    assert!(whole(&a).iter().all(|x| x.is_finite() && x.abs() < 1.0));
}

trait AllOnsets {
    const ALL_FOR_TEST: [Onset; 4];
}
impl AllOnsets for Onset {
    const ALL_FOR_TEST: [Onset; 4] = [Onset::None, Onset::Breath, Onset::Fricative, Onset::Plosive];
}

#[test]
fn harmony_voices_stay_sample_aligned() {
    let profile = VoiceProfile::default();
    let part = |hz: f64, seed| {
        let notes = [
            VocalNote::new(960, 24_000, hz, Vowel::A),
            VocalNote::new(24_960, 24_000, hz * 1.25, Vowel::O).legato(),
        ];
        PreparedUtterance::from_notes(&profile, &notes, &options(48_000, 1, seed)).unwrap()
    };
    let voices = [part(220.0, 1), part(277.18, 2), part(329.63, 3)];
    let first = |v: &PreparedUtterance| whole(v).iter().position(|&x| x != 0.0).unwrap();
    for v in &voices {
        assert_eq!(first(v), 960);
        assert_eq!(v.frames(), voices[0].frames());
    }
}

#[test]
fn marker_lookup_finds_the_innermost_span() {
    let voice = speak(VoicePreset::SoftRounded, 48_000, 1, 0);
    for m in voice.markers() {
        let mid = (m.frames.start + m.frames.end) / 2;
        let found = voice.marker_at(m.kind, m.frames.start).unwrap();
        assert!(found.frames.contains(&m.frames.start));
        assert!(voice.marker_at(m.kind, mid).is_some());
    }
    assert!(
        voice
            .marker_at(MarkerKind::Syllable, voice.frames() + 10)
            .is_none()
    );
    let pause = voice
        .markers()
        .iter()
        .find(|m| m.kind == MarkerKind::Phrase)
        .unwrap()
        .frames
        .end;
    assert!(voice.marker_at(MarkerKind::Phrase, pause).is_none());
}

#[test]
fn rapid_vowel_switching_stays_bounded() {
    // Review finding: direct-form resonators grew without bound here. Alternate A
    // and I every 1.875 ms at 192 kHz with the narrowest, fastest-moving formants.
    let profile = VoiceProfile {
        mode: VoiceMode::Formant(VoiceSource::Saw),
        resonance: 0.5,
        bandwidth: 0.25,
        transition: 1.0,
        attack: 1.0,
        release: 1.0,
        gain: 2.0,
        tilt: 0.0,
        breath: 0.0,
        vibrato: 0.0,
        jitter: 0.0,
        shimmer: 0.0,
        ..VoiceProfile::default()
    };
    let events: Vec<VocalEvent> = (0..MAX_EVENTS)
        .map(|i| VocalEvent::vowel(if i % 2 == 0 { Vowel::A } else { Vowel::I }, 1875, 220.0))
        .collect();
    for rate in [48_000, 192_000] {
        let voice = PreparedUtterance::new(
            &profile,
            &UtterancePlan::new(events.clone()),
            &options(rate, 1, 0),
        )
        .unwrap();
        let mut synth = voice.synth();
        let mut out = vec![0.0; voice.frames() as usize];
        synth.fill(&mut out);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() < 1.0), "{rate}");
        assert!(
            synth.peak_before_guard() < 4.0,
            "{rate}: {}",
            synth.peak_before_guard()
        );
    }
}

#[test]
fn zero_length_notes_and_empty_utterances_cost_nothing() {
    let profile = VoiceProfile::default();
    let o = options(48_000, 1, 0);
    // A skipped note neither orders nor lengthens the voice.
    let notes = [
        VocalNote::new(0, 4800, 220.0, Vowel::A),
        VocalNote::new(2000, 0, 220.0, Vowel::A),
        VocalNote::new(5_760_000_000, 0, 220.0, Vowel::A),
    ];
    let voice = PreparedUtterance::from_notes(&profile, &notes, &o).unwrap();
    assert_eq!(voice.body_frames(), 4800);
    // An empty utterance has no tail, even with a zero time limit.
    let zero = VoiceOptions {
        limits: VoiceLimits {
            max_seconds: 0.0,
            ..VoiceLimits::default()
        },
        ..o
    };
    let empty = PreparedUtterance::new(&profile, &UtterancePlan::default(), &zero).unwrap();
    assert_eq!(empty.frames(), 0);
    assert_eq!(
        PreparedUtterance::from_notes(&profile, &[], &zero)
            .unwrap()
            .frames(),
        0
    );
    let silent = [VocalNote::new(100, 0, 220.0, Vowel::A)];
    assert_eq!(
        PreparedUtterance::from_notes(&profile, &silent, &zero)
            .unwrap()
            .frames(),
        0
    );
}
