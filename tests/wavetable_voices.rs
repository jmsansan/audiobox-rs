//! Opt-in per-note voices for wavetable instruments (#85): `voices=N` gives each
//! note a fresh attack at phase zero while earlier notes finish their releases.
#![cfg(feature = "synth")]
use audiobox::ErrorCode;
use audiobox::synth::*;

/// The neutral fixture from #85: a stepped table, a slow release, repeated notes.
const TABLE: &str = "wavetable=89acdeefffeedca97653211000112356 adsr=9,420,28,360 volume=13";

fn score(voices: &str, notes: &str) -> Score {
    Score::parse(&format!(
        "tempo 90\ninstrument tone {TABLE} {voices}\nchannel test = @tone {notes}"
    ))
    .unwrap()
}
fn render(score: &Score) -> Vec<f32> {
    score
        .render(48_000)
        .unwrap()
        .channel_data(0)
        .unwrap()
        .to_vec()
}
fn step(samples: &[f32], at: usize) -> f32 {
    (samples[at] - samples[at - 1]).abs()
}
/// The largest adjacent-sample step near `at`, leaving out the onset itself.
fn surrounding(samples: &[f32], at: usize) -> f32 {
    (at - 130..at + 130)
        .filter(|&i| i.abs_diff(at) > 5)
        .map(|i| step(samples, i))
        .fold(0.0, f32::max)
}

#[test]
fn repeated_notes_start_smoothly_while_the_default_stays_raw() {
    let notes = "q8 o4 a4!91 a4!91 a4!91 a4!91";
    let raw = render(&score("", notes));
    let smooth = render(&score("voices=4", notes));
    // At 90 BPM a quarter note is 32,000 frames at 48 kHz.
    for onset in [32_000, 64_000, 96_000] {
        // The raw voice restarts its table at a nonzero level: its onset step stands
        // out from the stepped wave around it.
        assert!(
            step(&raw, onset) > surrounding(&raw, onset),
            "raw onset at {onset}"
        );
        // A fresh voice attacks from silence at phase zero while the last note's
        // release continues, so the onset is no larger than the wave's own steps.
        assert!(
            step(&smooth, onset) <= surrounding(&smooth, onset),
            "smooth onset at {onset}: {} > {}",
            step(&smooth, onset),
            surrounding(&smooth, onset)
        );
    }
    assert_eq!(smooth.len(), raw.len());
}

#[test]
fn an_isolated_note_sounds_exactly_as_the_raw_voice() {
    // The release (360 ms) ends inside each rest (667 ms): nothing overlaps, and a
    // fresh voice renders the raw voice's samples bit for bit.
    let notes = "o4 a4!91 r4 a4!91 r4 >c4!60 r2";
    let raw = render(&score("", notes));
    let smooth = render(&score("voices=4", notes));
    assert_eq!(raw.len(), smooth.len());
    assert!(
        raw.iter()
            .zip(&smooth)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
}

#[test]
fn an_outgoing_release_continues_under_the_next_attack() {
    // Adjacent full gates: A4's release must survive C5's attack, so the pair
    // equals the two notes rendered apart and added.
    let pair = render(&score("voices=2", "q8 o4 a4 >c4 r1"));
    let first = render(&score("voices=2", "q8 o4 a4 r4 r1"));
    let second = render(&score("voices=2", "q8 o4 r4 >c4 r1"));
    assert_eq!(pair.len(), first.len());
    for i in 0..pair.len() {
        let sum = first[i] + second.get(i).copied().unwrap_or(0.0);
        assert!(
            (pair[i] - sum).abs() < 1e-6,
            "frame {i}: {} vs {sum}",
            pair[i]
        );
    }
    // The tail is audible after the second onset.
    assert!(first[32_000..32_000 + 4_800].iter().any(|s| s.abs() > 0.01));
}

#[test]
fn rapid_notes_steal_the_oldest_release_within_the_limit_and_keep_the_duration() {
    let notes = "o4 l32 [a b >c< g]4 r1";
    let raw = render(&score("", notes));
    for voices in [1, 2, 3] {
        let pooled = render(&score(&format!("voices={voices}"), notes));
        assert_eq!(pooled.len(), raw.len(), "{voices} voices");
        assert!(pooled.iter().all(|s| s.is_finite()));
    }
    // More voices keep more tails sounding together, so the output differs.
    let two = render(&score("voices=2", notes));
    let three = render(&score("voices=3", notes));
    assert_ne!(two, three);
}

#[test]
fn chunks_reset_and_stereo_match_whole_renders() {
    let s = score("voices=3", "q4 o4 l8 L [c e g >c< g!40 e!100]2 {c e g}4");
    let whole = render(&s);
    let options = RenderOptions::default();
    let mut synth = Synth::new(&s, 48_000, &options).unwrap();
    let mut out = Vec::new();
    let mut buffer = [0.0f32; 101];
    for round in 0..2 {
        loop {
            let n = synth.fill(&mut buffer[..(round * 37 + 7) % 101 + 1]);
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buffer[..n]);
        }
        assert_eq!(out, whole, "round {round}");
        out.clear();
        synth.reset();
    }
    // Stereo copies the mono wavetable to both channels.
    let stereo = s.render_stereo(48_000).unwrap();
    let mono = s.render(48_000).unwrap();
    for c in 0..2 {
        assert_eq!(
            stereo.channel_data(c).unwrap(),
            mono.channel_data(0).unwrap()
        );
    }
}

