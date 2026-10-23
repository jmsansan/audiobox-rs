#![cfg(feature = "events")]
use audiobox::events::{EventBank, Placement, Sources, Trigger};
use audiobox::mixer::{
    Attenuation, DistanceModel, Emitter, Fade, Instances, Mixer, MixerConfig, MixerEvent,
    MixerHandle, Play,
};
use audiobox::{Audio, ErrorCode};
use std::sync::Arc;

const RATE: u32 = 1000;
fn setup(queue: usize) -> (Mixer, MixerHandle, Sources) {
    let (m, h) = Mixer::new(MixerConfig {
        sample_rate: RATE,
        channels: 1,
        queue,
        buses: vec!["effects".into()],
        ..Default::default()
    })
    .unwrap();
    let mut s = Sources::default();
    for (name, level) in [("a", 0.1), ("b", 0.2), ("c", 0.3)] {
        s.insert(name, audio(level, 100)).unwrap();
    }
    (m, h, s)
}
fn audio(level: f32, frames: usize) -> Arc<Audio> {
    Arc::new(Audio::new(vec![vec![level; frames]], RATE).unwrap())
}
fn messages(h: &mut MixerHandle) -> Vec<MixerEvent> {
    std::iter::from_fn(|| h.poll_event()).collect()
}
fn error(text: &str) -> audiobox::events::ParseError {
    EventBank::parse(text).err().expect("expected parse error")
}

#[test]
fn parsing_defaults_comments_crlf_and_character_columns() {
    assert!(EventBank::parse("# empty\r\n\t ").unwrap().is_empty());
    let b =
        EventBank::parse("# note\r\n\tevent step sources=a # comment\r\nevent ui.click sources=b")
            .unwrap();
    assert_eq!(b.len(), 2);
    assert_eq!(b.names().collect::<Vec<_>>(), ["step", "ui.click"]);
    let (mut m, mut h, s) = setup(8);
    let mut p = b.prepare(&h, &s).unwrap().player(123);
    let t = p.play(&mut h, "step").unwrap();
    assert_eq!((t.source, t.gain, t.pitch), (0, 1.0, 0.0));
    let mut out = [0.0; 3];
    m.render(&mut out);
    assert_eq!(out, [0.1; 3]);
    let e = error("#\n\u{2003}event step sources=a nope=1");
    assert_eq!((e.line, e.column, e.code), (2, 23, ErrorCode::Syntax));
    assert!(e.to_string().contains("line 2, column 23"));
    let generic: audiobox::Error = e.into();
    assert_eq!(generic.code, ErrorCode::Syntax);
}

#[test]
fn malformed_and_out_of_range_properties_have_stable_codes() {
    for text in [
        "x",
        "event",
        "event é sources=a",
        "event step",
        "event step sources=",
        "event step sources=a,",
        "event step sources=a foo=1",
        "event step sources=a choice=weighted",
        "event step sources=a gain=1 gain=2",
        "event step sources=a pitch=0,1,2",
        "event step sources=a position=1",
        "event step sources=a rate=abc",
        "event step sources=a\nevent step sources=b",
    ] {
        assert_eq!(error(text).code, ErrorCode::Syntax, "{text}");
    }
    for property in [
        "gain=-1",
        "gain=1,0",
        "gain=NaN",
        "gain=inf",
        "pitch=-49,1",
        "pitch=0,49",
        "pan=2",
        "rate=0",
        "fade-in=61",
        "min-gap=61",
        "min-gap=NaN",
        "priority=256",
        "priority=-1",
        "priority=18446744073709551616",
        "max-instances=65536",
        "max-instances=-1",
        "max-instances=18446744073709551616",
        "loop=1,1",
        "loop=-1,10",
        "loop=0,18446744073709551616",
        "position=0,NaN",
        "emitter=0,0,inf",
        "velocity=0,0,0",
        "attenuation=inverse,1,10,1",
        "emitter=0,0,0 position=0,0",
        "emitter=0,0,0 attenuation=linear,5,2,1",
        "emitter=0,0,0 attenuation=inverse,1,2,17",
    ] {
        assert_eq!(
            error(&format!("event step sources=a {property}")).code,
            ErrorCode::InvalidArgument,
            "{property}"
        );
    }
}

