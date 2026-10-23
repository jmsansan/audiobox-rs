//! Tracker clocks, source response, PCM lifecycle and deterministic rendering.
#![cfg(feature = "synth")]
use audiobox::synth::*;

fn options() -> RenderOptions {
    RenderOptions {
        aliasing: Aliasing::Naive,
        gain: 1.0,
        ..Default::default()
    }
}
fn score(instrument: Instrument, pitch: Pitch) -> Score {
    let mut s = Score::new(60, 48).unwrap();
    let i = s.add_instrument("i", instrument).unwrap();
    s.channel("a")
        .unwrap()
        .instrument(i)
        .unwrap()
        .note(pitch, Length::QUARTER)
        .unwrap();
    s
}
fn instrument(chip: ChipInstrument) -> Instrument {
    Instrument {
        chip: Some(chip),
        ..Instrument::default()
    }
}
fn render(s: &Score, rate: u32) -> Vec<f32> {
    s.render_with(rate, &options())
        .unwrap()
        .channel_data(0)
        .unwrap()
        .to_vec()
}
fn lane(values: &[i16]) -> Macro {
    Macro::new(values.to_vec(), MacroStep::Frames { frames: 1, hz: 4 }).unwrap()
}
fn frequency(samples: &[f32], rate: u32) -> f64 {
    let crossings: Vec<_> = samples
        .windows(2)
        .enumerate()
        .filter_map(|(i, w)| (w[0] < 0.0 && w[1] >= 0.0).then_some(i))
        .collect();
    (crossings.len() - 1) as f64 * rate as f64 / (crossings.last().unwrap() - crossings[0]) as f64
}
fn hash(samples: &[f32]) -> u64 {
    samples
        .iter()
        .flat_map(|s| s.to_bits().to_le_bytes())
        .fold(0xcbf29ce484222325, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        })
}

#[test]
fn macro_clocks_hold_loop_and_do_not_accumulate_rounding() {
    let mut m = Macro::new(vec![10, 20, 30], MacroStep::Frames { frames: 2, hz: 60 }).unwrap();
    for (frame, expected) in [
        (0, 10),
        (367, 10),
        (368, 20),
        (734, 20),
        (735, 30),
        (u64::MAX, 30),
    ] {
        assert_eq!(m.value_at(frame, 11025, 137, 48).unwrap(), expected);
    }
    m.loop_start = Some(1);
    for (frame, expected) in [(1103, 20), (1470, 30), (1838, 20), (2_205_000, 30)] {
        assert_eq!(m.value_at(frame, 11025, 137, 48).unwrap(), expected);
    }
    m.step = MacroStep::Ticks(7);
    // 7 ticks at 137 BPM and 48 TPQ are 704.151... samples at 11025 Hz.
    for (frame, expected) in [(704, 10), (705, 20), (1408, 20), (1409, 30), (2113, 20)] {
        assert_eq!(m.value_at(frame, 11025, 137, 48).unwrap(), expected);
    }
    assert!(m.value_at(0, 0, 137, 48).is_err());
    assert!(m.value_at(0, 11025, 0, 48).is_err());
    m.loop_start = Some(3);
    assert!(m.validate().is_err());
}

#[test]
fn wavetable_values_period_and_harmonics_match_a_square_wave() {
    let mut values = vec![15; 16];
    values.extend([0; 16]);
    let s = score(
        instrument(ChipInstrument {
            source: ChipSource::Wavetable(Wavetable::new(values).unwrap()),
            ..Default::default()
        }),
        Pitch::A4,
    );
    let data = render(&s, 44_000);
    assert!(data.iter().all(|s| s.abs() == 1.0));
    assert!((frequency(&data, 44_000) - 440.0).abs() < 0.1);
    let magnitude = |harmonic: usize| {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &v) in data[..4400].iter().enumerate() {
            let angle = std::f64::consts::TAU * (i * harmonic) as f64 / 100.0;
            re += v as f64 * angle.cos();
            im += v as f64 * angle.sin();
        }
        re.hypot(im) / 2200.0
    };
    assert!((magnitude(1) - 4.0 / std::f64::consts::PI).abs() < 0.002);
    assert!(magnitude(2) < 0.001);
    assert!((magnitude(3) / magnitude(1) - 1.0 / 3.0).abs() < 0.002);
}