#[test]
fn switching_instruments_releases_the_other_path() {
    let s = Score::parse(&format!(
        "tempo 90\ninstrument tone {TABLE} voices=2\ninstrument plain wave=pulse duty=50 adsr=0,0,100,50\n\
         channel test = q8 o4 @tone a4 @plain a4 @tone a4 r1"
    ))
    .unwrap();
    let out = render(&s);
    // The pooled tail keeps sounding under the plain note that follows it.
    let plain_only = render(
        &Score::parse("tempo 90\ninstrument plain wave=pulse duty=50 adsr=0,0,100,50\nchannel test = @plain q8 o4 r4 a4 r4 r1")
            .unwrap(),
    );
    assert!((32_000..32_000 + 9_600).any(|i| (out[i] - plain_only[i]).abs() > 1e-3));
    assert!(out.iter().all(|s| s.is_finite()));
}

#[test]
fn chords_need_enough_voices() {
    assert!(
        Score::parse(&format!(
            "tempo 90\ninstrument t {TABLE} voices=3\nchannel a=@t {{c e g}}4"
        ))
        .is_ok()
    );
    let e = Score::parse(&format!(
        "tempo 90\ninstrument t {TABLE} voices=2\nchannel a=@t {{c e g}}4"
    ))
    .unwrap_err();
    assert_eq!((e.code, e.line), (ErrorCode::InvalidArgument, 3));
}

#[test]
fn only_wavetables_take_voices_and_only_voices() {
    for (instrument, why) in [
        ("wavetable=0f voices=2 unison=2", "other synthwave settings"),
        ("wavetable=0f voices=2 filter=lowpass", "a filter"),
        ("wavetable=0f voices=1 legato=on", "legato"),
        ("sample=00040f08 sample-rate=1000 voices=2", "PCM"),
        ("wave=noise noise-period=3 voices=2", "period noise"),
        (
            "macro-volume=15,7 voices=2",
            "a waveform source with macros",
        ),
    ] {
        let e = Score::parse(&format!(
            "tempo 90\ninstrument t {instrument}\nchannel a=@t c4"
        ))
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidArgument, "{why}: {e}");
        assert_eq!(e.line, 2, "{why}");
    }
    // The typed form: a wavetable source and synthwave settings with only `voices`.
    let table = Wavetable::new(vec![0, 15]).unwrap();
    let mut instrument = Instrument {
        chip: Some(ChipInstrument {
            source: ChipSource::Wavetable(table),
            ..Default::default()
        }),
        synthwave: Some(Synthwave {
            voices: 4,
            ..Default::default()
        }),
        ..Instrument::default()
    };
    assert!(instrument.validate().is_ok());
    instrument.synthwave = Some(Synthwave {
        voices: 4,
        unison: 2,
        ..Default::default()
    });
    assert!(instrument.validate().is_err());
}

#[test]
fn a_stolen_voice_attacks_from_silence() {
    // One voice, adjacent full gates: the second note takes the first note's voice.
    let s = Score::parse(
        "tempo 600\ninstrument t wavetable=fffffff0 adsr=50,0,100,500 voices=1\nchannel a=@t q8 c4 c4 r1",
    )
    .unwrap();
    let out = s.render(48_000).unwrap();
    let x = out.channel_data(0).unwrap();
    // At 600 BPM a quarter note is 4,800 frames; the 50 ms attack is 2,400.
    let peak = |range: std::ops::Range<usize>| range.map(|i| x[i].abs()).fold(0.0, f32::max);
    let full = peak(2_400..4_800);
    assert!(full > 0.1);
    // The second note starts quietly and rises through its attack again.
    assert!(
        peak(4_800..4_900) < full * 0.1,
        "{} vs {full}",
        peak(4_800..4_900)
    );
    assert!(peak(7_200..9_600) > full * 0.9);
}
