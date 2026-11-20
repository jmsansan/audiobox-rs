//! Prepared voices as mixer streams and named sound events.
#![cfg(all(feature = "voice", feature = "mixer"))]
use audiobox::ErrorCode;
use audiobox::mixer::{Fade, Loop, Mixer, MixerConfig, Play, VoiceState};
use audiobox::voice::*;

fn voice(rate: u32) -> PreparedUtterance {
    let profile = VoiceProfile::preset(VoicePreset::SoftRounded);
    let plan = plan_gibberish("Hola, viajera.", &profile, &GibberishOptions::default()).unwrap();
    let options = VoiceOptions {
        sample_rate: rate,
        seed: 4,
        ..VoiceOptions::default()
    };
    PreparedUtterance::new(&profile, &plan, &options).unwrap()
}

fn mono_mixer(rate: u32) -> (Mixer, audiobox::mixer::MixerHandle) {
    Mixer::new(MixerConfig {
        sample_rate: rate,
        channels: 1,
        ..MixerConfig::default()
    })
    .unwrap()
}

fn render(mixer: &mut Mixer, frames: usize, block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    let mut buf = vec![0.0; block];
    while out.len() < frames {
        let n = block.min(frames - out.len());
        mixer.render(&mut buf[..n]);
        out.extend_from_slice(&buf[..n]);
    }
    out
}

#[test]
fn mixer_playback_matches_the_offline_render() {
    let v = voice(48_000);
    let reference = v.render().unwrap();
    let reference = reference.channel_data(0).unwrap();
    for block in [1, 37, 256, 4096] {
        let (mut mixer, mut handle) = mono_mixer(48_000);
        let id = handle.play(&v, &Play::default()).unwrap();
        let out = render(&mut mixer, reference.len() + 2048, block);
        let worst = reference
            .iter()
            .zip(&out)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "block {block}: {worst}");
        assert!(out[reference.len()..].iter().all(|&s| s == 0.0));
        while handle.poll_event().is_some() {}
        assert!(
            handle
                .voice_status(id)
                .is_none_or(|s| s.state == VoiceState::Finished)
        );
    }
}

#[test]
fn scheduled_start_start_position_and_whole_loops() {
    let v = voice(48_000);
    let reference = v.render().unwrap();
    let reference = reference.channel_data(0).unwrap().to_vec();
    let (mut mixer, mut handle) = mono_mixer(48_000);
    handle.at(1000).play(&v, &Play::default()).unwrap();
    let out = render(&mut mixer, 1000 + reference.len(), 512);
    assert!(out[..1000].iter().all(|&s| s == 0.0));
    assert!(
        (out[1000..]
            .iter()
            .zip(&reference)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max))
            < 1e-6
    );

    // A start position seeks on the game's thread by replaying.
    let (mut mixer, mut handle) = mono_mixer(48_000);
    let start = Play {
        start: audiobox::TimePosition::Frames(5000),
        ..Play::default()
    };
    handle.play(&v, &start).unwrap();
    let out = render(&mut mixer, reference.len() - 5000, 512);
    assert!(
        (out.iter()
            .zip(&reference[5000..])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max))
            < 1e-6
    );

    // A whole loop restarts with a reset and repeats exactly.
    let (mut mixer, mut handle) = mono_mixer(48_000);
    let looping = Play {
        looping: Loop::Whole,
        ..Play::default()
    };
    handle.play(&v, &looping).unwrap();
    let out = render(&mut mixer, reference.len() * 2, 300);
    let (a, b) = out.split_at(reference.len());
    assert!(
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0f32, f32::max)
            < 1e-6
    );
}