#[test]
fn parsing_has_text_event_candidate_and_name_bounds() {
    assert_eq!(
        error(&" ".repeat(1024 * 1024 + 1)).code,
        ErrorCode::LimitExceeded
    );
    let text = (0..1025)
        .map(|i| format!("event e{i} sources=a\n"))
        .collect::<String>();
    assert_eq!(error(&text).code, ErrorCode::LimitExceeded);
    assert_eq!(
        error(&format!("event step sources={}", vec!["a"; 65].join(","))).code,
        ErrorCode::LimitExceeded
    );
    assert!(EventBank::parse(&format!("event {} sources=a", "a".repeat(64))).is_ok());
    assert_eq!(
        error(&format!("event {} sources=a", "a".repeat(65))).code,
        ErrorCode::Syntax
    );
}

#[test]
fn preparation_checks_every_reference_and_loop_candidate_with_locations() {
    let (_, h, mut s) = setup(8);
    for (text, column) in [
        ("event step sources=missing", 12),
        ("event step sources=a bus=missing", 22),
        ("event step sources=a,b loop=0,101", 24),
    ] {
        let e = EventBank::parse(text)
            .unwrap()
            .prepare(&h, &s)
            .err()
            .unwrap();
        assert_eq!(
            (e.line, e.column, e.code),
            (1, column, ErrorCode::InvalidArgument)
        );
    }
    s.insert("short", audio(0.1, 1)).unwrap();
    assert!(
        EventBank::parse("event step sources=a,short choice=sequence loop=0,50")
            .unwrap()
            .prepare(&h, &s)
            .is_err()
    );
    assert!(s.insert("a", audio(0.9, 10)).is_err());
    assert!(s.insert("bad name", audio(0.9, 10)).is_err());
    assert!(
        s.insert("quad", Audio::new(vec![vec![0.0]; 4], RATE).unwrap())
            .is_err()
    );
}

#[test]
fn sequence_wraps_and_shuffle_visits_all_entries_without_boundary_repeats() {
    for choice in ["sequence", "shuffle", "random"] {
        let (_m, mut h, s) = setup(512);
        let b = EventBank::parse(&format!("event step sources=a,b,c choice={choice}"))
            .unwrap()
            .prepare(&h, &s)
            .unwrap();
        let mut p = b.player(0);
        let indices: Vec<_> = (0..300)
            .map(|_| p.play(&mut h, "step").unwrap().source)
            .collect();
        if choice == "random" {
            for i in 0..3 {
                assert!((60..140).contains(&indices.iter().filter(|&&j| j == i).count()));
            }
        } else {
            for cycle in indices.chunks(3) {
                let mut sorted = cycle.to_vec();
                sorted.sort_unstable();
                assert_eq!(sorted, [0, 1, 2]);
                if choice == "sequence" {
                    assert_eq!(cycle, [0, 1, 2]);
                }
            }
            assert!(indices.windows(2).all(|w| w[0] != w[1]));
        }
    }
    let (_m, mut h, s) = setup(16);
    let mut p = EventBank::parse("event single sources=a choice=shuffle")
        .unwrap()
        .prepare(&h, &s)
        .unwrap()
        .player(0);
    for _ in 0..5 {
        assert_eq!(p.play(&mut h, "single").unwrap().source, 0);
    }
}

#[test]
fn seed_and_event_name_determine_choices_and_ranges_independently() {
    let (_m, mut h, s) = setup(4096);
    let text = "event step sources=a,b,c choice=shuffle gain=0.1,0.9 pitch=-3,5\nevent ui sources=b gain=2 pitch=1";
    let b = EventBank::parse(text).unwrap().prepare(&h, &s).unwrap();
    let b2 = EventBank::parse(text.lines().rev().collect::<Vec<_>>().join("\n").as_str())
        .unwrap()
        .prepare(&h, &s)
        .unwrap();
    let mut p = b.player(42);
    let mut q = b2.player(42);
    let mut r = b.player(43);
    let mut different = false;
    for _ in 0..100 {
        let a = p.play(&mut h, "step").unwrap();
        q.play(&mut h, "ui").unwrap();
        h.play(audio(0.2, 1), &Play::default()).unwrap();
        let b = q.play(&mut h, "step").unwrap();
        let c = r.play(&mut h, "step").unwrap();
        assert_eq!((a.source, a.gain, a.pitch), (b.source, b.gain, b.pitch));
        assert!((0.1..=0.9).contains(&a.gain));
        assert!((-3.0..=5.0).contains(&a.pitch));
        different |= (a.source, a.gain, a.pitch) != (c.source, c.gain, c.pitch);
    }
    assert!(different);
}

