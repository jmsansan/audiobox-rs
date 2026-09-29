//! A small, dependency-free browser bridge around the real audiobox crate.
//! JavaScript owns no Rust pointers: input/output buffers stay in State and
//! views must be copied before the next exported call can grow WASM memory.
use audiobox::{
    decode_with_options, encode_with_options, Audio, AudioFormat, DecodeOptions, EncodeOptions,
    FadeCurve, FilterOptions, FilterType, Limits, NormalizeOptions,
};
use std::{cell::RefCell, f64::consts::TAU};

#[derive(Default)]
struct State {
    input: Vec<u8>,
    output: Vec<u8>,
    error: Vec<u8>,
    wave: Vec<f32>,
    source: Option<Audio>,
    result: Option<Audio>,
}
thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}
fn report(result: audiobox::Result<()>, state: &mut State) -> u32 {
    match result {
        Ok(()) => {
            state.error.clear();
            1
        }
        Err(error) => {
            state.error = error.to_string().into_bytes();
            0
        }
    }
}
fn demo(kind: u32) -> audiobox::Result<Audio> {
    let rate = 44100;
    let frames = rate as usize * 4;
    let mut noise = 0x12345678u32;
    let mut channels = vec![Vec::with_capacity(frames), Vec::with_capacity(frames)];
    for i in 0..frames {
        let t = i as f64 / rate as f64;
        noise ^= noise << 13;
        noise ^= noise >> 17;
        noise ^= noise << 5;
        for (c, channel) in channels.iter_mut().enumerate() {
            let sample = match kind {
                2 => {
                    let beat = t % 0.5;
                    let kick = (TAU * (65.0 * beat + 2.0 * (1.0 - (-25.0 * beat).exp()))).sin()
                        * (-18.0 * beat).exp();
                    let hat =
                        (noise as f64 / u32::MAX as f64 * 2.0 - 1.0) * (-90.0 * (t % 0.25)).exp();
                    0.48 * kick + 0.13 * hat
                }
                3 => {
                    let slope = (6000.0_f64 / 180.0).ln() / 4.0;
                    0.42 * (TAU * 180.0 * ((slope * t).exp() - 1.0) / slope).sin()
                }
                _ => {
                    let note = t % 1.0;
                    let envelope = (1.0 - (-35.0 * note).exp()) * (-2.5 * note).exp();
                    [220.0, 277.18, 329.63]
                        .iter()
                        .map(|f| (TAU * f * (1.0 + c as f64 * 0.003) * t).sin())
                        .sum::<f64>()
                        * 0.15
                        * envelope
                }
            };
            channel.push(sample as f32);
        }
    }
    Audio::new(channels, rate)
}

#[no_mangle]
pub extern "C" fn app_input(len: usize) -> usize {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if len > 16 * 1024 * 1024 {
            return 0;
        }
        s.input.resize(len, 0);
        s.input.as_mut_ptr() as usize
    })
}
#[no_mangle]
pub extern "C" fn app_load(kind: u32) -> u32 {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let loaded = if kind == 0 {
            decode_with_options(
                &s.input,
                &DecodeOptions {
                    limits: Limits {
                        max_duration_seconds: 30.0,
                        max_channels: 2,
                        max_sample_rate: 96000,
                        max_decoded_bytes: 24 * 1024 * 1024,
                        ..Limits::default()
                    },
                    ..DecodeOptions::default()
                },
            )
        } else {
            demo(kind)
        };
        let result = loaded.map(|audio| {
            s.result = Some(audio.clone());
            s.source = Some(audio);
        });
        report(result, &mut s)
    })
}
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn app_process(
    gain_db: f64,
    fade_in: f64,
    fade_out: f64,
    filter: u32,
    frequency: f64,
    mono: u32,
    reverse: u32,
    rate: u32,
    start: f64,
    end: f64,
    normalize: u32,
) -> u32 {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let process = || -> audiobox::Result<Audio> {
            let original = s.source.as_ref().ok_or_else(|| {
                audiobox::Error::new(audiobox::ErrorCode::InvalidArgument, "Load audio first")
            })?;
            let mut audio =
                original.cut(start, if end > 0.0 { end } else { original.duration() })?;
            if audio.frames() == 0 {
                return Err(audiobox::Error::new(
                    audiobox::ErrorCode::InvalidArgument,
                    "The selection is empty. Move the end after the start.",
                ));
            }
            if mono != 0 {
                audio = audio.to_mono();
            }
            if reverse != 0 {
                audio = audio.reverse();
            }
            if rate != 0 {
                audio = audio.resample(rate)?;
            }
            audio = audio.gain_db(gain_db)?;
            if filter != 0 {
                audio = audio.filter(FilterOptions::new(
                    if filter == 1 {
                        FilterType::Lowpass
                    } else {
                        FilterType::Highpass
                    },
                    frequency,
                ))?;
            }
            audio = audio.fade(fade_in, fade_out, FadeCurve::EqualPower)?;
            if normalize != 0 {
                audio = audio.normalize(NormalizeOptions::default())?;
            }
            Ok(audio)
        };
        let result = process().map(|audio| s.result = Some(audio));
        report(result, &mut s)
    })
}
#[no_mangle]
pub extern "C" fn app_stat(source: u32, metric: u32) -> f64 {
    STATE.with(|s| {
        let s = s.borrow();
        let audio = if source != 0 { &s.source } else { &s.result };
        audio.as_ref().map_or(0.0, |a| match metric {
            0 => a.sample_rate() as f64,
            1 => a.frames() as f64,
            2 => a.channels() as f64,
            3 => a.duration(),
            4 => a.peak_db(),
            _ => a.rms_db(),
        })
    })
}
#[no_mangle]
pub extern "C" fn app_wave(source: u32) -> usize {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let audio = if source != 0 { &s.source } else { &s.result };
        let wave = audio.as_ref().and_then(|a| a.waveform(512).ok());
        s.wave.clear();
        if let Some(wave) = wave {
            for (min, max) in wave.min.into_iter().zip(wave.max) {
                s.wave.extend([min, max]);
            }
        }
        s.wave.as_ptr() as usize
    })
}
#[no_mangle]
pub extern "C" fn app_encode(source: u32, format: u32) -> u32 {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let audio = if source != 0 { &s.source } else { &s.result };
        let result = audio
            .as_ref()
            .ok_or_else(|| {
                audiobox::Error::new(audiobox::ErrorCode::InvalidArgument, "Load audio first")
            })
            .and_then(|a| {
                encode_with_options(
                    a,
                    match format {
                        1 => AudioFormat::Flac,
                        2 => AudioFormat::Mp3,
                        _ => AudioFormat::Wav,
                    },
                    &EncodeOptions::default(),
                )
            })
            .map(|bytes| s.output = bytes);
        report(result, &mut s)
    })
}
#[no_mangle]
pub extern "C" fn app_output_ptr() -> usize {
    STATE.with(|s| s.borrow().output.as_ptr() as usize)
}
#[no_mangle]
pub extern "C" fn app_output_len() -> usize {
    STATE.with(|s| s.borrow().output.len())
}
#[no_mangle]
pub extern "C" fn app_error_ptr() -> usize {
    STATE.with(|s| s.borrow().error.as_ptr() as usize)
}
#[no_mangle]
pub extern "C" fn app_error_len() -> usize {
    STATE.with(|s| s.borrow().error.len())
}
