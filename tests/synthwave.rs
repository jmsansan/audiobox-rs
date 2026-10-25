#![cfg(feature = "synth")]
use audiobox::synth::*;
use audiobox::{Audio, ErrorCode};
const RATE: u32 = 16000;
const REFERENCE:&str="tempo 180
instrument pad wave=saw unison=5 detune=18 spread=90 voices=4 adsr=10,40,70,60 filter=lowpass cutoff=1800 resonance=150 keytrack=40 filter-adsr=20,80,40,50 filter-env=2400 lfo-cutoff=beats,2,1,500 lfo-pan=1500,30
instrument bass wave=triangle voices=1 legato=on glide=60 filter=lowpass cutoff=900 lfo-pitch=3000,12
channel pad=@pad l8 {c e g}!90 {d f a}!100 L [{e g b} {c e g}]2
channel bass=@bass o2 l8 c d L [e c]2";
fn render(text: &str) -> Audio {
    Score::parse(text).unwrap().render_stereo(RATE).unwrap()
}
fn energy(s: &[f32]) -> f64 {
    s.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / s.len() as f64
}
fn magnitude(s: &[f32], hz: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &x) in s.iter().enumerate() {
        let p = std::f64::consts::TAU * hz * i as f64 / RATE as f64;
        re += x as f64 * p.cos();
        im += x as f64 * p.sin();
    }
    2.0 * (re * re + im * im).sqrt() / s.len() as f64
}
#[test]
fn chord_builder_matches_text_and_extends_the_whole_group() {
    let parsed=Score::parse("tempo 60\ninstrument pad wave=saw unison=3 detune=12 spread=80 voices=4\nchannel a=@pad {c e >c}4!90^8 r8").unwrap();
    let mut typed = Score::new(60, 48).unwrap();
    let i = typed
        .add_instrument(
            "pad",
            Instrument {
                synthwave: Some(Synthwave {
                    unison: 3,
                    detune: 12,
                    spread: 80,
                    voices: 4,
                    ..Default::default()
                }),
                waveform: Waveform::Sawtooth,
                ..Default::default()
            },
        )
        .unwrap();
    typed
        .channel("a")
        .unwrap()
        .instrument(i)
        .unwrap()
        .chord_with_velocity(
            &[Pitch::C4, Pitch::new(64).unwrap(), Pitch::new(72).unwrap()],
            Length::QUARTER,
            90,
        )
        .unwrap()
        .tie(Length::EIGHTH)
        .unwrap()
        .rest(Length::EIGHTH)
        .unwrap();
    assert_eq!(typed, parsed);
    assert!(
        parsed.channels()[0]
            .notes()
            .iter()
            .all(|n| n.start == 0 && n.length == 72 && n.velocity == 90)
    );
}
#[test]
fn chords_and_new_parameters_have_bounded_stable_errors() {
    for key in [
        "unison=0",
        "detune=101",
        "spread=101",
        "voices=17",
        "pan=-101",
        "glide=1",
        "legato=on",
        "filter=lowpass cutoff=0",
        "filter=highpass resonance=49",
        "filter=lowpass keytrack=201",
        "filter=lowpass filter-env=9601",
        "filter=lowpass filter-adsr=1,2,101,3",
        "lfo-pitch=20001,1",
        "lfo-amplitude=1000,101",
        "lfo-cutoff=1000,100",
        "lfo-pan=beats,0,1,10",
        "unison=18446744073709551616",
    ] {
        let e = Score::parse(&format!("instrument p {key}\nchannel a=@p c4")).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidArgument, "{key}: {e}");
        assert_eq!(e.line, 1);
        assert!(e.column > 1);
    }
    for key in [
        "unison=abc",
        "unison=2 unison=3",
        "lfo-pitch=beats,1,2",
        "lfo-invalid=2,3",
        "filter=notch",
    ] {
        assert_eq!(
            Score::parse(&format!("instrument p {key}"))
                .unwrap_err()
                .code,
            ErrorCode::Syntax,
            "{key}"
        );
    }
    for body in [
        "{}4",
        "{c c}4",
        "{c e g}4&c4",
        "c4&{c e}4",
        "{c r}4",
        "{c4 e}4",
        "{c e",
        "{{c}}4",
    ] {
        assert!(
            Score::parse(&format!("instrument p unison=2\nchannel a=@p {body}")).is_err(),
            "{body}"
        );
    }
    assert!(Score::parse("instrument p wave=saw\nchannel a=@p {c e}4").is_err());
    assert!(Score::parse("instrument p voices=2\nchannel a=@p {c e g}4").is_err());
    assert!(Score::parse("instrument p unison=2 sample=0001").is_err());
    assert!(Score::parse("instrument p unison=2 drum=kick").is_err());
    assert!(Score::parse("instrument p unison=2 wave=noise").is_err());
    let mut score = Score::with_limits(
        60,
        48,
        ScoreLimits {
            max_notes: 2,
            ..Default::default()
        },
    )
    .unwrap();
    let i = score
        .add_instrument(
            "p",
            Instrument {
                synthwave: Some(Default::default()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut writer = score.channel("a").unwrap();
    writer.instrument(i).unwrap();
    assert_eq!(
        writer
            .chord(
                &[Pitch::C4, Pitch::A4, Pitch::new(64).unwrap()],
                Length::QUARTER
            )
            .err()
            .unwrap()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(writer.position(), 0);
    writer
        .chord(&[Pitch::C4, Pitch::A4], Length::QUARTER)
        .unwrap();
    assert_eq!(score.channels()[0].notes().len(), 2);
}
#[test]
fn a_chord_contains_all_requested_fundamentals() {
    let audio =
        render("tempo 60\ninstrument p wave=triangle synthwave=on\nchannel a=@p {a >c+ e}4");
    let samples = audio.channel_data(0).unwrap();
    for midi in [69, 73, 76] {
        let f = Pitch::new(midi).unwrap().frequency();
        assert!(magnitude(&samples[..RATE as usize], f) > 0.17, "{midi}");
    }
}
#[test]
fn unison_detune_and_spread_have_measured_effects() {
    let a = render("tempo 60\ninstrument p wave=saw unison=2 detune=30 spread=0\nchannel a=@p a2");
    let left = a.channel_data(0).unwrap();
    assert_eq!(left, a.channel_data(1).unwrap());
    for cents in [-30.0f64, 30.0] {
        let hz = 440.0 * 2.0f64.powf(cents / 1200.0);
        assert!(magnitude(left, hz) > 0.065);
    }
    let b =
        render("tempo 60\ninstrument p wave=saw unison=2 detune=30 spread=100\nchannel a=@p a2");
    let side: Vec<_> = b
        .channel_data(0)
        .unwrap()
        .iter()
        .zip(b.channel_data(1).unwrap())
        .map(|(a, b)| a - b)
        .collect();
    assert!(energy(&side) > 0.005);
    let zero = render("tempo 60\ninstrument p wave=pulse unison=2 detune=0\nchannel a=@p a4");
    assert!(magnitude(zero.channel_data(0).unwrap(), 440.0) > 0.25);
}
#[test]
fn filter_envelope_opens_the_actual_output_spectrum() {
    let a = render(
        "tempo 60\ninstrument p wave=saw synthwave=on filter=lowpass cutoff=200 filter-env=4800 filter-adsr=500,0,100,20\nchannel a=@p a4",
    );
    let s = a.channel_data(0).unwrap();
    let early = magnitude(&s[800..2400], 1320.0);
    let late = magnitude(&s[10000..14000], 1320.0);
    assert!(late > early * 10.0, "early={early} late={late}");
}
#[test]
fn free_and_beat_lfos_follow_the_same_song_clock() {
    let a = render("tempo 120\ninstrument p wave=triangle lfo-amplitude=2000,100\nchannel a=@p a2");
    let b = render(
        "tempo 120\ninstrument p wave=triangle lfo-amplitude=beats,1,1,100\nchannel a=@p a2",
    );
    assert_eq!(a.channel_data(0).unwrap(), b.channel_data(0).unwrap());
    let s = a.channel_data(0).unwrap();
    assert!(energy(&s[5600..6400]) > energy(&s[1600..2400]) * 100.0);
    let pan = render("tempo 120\ninstrument p lfo-pan=2000,100\nchannel a=@p a2");
    let l = pan.channel_data(0).unwrap();
    let r = pan.channel_data(1).unwrap();
    assert!(energy(&r[1600..2400]) > energy(&l[1600..2400]) * 100.0);
    assert!(energy(&l[5600..6400]) > energy(&r[5600..6400]) * 100.0);
}
#[test]
fn offline_stream_chunks_reset_and_stereo_fold_agree() {
    let score = Score::parse(REFERENCE).unwrap();
    let options = RenderOptions {
        loops: Some(2),
        ..Default::default()
    };
    let stereo = score.render_stereo_with(RATE, &options).unwrap();
    let mono = score.render_with(RATE, &options).unwrap();
    for i in 0..mono.frames() {
        assert!(
            (mono.channel_data(0).unwrap()[i]
                - (stereo.channel_data(0).unwrap()[i] + stereo.channel_data(1).unwrap()[i]) * 0.5)
                .abs()
                < 1e-7
        );
    }
    let mut synth = Synth::new_stereo(&score, RATE, &options).unwrap();
    assert_eq!(synth.channels(), 2);
    for block in [1, 7, 257, 4096] {
        synth.reset();
        let mut offset = 0;
        let mut buffer = vec![9.0; block * 2 + 1];
        loop {
            buffer.fill(9.0);
            let n = synth.fill(&mut buffer);
            assert!(buffer[n * 2..].iter().all(|&x| x == 9.0));
            for i in 0..n {
                for c in 0..2 {
                    assert_eq!(
                        buffer[2 * i + c],
                        stereo.channel_data(c).unwrap()[offset + i]
                    );
                }
            }
            offset += n;
            if n < block {
                break;
            }
        }
        assert_eq!(offset, stereo.frames());
    }
}
#[test]
fn chords_crossing_the_loop_point_restart_every_member() {
    let score=Score::parse("tempo 120\ninstrument p wave=triangle synthwave=on\nchannel a=@p {c e g}2\nchannel clock=r4 L r4").unwrap();
    let a = score
        .render_with(
            RATE,
            &RenderOptions {
                loops: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    let s = a.channel_data(0).unwrap();
    assert_eq!(&s[..2000], &s[RATE as usize..RATE as usize + 2000]);
}
#[cfg(feature = "mixer")]
#[test]
fn stereo_synth_is_a_correct_mixer_stream() {
    use audiobox::mixer::{Mixer, MixerConfig, Play};
    let score = Score::parse(REFERENCE).unwrap();
    let audio = score.render_stereo(RATE).unwrap();
    let synth = Synth::new_stereo(&score, RATE, &Default::default()).unwrap();
    let (mut m, mut h) = Mixer::new(MixerConfig {
        sample_rate: RATE,
        channels: 2,
        ..Default::default()
    })
    .unwrap();
    h.play(synth, &Play::default()).unwrap();
    let mut out = vec![0.0; audio.frames() * 2];
    for chunk in out.chunks_mut(514) {
        m.render(chunk);
        while h.poll_event().is_some() {}
    }
    for i in 0..audio.frames() {
        for c in 0..2 {
            assert_eq!(out[2 * i + c], audio.channel_data(c).unwrap()[i]);
        }
    }
}
#[test]
fn a_zero_release_reused_voice_gets_a_fresh_attack_after_a_rest() {
    let score =
        Score::parse("tempo 120\ninstrument p voices=1 adsr=100,0,100,0\nchannel a=@p c4 r4 c4")
            .unwrap();
    let audio = score.render(RATE).unwrap();
    let s = audio.channel_data(0).unwrap();
    assert_eq!(s[0], 0.0);
    assert_eq!(s[RATE as usize], 0.0);
    assert_eq!(&s[..1600], &s[RATE as usize..RATE as usize + 1600]);
    assert!(
        s[RATE as usize..RATE as usize + 100]
            .iter()
            .all(|x| x.abs() < 0.02)
    );
}
#[test]
fn synthwave_samples_have_a_cross_platform_pinned_hash() {
    let audio = render(REFERENCE);
    let mut hash = 0xcbf29ce484222325u64;
    for c in 0..2 {
        for sample in audio.channel_data(c).unwrap() {
            for b in sample.to_bits().to_le_bytes() {
                hash = (hash ^ b as u64).wrapping_mul(0x100000001b3);
            }
        }
    }
    assert_eq!(hash, 1747078165250861946);
}

/// Static, gliding and LFO-modulated voices, a filter envelope and a returning
/// pitch: the reused frequency, filter coefficient and LFO phase of 0.4.1 must
/// render exactly what 0.4.0 rendered.
#[test]
fn reused_parameters_render_as_before() {
    let audio = render(
        "tempo 120
instrument pad wave=saw unison=4 detune=20 spread=80 voices=4 adsr=10,100,70,200 filter=bandpass cutoff=900 resonance=150 keytrack=50
instrument bass wave=pulse voices=1 glide=40 legato=on adsr=2,80,60,50 filter=lowpass cutoff=300 resonance=200 filter-env=3000 filter-adsr=0,150,10,60
instrument lead wave=triangle voices=2 lfo-pitch=6000,25 lfo-cutoff=beats,1,2,900 filter=highpass cutoff=400 lfo-amplitude=3000,40 lfo-pan=beats,3,4,60
channel pad=@pad o4 {c e g}2 {c e g}4 {d f a}4
channel bass=@bass o2 l8 c c g >c< c c g c
channel lead=@lead o5 l8 r4 e g e c",
    );
    let bits: Vec<u32> = audio
        .all_channels()
        .iter()
        .flatten()
        .map(|s| s.to_bits())
        .collect();
    let hash = bits
        .iter()
        .flat_map(|b| b.to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
        });
    assert_eq!(hash, 0x8bccebaa38a879ca);
}