#[test]
fn failed_requests_preserve_state_including_shuffle_cycle_regeneration() {
    for choice in ["sequence", "shuffle", "random"] {
        let (mut m, mut h, s) = setup(1);
        let b = EventBank::parse(&format!(
            "event step sources=a,b,c choice={choice} gain=0.2,0.8 pitch=-1,1"
        ))
        .unwrap()
        .prepare(&h, &s)
        .unwrap();
        let mut p = b.player(12);
        let mut reference = b.player(12);
        let (_, mut wrong, _) = setup(4);
        for _ in 0..10 {
            h.stop_all(Fade::NONE).unwrap(); // Fill queue, including at shuffle-cycle starts.
            assert_eq!(
                p.play(&mut h, "step").unwrap_err().code,
                ErrorCode::LimitExceeded
            );
            assert_eq!(
                p.play(&mut wrong, "step").unwrap_err().code,
                ErrorCode::InvalidArgument
            );
            assert!(p.play(&mut h, "missing").is_err());
            assert!(
                p.play_with(
                    &mut h,
                    "step",
                    Trigger {
                        placement: Some(Placement::Position([f32::NAN, 0.0])),
                        ..Default::default()
                    }
                )
                .is_err()
            );
            m.render(&mut [0.0; 1]);
            messages(&mut h);
            let a = p.play(&mut h, "step").unwrap();
            m.render(&mut [0.0; 1]);
            messages(&mut h);
            let b = reference.play(&mut h, "step").unwrap();
            m.render(&mut [0.0; 1]);
            messages(&mut h);
            assert_eq!((a.source, a.gain, a.pitch), (b.source, b.gain, b.pitch));
        }
    }
}

#[test]
fn instance_limits_are_shared_across_variants_and_players_but_not_other_events() {
    let (mut m, mut h, s) = setup(32);
    let bank = EventBank::parse("event step sources=a,b choice=sequence max-instances=1\nevent ui sources=a max-instances=1").unwrap().prepare(&h,&s).unwrap();
    let mut p = bank.player(1);
    let mut q = bank.clone().player(1);
    let a = p.play(&mut h, "step").unwrap();
    let b = q.play(&mut h, "step").unwrap();
    let c = p.play(&mut h, "step").unwrap();
    let ui = p.play(&mut h, "ui").unwrap();
    let manual = h
        .play(
            audio(0.2, 20),
            &Play {
                instances: Some(Instances::new(0).max(1)),
                ..Default::default()
            },
        )
        .unwrap();
    m.render(&mut [0.0; 1]);
    let events = messages(&mut h);
    assert!(events.contains(&MixerEvent::Limited(b.voice)));
    assert!(events.contains(&MixerEvent::Limited(c.voice)));
    assert!(!events.contains(&MixerEvent::Limited(a.voice)));
    assert!(!events.contains(&MixerEvent::Limited(ui.voice)));
    assert!(!events.contains(&MixerEvent::Limited(manual)));
    assert_eq!(p.play(&mut h, "step").unwrap().source, 0); // Audio-side refusals consumed choices.
}

#[test]
fn minimum_repeat_gap_is_judged_at_scheduled_start_time() {
    let (mut m, mut h, s) = setup(32);
    let mut p = EventBank::parse("event step sources=a,b choice=sequence min-gap=0.01")
        .unwrap()
        .prepare(&h, &s)
        .unwrap()
        .player(0);
    let mut ids = Vec::new();
    for frame in [0, 9, 10] {
        ids.push(
            p.play_with(
                &mut h,
                "step",
                Trigger {
                    frame: Some(frame),
                    ..Default::default()
                },
            )
            .unwrap()
            .voice,
        );
    }
    m.render(&mut [0.0; 11]);
    let e = messages(&mut h);
    assert!(e.contains(&MixerEvent::Limited(ids[1])));
    assert!(!e.contains(&MixerEvent::Limited(ids[0])));
    assert!(!e.contains(&MixerEvent::Limited(ids[2])));
}