#[test]
fn volume_macros_keep_source_phase_running_while_muted() {
    for source in [
        ChipSource::Waveform,
        ChipSource::Wavetable(Wavetable::new(vec![0, 3, 7, 15]).unwrap()),
        ChipSource::Sample(ChipSample {
            looping: true,
            ..ChipSample::new(vec![0, 4, 15], 4, 1337).unwrap()
        }),
    ] {
        let mut chip = ChipInstrument {
            source,
            ..Default::default()
        };
        let baseline = render(&score(instrument(chip.clone()), Pitch::A4), 8000);
        chip.volume = Some(lane(&[0, 15, 7, 15]));
        let data = render(&score(instrument(chip), Pitch::A4), 8000);
        assert!(data[..2000].iter().all(|&s| s == 0.0));
        assert_eq!(&data[2000..4000], &baseline[2000..4000]);
        assert_eq!(&data[6000..], &baseline[6000..]);
        for (a, b) in data[4000..6000].iter().zip(&baseline[4000..6000]) {
            assert!((*a as f64 - *b as f64 * 7.0 / 15.0).abs() < 6e-8);
        }
    }
}

#[test]
fn duty_arpeggio_and_fine_pitch_lanes_change_the_sound_at_boundaries() {
    let chip = ChipInstrument {
        duty: Some(lane(&[0, 1, 2, 3])),
        arpeggio: Some(lane(&[0, 12, 0, 12])),
        pitch: Some(lane(&[0, 0, 1200, -1200])),
        ..Default::default()
    };
    let data = render(&score(instrument(chip), Pitch::A4), 48_000);
    for (n, expected) in [440.0, 880.0, 880.0, 440.0].into_iter().enumerate() {
        let part = &data[n * 12000..(n + 1) * 12000];
        assert!((frequency(part, 48000) - expected).abs() < 0.2);
        let duty = part.iter().filter(|&&s| s > 0.0).count() as f64 / part.len() as f64;
        assert!((duty - [0.125, 0.25, 0.5, 0.75][n]).abs() < 0.005);
    }
}

