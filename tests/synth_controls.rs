#![cfg(feature = "synth")]
use audiobox::ErrorCode;
use audiobox::synth::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Count;
thread_local! {
    static ENABLED:Cell<bool>=const {Cell::new(false)};
    static COUNT:Cell<usize>=const {Cell::new(0)};
}
fn count() {
    if ENABLED.try_with(Cell::get).unwrap_or(false) {
        COUNT.with(|c| c.set(c.get() + 1));
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
    COUNT.with(|c| c.set(0));
    ENABLED.with(|c| c.set(true));
    let r = f();
    ENABLED.with(|c| c.set(false));
    assert_eq!(COUNT.with(Cell::get), 0);
    r
}
fn target() -> ControlTarget {
    ControlTarget::Instrument {
        track: 0,
        instrument: 0,
    }
}
fn event(frame: u64, p: ControlParameter, value: f64, n: u64, curve: ControlCurve) -> ControlEvent {
    ControlEvent {
        ramp_frames: n,
        curve,
        ..ControlEvent::new(frame, target(), p, value)
    }
}
fn synth(instrument: Instrument, rate: u32, frames: u64, stereo: bool) -> Synth {
    let map = TempoMap::new(
        rate,
        48,
        &[TempoChange {
            tick: 0,
            bpm: 123.456,
        }],
    )
    .unwrap();
    let score = FrameScore {
        instruments: vec![instrument],
        tracks: vec![vec![FrameNote::new(0, frames, Pitch::new(48).unwrap(), 0)]],
        end_frame: frames,
    };
    let options = RenderOptions {
        gain: 0.125,
        aliasing: Aliasing::Naive,
        ..Default::default()
    };
    if stereo {
        Synth::from_frames_stereo(&score, &map, &options)
    } else {
        Synth::from_frames(&score, &map, &options)
    }
    .unwrap()
}
fn render(mut synth: Synth, sizes: &[usize]) -> (Vec<f32>, Vec<ControlReport>) {
    let ch = synth.channels();
    let mut chunk = vec![0.0; sizes.iter().max().unwrap() * ch];
    let mut out = Vec::new();
    let mut reports = Vec::new();
    let mut i = 0;
    loop {
        let n = no_alloc(|| synth.fill(&mut chunk[..sizes[i % sizes.len()] * ch]));
        out.extend_from_slice(&chunk[..n * ch]);
        while let Some(report) = no_alloc(|| synth.poll_control_event()) {
            reports.push(report);
        }
        if synth.is_finished() {
            break;
        }
        i += 1;
    }
    (out, reports)
}
fn wide() -> Instrument {
    Instrument {
        waveform: Waveform::Sawtooth,
        synthwave: Some(Synthwave {
            voices: 2,
            filter: Some(VoiceFilter::default()),
            pitch_lfo: Some(Lfo {
                rate: LfoRate::Beats {
                    numerator: 1,
                    denominator: 1,
                },
                depth: 12,
            }),
            cutoff_lfo: Some(Lfo {
                rate: LfoRate::Beats {
                    numerator: 2,
                    denominator: 1,
                },
                depth: 100,
            }),
            amplitude_lfo: Some(Lfo {
                rate: LfoRate::MilliHertz(3000),
                depth: 10,
            }),
            pan_lfo: Some(Lfo {
                rate: LfoRate::MilliHertz(2000),
                depth: 10,
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn held_expression_and_pan_have_exact_source_frame_ramps() {
    let base = synth(Instrument::default(), 8000, 1000, true);
    let control = render(base.clone(), &[4096]).0;
    let mut moving = base;
    moving
        .prepare_controls(
            &[
                event(
                    100,
                    ControlParameter::Expression,
                    0.0,
                    200,
                    ControlCurve::Linear,
                ),
                event(
                    400,
                    ControlParameter::Expression,
                    1.0,
                    100,
                    ControlCurve::Linear,
                ),
                event(600, ControlParameter::Pan, 1.0, 100, ControlCurve::Linear),
            ],
            8,
        )
        .unwrap();
    let (out, reports) = render(moving, &[1, 7, 64, 333]);
    for f in 0..1000 {
        let gain = if f <= 100 {
            1.0
        } else if f < 300 {
            1.0 - (f - 100) as f64 / 200.0
        } else if f <= 400 {
            0.0
        } else if f < 500 {
            (f - 400) as f64 / 100.0
        } else {
            1.0
        };
        let pan = if f <= 600 {
            0.0
        } else if f < 700 {
            (f - 600) as f64 / 100.0
        } else {
            1.0
        };
        assert!(
            (out[f * 2] as f64 - control[f * 2] as f64 * gain * (1.0 - pan)).abs() < 1e-7,
            "left frame {f}"
        );
        assert!(
            (out[f * 2 + 1] as f64 - control[f * 2 + 1] as f64 * gain).abs() < 1e-7,
            "right frame {f}"
        );
    }
    assert_eq!(&out[..202], &control[..202]);
    assert_eq!(
        reports.iter().map(|r| r.frame).collect::<Vec<_>>(),
        [100, 400, 600]
    );
    assert!(reports.iter().all(|r| r.status == ControlStatus::Applied));
}

#[test]
fn exponential_ramps_retarget_and_future_insertions_are_recompiled() {
    let mut source = synth(Instrument::default(), 8000, 1000, false);
    source
        .prepare_controls(
            &[event(
                100,
                ControlParameter::Expression,
                0.25,
                100,
                ControlCurve::Exponential,
            )],
            8,
        )
        .unwrap();
    source.fill(&mut [0.0; 101]);
    let value = source
        .control_value(target(), ControlParameter::Expression)
        .unwrap();
    assert!((value - 0.25f64.powf(0.01)).abs() < 1e-14);
    source.fill(&mut [0.0; 49]);
    assert!(
        (source
            .control_value(target(), ControlParameter::Expression)
            .unwrap()
            - 0.5)
            .abs()
            < 1e-14
    );
    let id = source
        .schedule_control(event(
            150,
            ControlParameter::Expression,
            1.0,
            50,
            ControlCurve::Linear,
        ))
        .unwrap();
    assert_eq!(id, ControlId(1));
    source.fill(&mut [0.0; 25]);
    assert!(
        (source
            .control_value(target(), ControlParameter::Expression)
            .unwrap()
            - 0.75)
            .abs()
            < 1e-14
    );
    source.fill(&mut [0.0; 25]);
    assert_eq!(
        source
            .control_value(target(), ControlParameter::Expression)
            .unwrap(),
        1.0
    );
    // An earlier future event changes the starting point of a previously queued ramp.
    let mut ordered = synth(Instrument::default(), 8000, 1000, false);
    ordered
        .prepare_controls(
            &[event(
                300,
                ControlParameter::Expression,
                1.0,
                100,
                ControlCurve::Exponential,
            )],
            8,
        )
        .unwrap();
    let previous = render(ordered.clone(), &[4096]);
    assert_eq!(
        ordered
            .schedule_control(event(
                200,
                ControlParameter::Expression,
                0.0,
                0,
                ControlCurve::Linear
            ))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        render(ordered.clone(), &[7, 31]),
        previous,
        "a new point cannot invalidate a later exponential ramp"
    );
    ordered
        .schedule_control(event(
            200,
            ControlParameter::Expression,
            0.25,
            0,
            ControlCurve::Linear,
        ))
        .unwrap();
    ordered.fill(&mut [0.0; 350]);
    assert!(
        (ordered
            .control_value(target(), ControlParameter::Expression)
            .unwrap()
            - 0.5)
            .abs()
            < 1e-14
    );
    let mut tiny = synth(Instrument::default(), 8000, 1000, false);
    tiny.prepare_controls(
        &[
            event(
                0,
                ControlParameter::Expression,
                f64::from_bits(1),
                0,
                ControlCurve::Linear,
            ),
            event(
                10,
                ControlParameter::Expression,
                1.0,
                100,
                ControlCurve::Exponential,
            ),
        ],
        2,
    )
    .unwrap();
    for _ in 0..111 {
        tiny.fill(&mut [0.0; 1]);
        let v = tiny
            .control_value(target(), ControlParameter::Expression)
            .unwrap();
        assert!(v.is_finite() && v > 0.0 && v <= 1.0);
    }
}

#[test]
fn targeting_is_per_track_instrument_or_authored_note_with_inactive_reports() {
    let map = TempoMap::new(
        8000,
        48,
        &[TempoChange {
            tick: 0,
            bpm: 120.0,
        }],
    )
    .unwrap();
    let score = FrameScore {
        instruments: vec![Instrument::default()],
        tracks: vec![
            vec![
                FrameNote::new(0, 200, Pitch::C4, 0),
                FrameNote::new(0, 200, Pitch::C4, 0),
                FrameNote::new(300, 500, Pitch::C4, 0),
            ],
            vec![FrameNote::new(0, 500, Pitch::C4, 0)],
        ],
        end_frame: 600,
    };
    let options = RenderOptions {
        gain: 0.125,
        aliasing: Aliasing::Naive,
        ..Default::default()
    };
    let mut source = Synth::from_frames(&score, &map, &options).unwrap();
    source
        .prepare_controls(
            &[
                ControlEvent::new(
                    0,
                    ControlTarget::Note { track: 0, note: 0 },
                    ControlParameter::Expression,
                    0.0,
                ),
                event(
                    0,
                    ControlParameter::Expression,
                    0.5,
                    0,
                    ControlCurve::Linear,
                ),
                ControlEvent::new(
                    20,
                    ControlTarget::Note { track: 0, note: 1 },
                    ControlParameter::Expression,
                    0.0,
                ),
            ],
            3,
        )
        .unwrap();
    source.fill(&mut [0.0; 1]);
    assert_eq!(
        source
            .control_value(target(), ControlParameter::Expression)
            .unwrap(),
        0.5
    );
    assert_eq!(
        source
            .control_value(
                ControlTarget::Instrument {
                    track: 1,
                    instrument: 0
                },
                ControlParameter::Expression
            )
            .unwrap(),
        1.0
    );
    let first = source.poll_control_event().unwrap();
    assert_eq!(first.status, ControlStatus::TargetInactive);
    source.fill(&mut [0.0; 20]);
    assert_eq!(
        source
            .control_value(
                ControlTarget::Note { track: 0, note: 1 },
                ControlParameter::Expression
            )
            .unwrap(),
        0.0
    );
    source.fill(&mut [0.0; 280]);
    assert_eq!(
        source
            .control_value(
                ControlTarget::Note { track: 0, note: 2 },
                ControlParameter::Expression
            )
            .unwrap(),
        0.5
    );
    assert_eq!(
        source.poll_control_event().unwrap().status,
        ControlStatus::Applied
    );
    assert_eq!(
        source.poll_control_event().unwrap().status,
        ControlStatus::Applied
    );
}

#[test]
fn invalid_controls_and_full_plans_preserve_the_running_source() {
    let mut source = synth(Instrument::default(), 8000, 1000, false);
    let valid = event(
        100,
        ControlParameter::Expression,
        0.5,
        100,
        ControlCurve::Linear,
    );
    source.prepare_controls(&[valid], 3).unwrap();
    let expected = render(source.clone(), &[4096]);
    for invalid in [
        event(
            100,
            ControlParameter::Expression,
            f64::NAN,
            0,
            ControlCurve::Linear,
        ),
        event(
            100,
            ControlParameter::Expression,
            1.01,
            0,
            ControlCurve::Linear,
        ),
        event(
            100,
            ControlParameter::CutoffHz,
            1000.0,
            0,
            ControlCurve::Linear,
        ),
        event(
            100,
            ControlParameter::Pan,
            0.5,
            100,
            ControlCurve::Exponential,
        ),
        event(
            100,
            ControlParameter::Expression,
            0.0,
            100,
            ControlCurve::Exponential,
        ),
        event(
            1000,
            ControlParameter::Expression,
            0.5,
            0,
            ControlCurve::Linear,
        ),
        ControlEvent {
            target: ControlTarget::Note { track: 0, note: 9 },
            ..valid
        },
        ControlEvent {
            target: ControlTarget::Instrument {
                track: 9,
                instrument: 0,
            },
            ..valid
        },
    ] {
        assert_eq!(
            source.schedule_control(invalid).unwrap_err().code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(render(source.clone(), &[1, 33]), expected);
    }
    assert_eq!(
        source
            .schedule_control(ControlEvent {
                ramp_frames: u64::MAX,
                ..valid
            })
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        source
            .prepare_controls(&[valid], MAX_SYNTH_CONTROLS + 1)
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        source
            .prepare_controls(
                &[
                    ControlEvent { frame: 90, ..valid },
                    ControlEvent { frame: 80, ..valid }
                ],
                3
            )
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(render(source.clone(), &[4096]), expected);
    for name in [
        "voices",
        "unison",
        "waveform",
        "attack-ms",
        "filter-mode",
        "lfo-rate",
        "not-a-control",
    ] {
        assert_eq!(
            name.parse::<ControlParameter>().unwrap_err().code,
            ErrorCode::InvalidArgument
        );
    }
    source
        .schedule_control(event(
            200,
            ControlParameter::Expression,
            0.25,
            0,
            ControlCurve::Linear,
        ))
        .unwrap();
    source
        .schedule_control(event(
            200,
            ControlParameter::Expression,
            1.0,
            0,
            ControlCurve::Linear,
        ))
        .unwrap();
    assert_eq!(source.controls_len(), 3);
    assert_eq!(
        source.schedule_control(valid).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    source.fill(&mut [0.0; 201]);
    assert_eq!(
        source
            .control_value(target(), ControlParameter::Expression)
            .unwrap(),
        1.0
    );
    // Consumed entries retain their capacity for replay.
    assert_eq!(
        source
            .schedule_control(event(
                300,
                ControlParameter::Expression,
                0.5,
                0,
                ControlCurve::Linear
            ))
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    let mut live = synth(Instrument::default(), 8000, 1000, false);
    live.prepare_controls(&[], 3).unwrap();
    live.fill(&mut [0.0; 100]);
    assert_eq!(
        live.schedule_control(event(
            99,
            ControlParameter::Expression,
            0.5,
            0,
            ControlCurve::Linear
        ))
        .unwrap_err()
        .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        live.prepare_controls(&[], 3).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn filter_pitch_and_all_lfo_depths_remain_bounded_across_extremes() {
    use ControlParameter::*;
    for rate in [1000, 4000, 8000, 32000, 44100, 48000, 192000, 768000] {
        let mut source = synth(wide(), rate, 4096, true);
        let mut events = vec![
            event(100, CutoffHz, 1.0, 128, ControlCurve::Exponential),
            event(100, ResonanceQ, 10.0, 128, ControlCurve::Linear),
            event(100, PitchCents, -2400.0, 128, ControlCurve::Linear),
            event(300, CutoffHz, 96000.0, 512, ControlCurve::Exponential),
            event(300, PitchCents, 2400.0, 512, ControlCurve::Linear),
            event(900, ResonanceQ, 0.5, 512, ControlCurve::Linear),
        ];
        for (p, value) in [
            (PitchLfoDepth, 2400.0),
            (CutoffLfoDepth, 9600.0),
            (AmplitudeLfoDepth, 100.0),
            (PanLfoDepth, 100.0),
        ] {
            events.push(event(1500, p, value, 512, ControlCurve::Linear));
        }
        source.prepare_controls(&events, 16).unwrap();
        let (out, reports) = render(source, &[1, 7, 64, 511]);
        assert!(
            out.iter().all(|s| s.is_finite() && s.abs() < 64.0),
            "{rate} Hz"
        );
        let difference = out
            .chunks_exact(2)
            .collect::<Vec<_>>()
            .windows(2)
            .map(|p| (p[1][0] - p[0][0]).abs().max((p[1][1] - p[0][1]).abs()))
            .fold(0.0, f32::max);
        assert!(
            difference < 64.0,
            "{rate} Hz maximum adjacent-frame step {difference}"
        );
        assert_eq!(reports.len(), events.len());
        assert!(reports.iter().all(|r| r.status == ControlStatus::Applied));
    }
}

fn fingerprint(samples: &[f32], reports: &[ControlReport]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for byte in samples
        .iter()
        .flat_map(|s| s.to_bits().to_le_bytes())
        .chain(reports.iter().flat_map(|r| {
            r.frame
                .to_le_bytes()
                .into_iter()
                .chain(r.id.0.to_le_bytes())
                .chain([u8::from(r.status == ControlStatus::Applied)])
        }))
    {
        h = (h ^ byte as u64).wrapping_mul(0x100000001b3);
    }
    h
}
#[test]
fn control_playback_is_pinned_chunk_independent_and_replayable() {
    let mut source = synth(wide(), 16000, 8000, true);
    source
        .prepare_controls(
            &[
                event(
                    100,
                    ControlParameter::Expression,
                    0.2,
                    200,
                    ControlCurve::Exponential,
                ),
                event(500, ControlParameter::Pan, -0.8, 700, ControlCurve::Linear),
                event(
                    700,
                    ControlParameter::CutoffHz,
                    6000.0,
                    1000,
                    ControlCurve::Exponential,
                ),
                event(
                    800,
                    ControlParameter::ResonanceQ,
                    4.0,
                    700,
                    ControlCurve::Linear,
                ),
                event(
                    1800,
                    ControlParameter::PitchCents,
                    700.0,
                    1200,
                    ControlCurve::Linear,
                ),
                event(
                    3000,
                    ControlParameter::PitchLfoDepth,
                    200.0,
                    200,
                    ControlCurve::Linear,
                ),
                event(
                    3000,
                    ControlParameter::CutoffLfoDepth,
                    800.0,
                    200,
                    ControlCurve::Linear,
                ),
                event(
                    3000,
                    ControlParameter::AmplitudeLfoDepth,
                    70.0,
                    200,
                    ControlCurve::Linear,
                ),
                event(
                    3000,
                    ControlParameter::PanLfoDepth,
                    40.0,
                    200,
                    ControlCurve::Linear,
                ),
            ],
            16,
        )
        .unwrap();
    let expected = render(source.clone(), &[10000]);
    assert_eq!(
        render(source.clone(), &[1, 7, 64, 333, 1000, 4096]),
        expected
    );
    source.fill(&mut [0.0; 1007]);
    no_alloc(|| source.reset());
    assert_eq!(render(source, &[31, 127]), expected);
    assert_eq!(
        fingerprint(&expected.0, &expected.1),
        10770329778336055541,
        "automated PCM/reports"
    );
}

#[test]
fn an_empty_prepared_plan_preserves_legacy_score_bits_and_loops_are_explicit() {
    let score=Score::parse("tempo 137\ninstrument pad voices=2 adsr=30,0,100,40 filter=lowpass cutoff=1800 lfo-pan=2000,30\nchannel a=@pad c4 e4 g4").unwrap();
    let original = Synth::new_stereo(&score, 8000, &Default::default()).unwrap();
    let mut enabled = original.clone();
    enabled.prepare_controls(&[], 16).unwrap();
    assert_eq!(render(original, &[10000]), render(enabled, &[1, 31, 333]));
    let mut looping = Synth::new(
        &score,
        8000,
        &RenderOptions {
            loops: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        looping.prepare_controls(&[], 16).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
}