#[test]
fn source_position_follows_pause_and_rate() {
    let v = voice(48_000);
    let (mut mixer, mut handle) = mono_mixer(48_000);
    let id = handle.play(&v, &Play::default()).unwrap();
    render(&mut mixer, 9600, 480);
    let before = handle.voice_status(id).unwrap().position;
    assert_eq!(before, 9600);
    handle.pause(id, Fade::NONE).unwrap();
    render(&mut mixer, 4800, 480);
    let paused = handle.voice_status(id).unwrap();
    assert_eq!(
        (paused.state, paused.position),
        (VoiceState::Paused, before)
    );
    let syllable = v.marker_at(MarkerKind::Syllable, paused.position).cloned();
    handle.resume(id, Fade::NONE).unwrap();
    render(&mut mixer, 480, 480);
    assert!(handle.voice_status(id).unwrap().position > before);
    assert_eq!(v.marker_at(MarkerKind::Syllable, before).cloned(), syllable);

    // A 44.1 kHz mixer resamples a 48 kHz voice; source frames advance at 48 kHz.
    let (mut mixer, mut handle) = mono_mixer(44_100);
    let id = handle.play(&v, &Play::default()).unwrap();
    render(&mut mixer, 44_100 / 2, 441);
    let position = handle.voice_status(id).unwrap().position as f64;
    assert!((position - 24_000.0).abs() < 64.0, "{position}");

    // Double rate plays the source twice as fast.
    let (mut mixer, mut handle) = mono_mixer(48_000);
    let fast = Play {
        rate: 2.0,
        ..Play::default()
    };
    let id = handle.play(&v, &fast).unwrap();
    render(&mut mixer, 4800, 480);
    let position = handle.voice_status(id).unwrap().position as f64;
    assert!((position - 9600.0).abs() < 64.0, "{position}");
    handle.stop(id, Fade::linear(0.01)).unwrap();
    render(&mut mixer, 4800, 480);
    while handle.poll_event().is_some() {}
    assert!(
        handle
            .voice_status(id)
            .is_none_or(|s| s.state == VoiceState::Finished)
    );
}

#[test]
fn stream_seeks_are_exact_and_bounded() {
    use audiobox::mixer::Stream;
    let v = voice(48_000);
    let mut synth = v.synth();
    assert_eq!(Stream::frames(&synth), Some(v.frames()));
    assert_eq!(
        (Stream::sample_rate(&synth), Stream::channels(&synth)),
        (48_000, 1)
    );
    let mut all = vec![0.0; v.frames() as usize];
    synth.read(&mut all).unwrap();
    assert_eq!(synth.read(&mut all).unwrap(), 0);
    for at in [0, 1, v.frames() / 2, v.frames()] {
        Stream::seek(&mut synth, at).unwrap();
        let mut rest = vec![0.0; (v.frames() - at) as usize];
        synth.read(&mut rest).unwrap();
        assert_eq!(&rest[..], &all[at as usize..]);
    }
    assert_eq!(
        Stream::seek(&mut synth, v.frames() + 1).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
}

#[cfg(feature = "events")]
#[test]
fn sound_events_name_prepared_voices() {
    use audiobox::events::{EventBank, Sources};
    let v = voice(48_000);
    let mut sources = Sources::default();
    sources.insert_voice("greeting", &v).unwrap();
    assert_eq!(
        sources.insert_voice("greeting", &v).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        sources.insert_voice("bad name", &v).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let empty = PreparedUtterance::new(
        &VoiceProfile::default(),
        &UtterancePlan::default(),
        &VoiceOptions::default(),
    )
    .unwrap();
    assert_eq!(
        sources.insert_voice("empty", &empty).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let bank = EventBank::parse("event npc.hello sources=greeting").unwrap();
    let missing = EventBank::parse("event npc.bye sources=farewell").unwrap();
    let (mut mixer, mut handle) = mono_mixer(48_000);
    assert!(missing.prepare(&handle, &sources).is_err());
    let prepared = bank.prepare(&handle, &sources).unwrap();
    let mut player = prepared.player(1);
    let reference = v.render().unwrap();
    let reference = reference.channel_data(0).unwrap().to_vec();
    let mut takes = Vec::new();
    for _ in 0..2 {
        player.play(&mut handle, "npc.hello").unwrap();
        takes.push(render(&mut mixer, reference.len() + 512, 256));
    }
    assert_eq!(takes[0], takes[1]);
    assert!(
        takes[0]
            .iter()
            .zip(&reference)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max)
            < 1e-6
    );
    assert!(player.play(&mut handle, "npc.unknown").is_err());
}
