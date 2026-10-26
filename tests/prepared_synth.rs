#![cfg(feature = "synth")]
use audiobox::synth::*;
use audiobox::{ErrorCode, Result};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Count;
thread_local! {
    static ENABLED: Cell<bool> = const {Cell::new(false)};
    static ALLOCS: Cell<usize> = const {Cell::new(0)};
}
fn count() {
    if ENABLED.try_with(Cell::get).unwrap_or(false) {
        ALLOCS.with(|a| a.set(a.get() + 1));
    }
}
unsafe impl GlobalAlloc for Count {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static GLOBAL: Count = Count;
fn no_alloc<T>(f: impl FnOnce() -> T) -> T {
    ALLOCS.with(|a| a.set(0));
    ENABLED.with(|a| a.set(true));
    let result = f();
    ENABLED.with(|a| a.set(false));
    assert_eq!(ALLOCS.with(Cell::get), 0, "callback allocated");
    result
}
fn map(rate: u32) -> TempoMap {
    TempoMap::new(
        rate,
        48,
        &[
            TempoChange {
                tick: 0,
                bpm: 123.456,
            },
            TempoChange {
                tick: 256 * 48,
                bpm: 98.765,
            },
            TempoChange {
                tick: 512 * 48,
                bpm: 150.125,
            },
        ],
    )
    .unwrap()
}
// Independent common-denominator arithmetic for this fixture. Does not call
// engine time conversion or sum independently rounded segment/note durations.
fn reference(tick: u64, rate: u32) -> u64 {
    let d = 123456u128 * 98765 * 150125 * 48;
    let n = tick.min(256 * 48) as u128 * rate as u128 * 60000 * (d / (123456 * 48))
        + tick.saturating_sub(256 * 48).min(256 * 48) as u128
            * rate as u128
            * 60000
            * (d / (98765 * 48))
        + tick.saturating_sub(512 * 48) as u128 * rate as u128 * 60000 * (d / (150125 * 48));
    ((n + d / 2) / d) as u64
}
fn options() -> RenderOptions {
    RenderOptions {
        gain: 1.0,
        aliasing: Aliasing::Naive,
        ..Default::default()
    }
}
fn render(mut synth: Synth, sizes: &[usize]) -> Vec<f32> {
    let channels = synth.channels();
    let mut out = Vec::new();
    let mut chunk = vec![0.0; sizes.iter().max().unwrap() * channels];
    let mut i = 0;
    while !synth.is_finished() {
        let n = no_alloc(|| synth.fill(&mut chunk[..sizes[i % sizes.len()] * channels]));
        out.extend_from_slice(&chunk[..n * channels]);
        i += 1;
    }
    out
}
fn mix_hash(mut h: u64, bytes: impl IntoIterator<Item = u8>) -> u64 {
    for b in bytes {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

#[test]
fn fifteen_parts_have_independent_on_off_frames_over_five_minutes() {
    let rate = 1000;
    let map = map(rate);
    let end = reference(896 * 48, rate);
    assert!(end > 300 * rate as u64);
    let mut fingerprint = 0xcbf29ce484222325;
    let mut event_count = 0;
    for part in 0..15 {
        let mut notes = Vec::new();
        // Seven repeated 128-quarter sections, authored at absolute positions.
        for section in 0..7 {
            for q in 0..128 {
                let tick = (section * 128 + q) * 48 + (part % 3) * 2;
                let on = reference(tick, rate);
                let off = reference(tick + 12, rate);
                assert_eq!(map.frame_at(tick).unwrap(), on);
                assert_eq!(map.frame_at(tick + 12).unwrap(), off);
                fingerprint = mix_hash(
                    fingerprint,
                    on.to_le_bytes().into_iter().chain(off.to_le_bytes()),
                );
                notes.push(FrameNote::new(
                    on,
                    off,
                    Pitch::new(48 + part as u8).unwrap(),
                    0,
                ));
            }
        }
        event_count += notes.len();
        let score = FrameScore {
            instruments: vec![Instrument::default()],
            tracks: vec![notes],
            end_frame: end,
        };
        let mut synth = Synth::from_frames(&score, &map, &options()).unwrap();
        let mut samples = vec![0.0; synth.max_frames().unwrap() as usize];
        let written = no_alloc(|| synth.fill(&mut samples));
        assert_eq!(written, end as usize);
        let mut index = 0;
        for (f, &sample) in samples[..written].iter().enumerate() {
            while index < score.tracks[0].len() && score.tracks[0][index].off <= f as u64 {
                index += 1;
            }
            let held = index < score.tracks[0].len() && score.tracks[0][index].on <= f as u64;
            assert_eq!(
                sample.abs(),
                if held { 1.0 } else { 0.0 },
                "part {part}, frame {f}"
            );
            fingerprint = mix_hash(fingerprint, sample.to_bits().to_le_bytes());
        }
        no_alloc(|| synth.reset());
        let mut first = [0.0; 17];
        assert_eq!(no_alloc(|| synth.fill(&mut first)), 17);
        assert_eq!(&first, &samples[..17]);
    }
    assert_eq!(event_count, 13440);
    assert_eq!(
        fingerprint, 12380820460232774821,
        "long arrangement timing and PCM"
    );
}

#[test]
fn exact_map_rounding_changes_bounds_and_transport_agree() {
    for rate in [1000, 8000, 32000, 44100, 48000, 96000, 192000, 768000] {
        let m = map(rate);
        let mut previous = 0;
        for tick in [
            0,
            1,
            23,
            24,
            47,
            48,
            256 * 48 - 1,
            256 * 48,
            256 * 48 + 1,
            512 * 48,
            896 * 48,
        ] {
            let frame = m.frame_at(tick).unwrap();
            assert_eq!(frame, reference(tick, rate));
            assert!(frame >= previous);
            previous = frame;
        }
        #[cfg(feature = "music")]
        {
            use audiobox::music::{Quantize, TimeSignature, Transport};
            let fixed = TempoMap::new(
                rate,
                48,
                &[TempoChange {
                    tick: 0,
                    bpm: 123.456,
                }],
            )
            .unwrap();
            let transport = Transport::new(rate, 123.456, TimeSignature::default(), 0).unwrap();
            for q in 1..500 {
                let f = fixed.frame_at(q * 48).unwrap();
                assert_eq!(transport.boundary_after(f - 1, Quantize::Beat).unwrap(), f);
            }
        }
    }
    let halves = TempoMap::new(
        1000,
        16,
        &[TempoChange {
            tick: 0,
            bpm: 120.0,
        }],
    )
    .unwrap();
    assert_eq!(halves.frame_at(1).unwrap(), 31);
    assert_eq!(halves.frame_at(2).unwrap(), 63); // 62.5 -> 63, once.
    let precision = TempoMap::new(
        48000,
        48,
        &[TempoChange {
            tick: 0,
            bpm: 123.4564,
        }],
    )
    .unwrap();
    assert_eq!(precision.bpm_at(0), 123.456);
    assert_eq!(map(1000).bpm_at(256 * 48), 98.765);
}

fn short_map(rate: u32) -> TempoMap {
    TempoMap::new(
        rate,
        48,
        &[
            TempoChange {
                tick: 0,
                bpm: 123.456,
            },
            TempoChange {
                tick: 80,
                bpm: 98.765,
            },
        ],
    )
    .unwrap()
}

#[test]
fn held_lfo_phase_and_tick_macros_follow_the_scheduled_tempo_clock() {
    let rate = 1000;
    let map = short_map(rate);
    let change = map.frame_at(80).unwrap();
    let end = map.frame_at(240).unwrap();
    let beat = |frame: u64| {
        frame.min(change) as u128 * 123456 + frame.saturating_sub(change) as u128 * 98765
    };
    let denominator = rate as u128 * 60000;
    let wide = Instrument {
        synthwave: Some(Synthwave {
            voices: 1,
            amplitude_lfo: Some(Lfo {
                rate: LfoRate::Beats {
                    numerator: 1,
                    denominator: 1,
                },
                depth: 100,
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let score = FrameScore {
        instruments: vec![wide],
        tracks: vec![vec![FrameNote::new(0, end, Pitch::C4, 0)]],
        end_frame: end,
    };
    let audio = render(
        Synth::from_frames(&score, &map, &options()).unwrap(),
        &[1, 7, 333],
    );
    let mut unmodulated = score.clone();
    unmodulated.instruments[0]
        .synthwave
        .as_mut()
        .unwrap()
        .amplitude_lfo = None;
    let control = render(
        Synth::from_frames(&unmodulated, &map, &options()).unwrap(),
        &[4096],
    );
    for f in 0..end {
        let turns = (beat(f) % denominator) as f64 / denominator as f64;
        let expected = 0.5 * (1.0 - (std::f64::consts::TAU * turns).sin());
        assert!(
            (audio[f as usize] as f64 - control[f as usize] as f64 * expected).abs() < 0.000001,
            "LFO frame {f}"
        );
    }
    let instrument = Instrument {
        chip: Some(ChipInstrument {
            volume: Some(Macro {
                values: vec![15, 0, 15, 0],
                step: MacroStep::Ticks(48),
                loop_start: Some(0),
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let start = 123;
    let score = FrameScore {
        instruments: vec![instrument],
        tracks: vec![vec![FrameNote::new(start, end, Pitch::C4, 0)]],
        end_frame: end,
    };
    let audio = render(
        Synth::from_frames(&score, &map, &options()).unwrap(),
        &[7, 1, 128, 19],
    );
    for f in 0..end {
        let value = if f < start {
            0.0
        } else if ((beat(f) - beat(start)) / denominator) % 2 == 0 {
            1.0
        } else {
            0.0
        };
        assert_eq!(audio[f as usize].abs(), value, "macro frame {f}");
    }
}

#[test]
fn ties_simultaneous_notes_and_release_have_explicit_ordering() {
    let map = short_map(1000);
    let middle = map.frame_at(80).unwrap();
    let end = map.frame_at(160).unwrap();
    let instrument = Instrument {
        envelope: Envelope {
            attack_ms: 1000,
            release_ms: 30,
            ..Default::default()
        },
        synthwave: Some(Synthwave {
            voices: 1,
            legato: true,
            filter: Some(VoiceFilter::default()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut score = FrameScore {
        instruments: vec![instrument],
        tracks: vec![vec![FrameNote::new(0, end, Pitch::C4, 0)]],
        end_frame: end,
    };
    let tied = render(
        Synth::from_frames(&score, &map, &options()).unwrap(),
        &[4096],
    );
    score.tracks[0] = vec![
        FrameNote::new(0, middle, Pitch::C4, 0),
        FrameNote::new(middle, end, Pitch::C4, 0),
    ];
    let legato = render(
        Synth::from_frames(&score, &map, &options()).unwrap(),
        &[1, 7, 65],
    );
    assert_eq!(
        tied, legato,
        "adjacent same-note legato preserves every state through a tempo boundary"
    );
    assert_eq!(tied.len(), end as usize + 30);
    let map = TempoMap::new(
        1000,
        48,
        &[TempoChange {
            tick: 0,
            bpm: 120.0,
        }],
    )
    .unwrap();
    let mut last = FrameNote::new(0, 10, Pitch::C4, 0);
    last.volume = 12;
    let score = FrameScore {
        instruments: vec![Instrument::default()],
        tracks: vec![vec![FrameNote::new(0, 10, Pitch::C4, 0), last]],
        end_frame: 20,
    };
    let audio = render(Synth::from_frames(&score, &map, &options()).unwrap(), &[64]);
    assert_eq!(audio[0], 0.8);
    assert!(audio[..10].iter().all(|x| x.abs() == 0.8));
    assert!(audio[10..].iter().all(|&x| x == 0.0));
    assert_eq!(audio.len(), 20);
}

#[test]
fn prepared_instruments_whole_chunks_reset_and_pcm_are_pinned() {
    let parsed=Score::parse("instrument pad wave=saw voices=3 unison=3 spread=80 adsr=5,10,80,40 filter=lowpass cutoff=1000 lfo-cutoff=beats,2,1,1200\ninstrument chip wavetable=0123456789abcdef macro-volume=15,8,0 macro-volume-step=ticks,12 macro-volume-loop=0\ninstrument drum drum=snare\nchannel a=@pad c4").unwrap();
    let map = short_map(8000);
    let f = |t| map.frame_at(t).unwrap();
    let score = FrameScore {
        instruments: parsed
            .instruments()
            .iter()
            .map(|(_, i)| i.clone())
            .collect(),
        tracks: vec![
            vec![
                FrameNote::new(0, f(160), Pitch::C4, 0),
                FrameNote::new(f(24), f(120), Pitch::new(64).unwrap(), 0),
                FrameNote::new(f(80), f(192), Pitch::new(67).unwrap(), 0),
            ],
            vec![FrameNote::new(f(12), f(192), Pitch::new(48).unwrap(), 1)],
            vec![
                FrameNote::new(f(48), f(49), Pitch::C4, 2),
                FrameNote::new(f(96), f(97), Pitch::C4, 2),
            ],
        ],
        end_frame: f(192),
    };
    let mut fingerprint = 0xcbf29ce484222325;
    for stereo in [false, true] {
        let synth = if stereo {
            Synth::from_frames_stereo(&score, &map, &options())
        } else {
            Synth::from_frames(&score, &map, &options())
        }
        .unwrap();
        let expected = render(synth.clone(), &[100000]);
        assert!(expected.iter().all(|s| s.is_finite()));
        let chunked = render(synth.clone(), &[1, 7, 64, 333, 1000, 4096]);
        assert_eq!(chunked, expected);
        let mut reset = synth;
        no_alloc(|| reset.fill(&mut [0.0; 777]));
        no_alloc(|| reset.reset());
        assert_eq!(render(reset, &[31, 127]), expected);
        fingerprint = mix_hash(
            fingerprint,
            expected.iter().flat_map(|x| x.to_bits().to_le_bytes()),
        );
    }
    assert_eq!(fingerprint, 15360897397046987816, "prepared instrument PCM");
}

#[test]
fn invalid_and_unrepresentable_preparation_is_rejected() {
    let code = |r: Result<TempoMap>| r.unwrap_err().code;
    for changes in [
        vec![],
        vec![TempoChange {
            tick: 1,
            bpm: 120.0,
        }],
        vec![
            TempoChange {
                tick: 0,
                bpm: 120.0,
            },
            TempoChange { tick: 0, bpm: 90.0 },
        ],
    ] {
        assert_eq!(
            code(TempoMap::new(1000, 48, &changes)),
            ErrorCode::InvalidArgument
        );
    }
    for bpm in [f64::NAN, f64::INFINITY, 0.999, 1000.001] {
        assert_eq!(
            code(TempoMap::new(1000, 48, &[TempoChange { tick: 0, bpm }])),
            ErrorCode::InvalidArgument
        );
    }
    for tpq in [0, 961] {
        assert_eq!(
            code(TempoMap::new(
                1000,
                tpq,
                &[TempoChange {
                    tick: 0,
                    bpm: 120.0
                }]
            )),
            ErrorCode::InvalidArgument
        );
    }
    let excess: Vec<_> = (0..=MAX_TEMPO_CHANGES)
        .map(|n| TempoChange {
            tick: n as u64,
            bpm: 120.0,
        })
        .collect();
    assert_eq!(
        code(TempoMap::new(1000, 48, &excess)),
        ErrorCode::LimitExceeded
    );
    let relatively_prime = [
        100003, 100019, 100043, 100049, 100057, 100069, 100103, 100109, 100129, 100151,
    ];
    let overflow: Vec<_> = relatively_prime
        .iter()
        .enumerate()
        .map(|(i, &n)| TempoChange {
            tick: i as u64 * 48,
            bpm: n as f64 / 1000.0,
        })
        .collect();
    assert_eq!(
        code(TempoMap::new(1000, 48, &overflow)),
        ErrorCode::LimitExceeded
    );
    let map = TempoMap::new(1000, 1, &[TempoChange { tick: 0, bpm: 1.0 }]).unwrap();
    assert_eq!(
        map.frame_at(u64::MAX).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    let valid = FrameScore {
        instruments: vec![Instrument::default()],
        tracks: vec![vec![FrameNote::new(0, 10, Pitch::C4, 0)]],
        end_frame: 20,
    };
    for note in [
        FrameNote::new(10, 10, Pitch::C4, 0),
        FrameNote::new(0, 21, Pitch::C4, 0),
        FrameNote::new(0, 10, Pitch::C4, 1),
        FrameNote {
            volume: 16,
            ..valid.tracks[0][0]
        },
        FrameNote {
            velocity: 128,
            ..valid.tracks[0][0]
        },
    ] {
        let mut score = valid.clone();
        score.tracks[0] = vec![note];
        assert_eq!(
            Synth::from_frames(&score, &map, &options())
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
    }
    for loops in [None, Some(1)] {
        assert_eq!(
            Synth::from_frames(&valid, &map, &RenderOptions { loops, ..options() })
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
    }
    let mut score = valid.clone();
    score.tracks = vec![vec![]; 9];
    assert_eq!(
        Synth::from_frames(&score, &map, &options())
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    score = valid.clone();
    score.end_frame = u64::MAX;
    assert_eq!(
        Synth::from_frames(&score, &map, &options())
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    score = valid.clone();
    score.tracks[0] = vec![valid.tracks[0][0]; 200001];
    assert_eq!(
        Synth::from_frames(&score, &map, &options())
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    score = valid;
    score.tracks[0] = vec![
        FrameNote::new(5, 10, Pitch::C4, 0),
        FrameNote::new(0, 10, Pitch::C4, 0),
    ];
    assert_eq!(
        Synth::from_frames(&score, &map, &options())
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
}

#[cfg(feature = "mixer")]
#[test]
fn fifteen_prepared_streams_start_on_one_mixer_frame_without_allocating() {
    use audiobox::mixer::{Mixer, MixerConfig, Play};
    let map = short_map(1000);
    let score = FrameScore {
        instruments: vec![Instrument::default()],
        tracks: vec![vec![
            FrameNote::new(100, 200, Pitch::C4, 0),
            FrameNote::new(400, 500, Pitch::C4, 0),
            FrameNote::new(799, 1100, Pitch::C4, 0),
        ]],
        end_frame: map.frame_at(160).unwrap(),
    };
    let control = render(
        Synth::from_frames(&score, &map, &options()).unwrap(),
        &[4096],
    );
    let start = 97;
    for size in [1, 7, 64, 331, 4096] {
        let (mut mixer, mut handle) = Mixer::new(MixerConfig {
            sample_rate: 1000,
            channels: 1,
            max_voices: 15,
            queue: 64,
            ..Default::default()
        })
        .unwrap();
        for _ in 0..15 {
            handle
                .at(start)
                .play(
                    Synth::from_frames(&score, &map, &options()).unwrap(),
                    &Play {
                        gain: 1.0 / 16.0,
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let mut out = vec![0.0; start as usize + control.len() + 50];
        for chunk in out.chunks_mut(size) {
            no_alloc(|| mixer.render(chunk));
        }
        assert!(out[..start as usize].iter().all(|&s| s == 0.0));
        for (f, &sample) in control.iter().enumerate() {
            assert_eq!(
                out[start as usize + f],
                sample * 15.0 / 16.0,
                "block {size}, frame {f}"
            );
        }
        assert!(
            out[start as usize + control.len()..]
                .iter()
                .all(|&s| s == 0.0)
        );
    }
}

#[test]
fn collapsed_tempo_boundaries_and_empty_bodies_are_well_defined() {
    let map = TempoMap::new(
        1000,
        960,
        &[
            TempoChange {
                tick: 0,
                bpm: 1000.0,
            },
            TempoChange {
                tick: 1,
                bpm: 500.0,
            },
            TempoChange {
                tick: 2,
                bpm: 250.0,
            },
        ],
    )
    .unwrap();
    assert_eq!(map.frame_at(2).unwrap(), 0);
    let empty = Synth::from_frames(&FrameScore::default(), &map, &options()).unwrap();
    assert!(render(empty, &[1, 7]).is_empty());
    let fixed = TempoMap::new(
        1000,
        48,
        &[TempoChange {
            tick: 0,
            bpm: 120.0,
        }],
    )
    .unwrap();
    let invalid_gain = RenderOptions {
        gain: f32::NAN,
        ..options()
    };
    assert_eq!(
        Synth::from_frames(&FrameScore::default(), &fixed, &invalid_gain)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    let short = FrameScore {
        end_frame: 1,
        ..Default::default()
    };
    assert_eq!(
        Synth::from_frames(&short, &short_map(1000), &options())
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
}