#[test]
fn text_settings_match_direct_mixer_play_and_spatial_overrides() {
    let emitter = Emitter {
        position: [1.0, 0.0, -2.0],
        velocity: [0.0, 0.0, 1.0],
        attenuation: Attenuation {
            model: DistanceModel::Linear,
            min_distance: 1.0,
            max_distance: 20.0,
            rolloff: 1.0,
        },
    };
    for (settings, placement, direct) in [
        (
            "position=1,0",
            None,
            Play {
                position: Some([1.0, 0.0]),
                ..Default::default()
            },
        ),
        (
            "emitter=1,0,-2 velocity=0,0,1 attenuation=linear,1,20,1",
            Some(Placement::Position([-1.0, 0.0])),
            Play {
                position: Some([-1.0, 0.0]),
                ..Default::default()
            },
        ),
        ("position=1,0", Some(Placement::Pan), Play::default()),
        (
            "emitter=1,0,-2 velocity=0,0,1 attenuation=linear,1,20,1",
            None,
            Play {
                emitter: Some(emitter),
                ..Default::default()
            },
        ),
        (
            "position=1,0",
            Some(Placement::Emitter(emitter)),
            Play {
                emitter: Some(emitter),
                ..Default::default()
            },
        ),
    ] {
        let (_, _, s) = setup(32);
        let config = MixerConfig {
            sample_rate: RATE,
            channels: 2,
            buses: vec!["effects".into()],
            ..Default::default()
        };
        let (mut a, mut ha) = Mixer::new(config.clone()).unwrap();
        let (mut b, mut hb) = Mixer::new(config).unwrap();
        let mut p=EventBank::parse(&format!("event step sources=a bus=effects gain=0.5 priority=99 loop=10,80 fade-in=0.01 rate=1 pitch=0 {settings}")).unwrap().prepare(&ha,&s).unwrap().player(0);
        ha.set_bus_gain(ha.bus("effects").unwrap(), 0.5, Fade::NONE)
            .unwrap();
        hb.set_bus_gain(hb.bus("effects").unwrap(), 0.5, Fade::NONE)
            .unwrap();
        p.play_with(
            &mut ha,
            "step",
            Trigger {
                frame: Some(5),
                placement,
            },
        )
        .unwrap();
        let bus = hb.bus("effects").unwrap();
        hb.at(5)
            .play(
                audio(0.1, 100),
                &Play {
                    bus,
                    gain: 0.5,
                    priority: 99,
                    looping: audiobox::mixer::Loop::Region { start: 10, end: 80 },
                    fade_in: Fade::linear(0.01),
                    ..direct
                },
            )
            .unwrap();
        let mut oa = [0.0; 200];
        let mut ob = [0.0; 200];
        a.render(&mut oa);
        b.render(&mut ob);
        assert_eq!(oa, ob);
        assert!(oa.iter().any(|&x| x != 0.0));
    }
}

#[test]
fn priority_and_replacement_policies_reach_mixer_admission() {
    for policy in ["oldest", "quietest"] {
        let (mut m, mut h, s) = setup(16);
        let mut p = EventBank::parse(&format!(
            "event step sources=a,b choice=sequence max-instances=1 policy={policy} priority=200"
        ))
        .unwrap()
        .prepare(&h, &s)
        .unwrap()
        .player(0);
        let a = p.play(&mut h, "step").unwrap();
        let b = p.play(&mut h, "step").unwrap();
        m.render(&mut [0.0; 1]);
        let e = messages(&mut h);
        assert!(e.contains(&MixerEvent::Stolen(a.voice)));
        assert!(!e.contains(&MixerEvent::Limited(b.voice)));
    }
}

#[cfg(feature = "sfx")]
#[test]
fn synthesized_effects_and_scores_are_resolved_like_recordings() {
    let (mut m, mut h, mut s) = setup(32);
    s.insert_effect(
        "jump",
        &audiobox::sfx::Effect::preset(audiobox::sfx::Preset::Jump, 42),
        RATE,
    )
    .unwrap();
    let score = audiobox::synth::Score::parse("instrument tone wave=triangle\nchannel a=@tone c16")
        .unwrap();
    s.insert_score("tune", &score, RATE, &Default::default())
        .unwrap();
    let mut p = EventBank::parse("event jump sources=jump\nevent win sources=tune")
        .unwrap()
        .prepare(&h, &s)
        .unwrap()
        .player(1);
    drop(s);
    p.play(&mut h, "jump").unwrap();
    p.play(&mut h, "win").unwrap();
    let mut output = [0.0; 100];
    m.render(&mut output);
    assert!(output.iter().any(|&x| x != 0.0));
}