#[test]
fn ntsc_timer_models_and_combined_four_bit_gain_are_quantized() {
    for (table, timer, divider) in [
        (PitchTable::NtscPulse, 253, 16.0),
        (PitchTable::NtscTriangle, 126, 32.0),
    ] {
        assert_eq!(table.timer(440.0).unwrap(), timer);
        assert_eq!(
            table.frequency(timer).unwrap(),
            1_789_773.0 / (divider * (timer + 1) as f64)
        );
        assert!(table.timer(f64::NAN).is_err());
        assert!(table.frequency(2048).is_err());
        let wave = if table == PitchTable::NtscPulse {
            Waveform::default()
        } else {
            Waveform::Triangle
        };
        let i = Instrument {
            chip: Some(ChipInstrument {
                pitch_table: Some(table),
                ..Default::default()
            }),
            ..Instrument::new(wave)
        };
        let actual = frequency(&render(&score(i, Pitch::A4), 48000), 48000);
        assert!((actual - table.frequency(timer).unwrap()).abs() < 0.03);
    }
    assert_eq!(PitchTable::NtscPulse.timer(1.0).unwrap(), 2047);
    assert_eq!(PitchTable::NtscPulse.timer(1e9).unwrap(), 8);
    assert_eq!(PitchTable::NtscTriangle.timer(1e9).unwrap(), 0);
    let i = Instrument {
        envelope: Envelope {
            attack_ms: 30,
            ..Default::default()
        },
        volume: 9,
        chip: Some(ChipInstrument {
            source: ChipSource::Wavetable(Wavetable::new(vec![15, 15]).unwrap()),
            volume_4bit: true,
            volume: Some(lane(&[11])),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut s = Score::new(60, 48).unwrap();
    let i = s.add_instrument("q", i).unwrap();
    s.channel("q")
        .unwrap()
        .instrument(i)
        .unwrap()
        .volume(13)
        .unwrap()
        .note_with_velocity(Pitch::C4, Length::QUARTER, 77)
        .unwrap();
    let data = render(&s, 1000);
    for (n, &v) in data[..100].iter().enumerate() {
        let combined =
            (n as f64 / 30.0).min(1.0) * 9.0 / 15.0 * 13.0 / 15.0 * 77.0 / 127.0 * 11.0 / 15.0;
        assert_eq!(v, ((combined * 15.0).round() / 15.0) as f32);
    }
}

#[test]
fn period_noise_matches_register_shifts_and_ignores_note_pitch() {
    assert_eq!(
        NTSC_NOISE_PERIODS,
        [
            4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068
        ]
    );
    assert!(noise_clock(16).is_err());
    for mode in [NoiseMode::Long, NoiseMode::Short] {
        for period in 0..16 {
            assert_eq!(
                noise_clock(period).unwrap(),
                1_789_773.0 / NTSC_NOISE_PERIODS[period as usize] as f64
            );
            let i = Instrument {
                chip: Some(ChipInstrument {
                    source: ChipSource::Noise { period },
                    ..Default::default()
                }),
                ..Instrument::new(Waveform::Noise(mode))
            };
            let data = render(&score(i.clone(), Pitch::C4), 1000);
            assert_eq!(data, render(&score(i, Pitch::A4), 1000));
            let (mut state, mut shifted) = (1u16, 0u64);
            for (n, &value) in data.iter().enumerate() {
                let target = (n as f64 * noise_clock(period).unwrap() / 1000.0).floor() as u64;
                while shifted < target {
                    let tap = if mode == NoiseMode::Long { 1 } else { 6 };
                    let feedback = (state ^ (state >> tap)) & 1;
                    state = (state >> 1) | (feedback << 14);
                    shifted += 1;
                }
                assert_eq!(
                    value,
                    if state & 1 == 0 { 1.0 } else { -1.0 },
                    "{mode:?}/{period}/{n}"
                );
            }
        }
    }
}

#[test]
fn pcm_uses_relative_pitch_even_above_output_nyquist_and_stops_at_its_end() {
    let i = instrument(ChipInstrument {
        source: ChipSource::Sample(ChipSample::new(vec![0, 15, 0, 15], 4, 125).unwrap()),
        ..Default::default()
    });
    let data = render(&score(i, Pitch::new(72).unwrap()), 1000);
    for (n, &v) in data.iter().enumerate() {
        let expected = if n < 16 {
            if (n / 4) % 2 == 0 { -1.0 } else { 1.0 }
        } else {
            0.0
        };
        assert_eq!(v, expected, "PCM frame {n}");
    }
    for bits in 2..=8 {
        let max = ((1u16 << bits) - 1) as u8;
        let sample = ChipSample {
            looping: true,
            ..ChipSample::new(vec![0, max], bits, 1000).unwrap()
        };
        let data = render(
            &score(
                instrument(ChipInstrument {
                    source: ChipSource::Sample(sample),
                    ..Default::default()
                }),
                Pitch::C4,
            ),
            1000,
        );
        for (n, &v) in data.iter().enumerate() {
            assert_eq!(v, if n % 2 == 0 { -1.0 } else { 1.0 });
        }
    }
}

#[test]
fn ended_pcm_clears_envelope_and_retrigger_restarts_data_and_macros() {
    let text = "tempo 120\ninstrument p sample=0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f sample-rate=1000 adsr=10,0,100,0 macro-volume=15,7 macro-volume-step=frames,1,100\nchannel a=@p c8 c8";
    let data = render(&Score::parse(text).unwrap(), 1000);
    assert_eq!(&data[..250], &data[250..500]);
    assert_eq!(data[250], 0.0);
    assert_eq!(data[251], 0.1);
    assert!(data[16..250].iter().all(|&v| v == 0.0));
    let gated=Score::parse("tempo 60\ninstrument p sample=0f sample-loop=on sample-rate=1000 adsr=0,0,100,20\nchannel a=@p q4 c4").unwrap();
    let data = render(&gated, 1000);
    assert_eq!(data[500], 1.0);
    assert!((data[510] - 0.5).abs() < 1e-7);
    assert!(data[520..].iter().all(|&v| v == 0.0));
}

#[test]
fn ties_continue_chip_clocks_and_extreme_pcm_slides_remain_bounded() {
    for source in [
        "wavetable=035af",
        "sample=00030f sample-rate=1000 sample-loop=on",
    ] {
        let definition = format!(
            "tempo 60\ninstrument w {source} macro-volume=15,7,3 macro-volume-loop=0\nchannel a=@w "
        );
        let tied = Score::parse(&format!("{definition}a8&a8")).unwrap();
        let whole = Score::parse(&format!("{definition}a4")).unwrap();
        assert_eq!(render(&tied, 8000), render(&whole, 8000));
    }
    for looping in [false, true] {
        let i = Instrument {
            slide: 1_200_000,
            chip: Some(ChipInstrument {
                source: ChipSource::Sample(ChipSample {
                    looping,
                    root: Pitch::new(0).unwrap(),
                    ..ChipSample::new(vec![0, 15], 4, 32000).unwrap()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let data = render(&score(i, Pitch::new(127).unwrap()), 1000);
        assert!(data.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
        assert_eq!(data.len(), 1000);
    }
}

#[test]
fn loop_carry_restarts_pcm_and_macro_age_even_after_a_one_shot_ended() {
    for looping in ["on", "off"] {
        let text = format!(
            "tempo 60\ninstrument p sample=000f0703 sample-rate=1000 sample-loop={looping} macro-volume=15,7,3 macro-volume-step=frames,1,4\nchannel a=@p c2\nchannel b=r4 L r4"
        );
        let score = Score::parse(&text).unwrap();
        let out = score
            .render_with(
                1000,
                &RenderOptions {
                    loops: Some(1),
                    ..options()
                },
            )
            .unwrap();
        let data = out.channel_data(0).unwrap();
        assert_eq!(data.len(), 3000);
        assert_eq!(&data[..1000], &data[2000..3000]);
        assert!(data[..4].iter().any(|&v| v != 0.0));
    }
}

#[test]
fn parser_matches_typed_controls_and_is_order_independent() {
    let a=Score::parse("instrument i sample=00030f sample-bits=4 sample-rate=1000 sample-root=69 sample-loop=on macro-volume=15,8,0 macro-volume-step=ticks,6 macro-volume-loop=1 quantize=volume\nchannel a=@i a4").unwrap();
    let mut sample = ChipSample::new(vec![0, 3, 15], 4, 1000).unwrap();
    sample.root = Pitch::A4;
    sample.looping = true;
    let mut volume = Macro::new(vec![15, 8, 0], MacroStep::Ticks(6)).unwrap();
    volume.loop_start = Some(1);
    let i = instrument(ChipInstrument {
        source: ChipSource::Sample(sample),
        volume: Some(volume),
        volume_4bit: true,
        ..Default::default()
    });
    assert_eq!(a.instruments()[0].1, i);
    let b=Score::parse("instrument i macro-volume-loop=1 macro-volume-step=ticks,6 sample-loop=on sample-root=69 sample-rate=1000 sample-bits=4 quantize=volume macro-volume=15,8,0 sample=00030f\nchannel a=@i a4").unwrap();
    assert_eq!(a, b);
    assert_eq!(render(&a, 8000), render(&b, 8000));
}

#[test]
fn malformed_sources_clocks_lanes_and_conflicting_controls_are_rejected() {
    for keys in [
        "wavetable=0",
        "wavetable=0z",
        "wavetable=éé",
        "sample=0",
        "sample=é",
        "sample=10 sample-bits=4",
        "sample=00 sample-bits=1",
        "sample=00 sample-rate=99",
        "sample=00 sample-root=128",
        "sample=00 sample-loop=yes",
        "sample-rate=1000",
        "wavetable=0f sample=00",
        "wavetable=0f wave=pulse",
        "sample=00 noise=long",
        "noise-period=0",
        "wave=noise noise-period=16",
        "wave=noise noise-period=1 slide=2",
        "wave=noise noise-period=1 macro-pitch=1",
        "wave=triangle macro-duty=0",
        "macro-volume=16",
        "macro-volume=-1",
        "macro-duty=4",
        "macro-arpeggio=49",
        "macro-pitch=1201",
        "macro-volume=1 macro-volume-step=frames,0,60",
        "macro-volume=1 macro-volume-step=ticks,0",
        "macro-volume=1 macro-volume-loop=1",
        "macro-volume-step=ticks,2",
        "macro-unknown=1",
        "quantize=unknown",
        "wave=saw quantize=ntsc-pulse",
        "arpeggio=0,4 macro-arpeggio=0,7",
        "duty=25,50 macro-duty=0,1",
        "drum=kick macro-volume=1",
    ] {
        let text = format!("instrument i {keys}");
        let e = Score::parse(&text).unwrap_err();
        assert!(e.line > 0 && e.column > 0, "{keys}: {e}");
    }
    assert!(Wavetable::new(vec![16, 0]).is_err());
    assert!(ChipSample::new(vec![], 4, 1000).is_err());
    assert!(ChipSample::new(vec![0; MAX_SAMPLE_VALUES + 1], 4, 1000).is_err());
    assert!(Macro::new(vec![0; MAX_MACRO_VALUES + 1], MacroStep::default()).is_err());
    for keys in [
        format!("wavetable={}", "f".repeat(257)),
        format!("sample={}", "00".repeat(MAX_SAMPLE_VALUES + 1)),
        format!("macro-volume={}", vec!["1"; 65].join(",")),
    ] {
        assert!(Score::parse(&format!("instrument i {keys}")).is_err());
    }
}

const REFERENCE:&str="tempo 137
instrument w wavetable=0123456789abcdefedcba98765432100 adsr=3,20,70,20 macro-volume=15,12,8,4 macro-volume-loop=1 macro-arpeggio=0,7,12 macro-arpeggio-step=ticks,6 macro-arpeggio-loop=0
instrument p wave=pulse quantize=ntsc-pulse macro-duty=0,1,2,3 macro-duty-loop=0 macro-pitch=0,-10,10 macro-pitch-loop=0
instrument n wave=noise noise=short noise-period=6 macro-volume=15,8,2,0
instrument s sample=00050a0f0c070300 sample-bits=4 sample-rate=3000 sample-root=60 sample-loop=on macro-volume=15,12,8 quantize=volume
channel a=@w o5 l8 L [c e g >c<]2
channel b=@p o3 l4 L [c g]2
channel c=@n l16 L [c r]8
channel d=@s o4 l8 L [c!80 r g!64 r]2";

#[test]
fn tracker_render_is_pinned_chunk_invariant_and_resettable() {
    let score = Score::parse(REFERENCE).unwrap();
    for aliasing in [Aliasing::Naive, Aliasing::BandLimited] {
        let opts = RenderOptions {
            aliasing,
            loops: Some(1),
            gain: 0.2,
        };
        let expected = score.render_with(11025, &opts).unwrap();
        let data = expected.channel_data(0).unwrap();
        assert_eq!(
            hash(data),
            match aliasing {
                Aliasing::Naive => 0x4230de9740dccbf8,
                Aliasing::BandLimited => 0x2a202e513de49085,
            }
        );
        for size in [1, 137, 4096] {
            let mut synth = Synth::new(&score, 11025, &opts).unwrap();
            let mut out = Vec::new();
            let mut buffer = vec![0.0; size];
            loop {
                let n = synth.fill(&mut buffer);
                out.extend_from_slice(&buffer[..n]);
                if n == 0 {
                    break;
                }
            }
            assert_eq!(&out, data);
            synth.reset();
            let mut reset = vec![0.0; data.len()];
            assert_eq!(synth.fill(&mut reset), data.len());
            assert_eq!(&reset, data);
        }
    }
}
