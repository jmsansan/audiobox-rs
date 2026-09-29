use audiobox::*;
use std::f64::consts::PI;
fn tone(frames: usize, rate: u32, hz: f64) -> Audio {
    Audio::new(
        vec![
            (0..frames)
                .map(|i| (0.6 * (2.0 * PI * hz * i as f64 / rate as f64).sin()) as f32)
                .collect(),
        ],
        rate,
    )
    .unwrap()
}
#[test]
fn planar_invariants_and_immutable_edits() {
    assert!(Audio::new(vec![], 48000).is_err());
    assert!(Audio::new(vec![vec![f32::NAN]], 48000).is_err());
    assert!(Audio::new(vec![vec![0.0], vec![]], 48000).is_err());
    assert!(Audio::new(vec![vec![]], 0).is_err());
    let a = Audio::new(vec![vec![1.0, 2.0, 3.0, 4.0]], 1000).unwrap();
    let clip = a
        .cut(TimePosition::Frames(1), TimePosition::Frames(3))
        .unwrap();
    assert_eq!(clip.channel_data(0).unwrap(), [2.0, 3.0]);
    assert_eq!(a.channel_data(0).unwrap(), [1.0, 2.0, 3.0, 4.0]);
    assert_eq!(
        a.remove(TimePosition::Frames(1), TimePosition::Frames(3))
            .unwrap()
            .channel_data(0)
            .unwrap(),
        [1.0, 4.0]
    );
    assert_eq!(
        a.cut(-0.002, 1.0).unwrap().channel_data(0).unwrap(),
        [3.0, 4.0]
    );
    assert_eq!(a.reverse().channel_data(0).unwrap(), [4.0, 3.0, 2.0, 1.0]);
    assert_eq!(a.concat(&a).unwrap().frames(), 8);
    assert_eq!(a.pad(0.002, 0.003).unwrap().frames(), 9);
    assert_eq!(
        a.to_stereo().to_mono().channel_data(0).unwrap(),
        a.channel_data(0).unwrap()
    );
    let mapped = a.to_stereo().map_channels(&[1, 0, 1]).unwrap();
    assert_eq!(mapped.channels(), 3);
    assert!(a.map_channels(&[1]).is_err());
    assert_eq!(Audio::merge(&[a.clone(), clip]).unwrap().frames(), 4);
}
#[test]
fn times_and_channel_layouts() {
    assert!(matches!(
        "1:02:03.5".parse::<TimePosition>().unwrap(),
        TimePosition::Seconds(3723.5)
    ));
    let a = Audio::silence("500ms".parse::<TimePosition>().unwrap(), 2, 48000).unwrap();
    assert_eq!(a.frames(), 24000);
    assert_eq!(a.duration_formatted(), "0:00.500");
    assert!("NaN".parse::<TimePosition>().is_err());
    assert!("-1:30".parse::<TimePosition>().is_err());
    let a = Audio::from_interleaved(&[1.0, 2.0, 3.0, 4.0], 2, 48000).unwrap();
    assert_eq!(a.channel_data(1).unwrap(), [2.0, 4.0]);
    assert_eq!(a.to_interleaved(), [1.0, 2.0, 3.0, 4.0]);
    assert!(Audio::from_interleaved(&[0.0; 3], 2, 48000).is_err());
}
#[test]
fn mix_crossfade_and_limits() {
    let a = Audio::new(vec![vec![0.5; 10]], 1000).unwrap();
    let b = Audio::new(vec![vec![1.0; 4]], 1000).unwrap();
    let m = a.mix(&b, -2, 2.0, true).unwrap();
    assert_eq!(m.channel_data(0).unwrap()[0], 2.5);
    assert_eq!(m.frames(), 10);
    assert_eq!(a.mix(&b, 9, 1.0, true).unwrap().frames(), 13);
    assert_eq!(a.crossfade_to(&b, 0.002).unwrap().frames(), 12);
    assert!(a.pad(TimePosition::Frames(i64::MAX), 0.0).is_err());
    assert!(a.to_channels(usize::MAX).is_err());
    assert!(a.speed(f64::NAN).is_err());
}
#[test]
fn level_and_waveform() {
    let a = tone(4800, 48000, 1000.0);
    let peak = a.peak_db();
    let out = a.normalize(NormalizeOptions::default()).unwrap();
    assert!((out.peak_db() + 1.0).abs() < 1e-5);
    assert_eq!(a.peak_db(), peak);
    let out = a.fade(0.01, 0.01, FadeCurve::EqualPower).unwrap();
    assert_eq!(out.channel_data(0).unwrap()[0], 0.0);
    assert_eq!(*out.channel_data(0).unwrap().last().unwrap(), 0.0);
    let a = Audio::new(vec![vec![-0.8, 0.9, -0.2, 0.1]], 48000).unwrap();
    let w = a.waveform(2).unwrap();
    assert_eq!(w.min, [-0.8, -0.2]);
    assert_eq!(w.max, [0.9, 0.1]);
    let a = Audio::new(vec![vec![0.4; 100]], 48000).unwrap();
    assert!(a.remove_dc_offset().peak() < 1e-7);
    let silence = Audio::silence(0.1, 1, 48000).unwrap();
    assert_eq!(silence.rms_db(), f64::NEG_INFINITY);
    assert_eq!(silence.loudness().integrated, f64::NEG_INFINITY);
    assert!(silence.true_peak_db().is_infinite());
}
#[test]
fn resampling_rejects_aliases_and_preserves_dc() {
    let a = Audio::new(vec![vec![0.5; 4800]], 48000)
        .unwrap()
        .resample(16000)
        .unwrap();
    assert_eq!(a.frames(), 1600);
    assert!(
        a.channel_data(0)
            .unwrap()
            .iter()
            .all(|&s| (s - 0.5).abs() < 1e-6)
    );
    let high = tone(4800, 48000, 14000.0).resample(16000).unwrap();
    let middle = &high.channel_data(0).unwrap()[100..1500];
    let rms =
        (middle.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / middle.len() as f64).sqrt();
    assert!(rms < 0.001, "alias RMS {rms}");
}
#[test]
fn filters_limiter_silence_and_time() {
    let a = tone(4800, 48000, 8000.0);
    let b = a
        .filter(FilterOptions::new(FilterType::Lowpass, 1000.0))
        .unwrap();
    assert!(b.rms_db() < a.rms_db() - 25.0);
    let b = a
        .gain(4.0)
        .unwrap()
        .limit(LimiterOptions::default())
        .unwrap();
    assert!(b.peak_db() <= -1.0 + 1e-5);
    let padded = a.pad(0.1, 0.1).unwrap();
    let trim = padded
        .trim_silence(SilenceOptions {
            min_duration_ms: 10.0,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(trim.frames(), a.frames());
    let a = tone(12000, 24000, 440.0);
    assert_eq!(a.speed(2.0).unwrap().frames(), 6000);
    assert_eq!(a.tempo(1.5).unwrap().frames(), 8000);
    assert_eq!(a.pitch(3.0).unwrap().frames(), 12000);
    let tempo = a.tempo(1.5).unwrap();
    let spectrum = analyze::spectrum(&tempo.channel_data(0).unwrap()[1000..], 24000, 4096).unwrap();
    let peak = spectrum
        .magnitudes
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    assert!((spectrum.frequencies[peak] - 440.0).abs() < 15.0);
}
#[test]
fn spectral_and_loudness_reference() {
    let a = tone(48000, 48000, 1000.0);
    let s = analyze::spectrum(a.channel_data(0).unwrap(), 48000, 4096).unwrap();
    let peak = s
        .magnitudes
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    assert!((s.frequencies[peak] - 1000.0).abs() < 12.0);
    // A 1 kHz sine at amplitude 0.6 is approximately -7.47 LUFS after K-weighting.
    assert!(
        (a.loudness().integrated + 7.47).abs() < 0.2,
        "{}",
        a.loudness().integrated
    );
    assert!(analyze::spectrum(&[], 48000, 7).is_err());
    assert!(analyze::spectrogram(&[], 48000, 1024, 0).is_err());
}