#[test]
fn immediate_event_controls_keep_submission_order_after_clock_advance() {
    let (mut m, mut h, s) = setup(32);
    let mut p = EventBank::parse("event step sources=a")
        .unwrap()
        .prepare(&h, &s)
        .unwrap()
        .player(0);
    m.render(&mut [0.0; 25]);
    let first = p.play(&mut h, "step").unwrap();
    h.stop(first.voice, Fade::NONE).unwrap();
    let mut out = [0.0; 10];
    m.render(&mut out);
    assert_eq!(out, [0.0; 10]);
    messages(&mut h);
    let second = p.play(&mut h, "step").unwrap();
    h.set_gain(second.voice, 0.25, Fade::NONE).unwrap();
    m.render(&mut out);
    assert_eq!(out, [0.025; 10]);
}

#[test]
fn event_keys_are_distinct_from_audio_identity_and_other_preparations() {
    let (mut m, mut h, mut s) = setup(32);
    let sound = audio(0.2, 100);
    s.insert("shared", sound.clone()).unwrap();
    let text = EventBank::parse("event step sources=shared max-instances=1").unwrap();
    let mut p = text.prepare(&h, &s).unwrap().player(0);
    let mut q = text.prepare(&h, &s).unwrap().player(0);
    let a = p.play(&mut h, "step").unwrap();
    let b = q.play(&mut h, "step").unwrap();
    let id = h
        .play(
            sound.clone(),
            &Play {
                instances: Some(Instances::of(&sound).max(1)),
                ..Default::default()
            },
        )
        .unwrap();
    let duplicate = p.play(&mut h, "step").unwrap();
    m.render(&mut [0.0; 1]);
    let e = messages(&mut h);
    for voice in [a.voice, b.voice, id] {
        assert!(!e.contains(&MixerEvent::Limited(voice)));
    }
    assert!(e.contains(&MixerEvent::Limited(duplicate.voice)));
}

#[test]
fn priorities_can_refuse_events_without_rewinding_their_choices() {
    let (_, _, s) = setup(8);
    let (mut m, mut h) = Mixer::new(MixerConfig {
        sample_rate: RATE,
        channels: 1,
        max_voices: 1,
        ..Default::default()
    })
    .unwrap();
    h.play(
        audio(0.1, 100),
        &Play {
            priority: 200,
            ..Default::default()
        },
    )
    .unwrap();
    let mut p = EventBank::parse("event low sources=a,b choice=sequence priority=50")
        .unwrap()
        .prepare(&h, &s)
        .unwrap()
        .player(0);
    let request = p.play(&mut h, "low").unwrap();
    m.render(&mut [0.0; 1]);
    assert!(messages(&mut h).contains(&MixerEvent::Rejected(request.voice)));
    assert_eq!(p.play(&mut h, "low").unwrap().source, 1);
}

#[test]
fn seeded_events_have_stable_sample_and_selection_hashes() {
    fn render(block: usize) -> (u64, u64) {
        let (mut m, mut h, s) = setup(128);
        let bank =
            EventBank::parse("event step sources=a,b,c choice=shuffle gain=0.2,0.9 pitch=-1,2")
                .unwrap()
                .prepare(&h, &s)
                .unwrap();
        let mut p = bank.player(42);
        let mut choices = 0xcbf29ce484222325u64;
        for i in 0..12 {
            let t = p
                .play_with(
                    &mut h,
                    "step",
                    Trigger {
                        frame: Some(i * 125),
                        ..Default::default()
                    },
                )
                .unwrap();
            for word in [t.source as u32, t.gain.to_bits(), t.pitch.to_bits()] {
                for byte in word.to_le_bytes() {
                    choices = (choices ^ byte as u64).wrapping_mul(0x100000001b3);
                }
            }
        }
        let mut output = vec![0.0f32; 1700];
        for part in output.chunks_mut(block) {
            m.render(part);
            messages(&mut h);
        }
        let pcm = output
            .iter()
            .flat_map(|s| s.to_bits().to_le_bytes())
            .fold(0xcbf29ce484222325u64, |h, b| {
                (h ^ b as u64).wrapping_mul(0x100000001b3)
            });
        (choices, pcm)
    }
    let result = render(31);
    assert_eq!(result, render(512));
    assert_eq!(result, (9770056229222453497, 474846089411066681));
}
