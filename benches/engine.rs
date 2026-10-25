//! Reproducible release baselines for the engine (#78). See `docs/performance.md`.
//!
//! ```text
//! cargo bench --bench engine --all-features -- [FILTER...] [--skip PATTERN] [--quick] [--runs N] [--csv PATH]
//! cargo bench --bench engine --all-features -- --list
//! cargo bench --bench engine --all-features -- --compare BASE.csv NEW.csv
//! ```
//!
//! Fixtures are generated before timing. Each case runs once to warm up, `--runs`
//! times with allocation counting off for wall time, and once more with counting on
//! for allocation figures, so the counters never inflate the reported times.
use audiobox::analyze;
#[cfg(all(
    feature = "assets",
    feature = "file-stream",
    not(target_family = "wasm")
))]
use audiobox::assets::{AssetKey, AudioCache, Loader, LoaderConfig};
use audiobox::mixer::effects::{Chorus, Compressor, Delay, Effect, Phaser, Reverb};
use audiobox::mixer::{
    Attenuation, Emitter, Fade, Loop, MasterLimiter, Mixer, MixerConfig, OutputLayout, Play, Sound,
    Variation,
};
use audiobox::sfx::{Effect as Sfx, Preset};
use audiobox::stream::Decoder;
#[cfg(all(
    feature = "assets",
    feature = "file-stream",
    not(target_family = "wasm")
))]
use audiobox::stream::FileStream;
use audiobox::synth::{RenderOptions, Score, Synth};
use audiobox::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

// Allocation counters for every thread, enabled only around tracked phases. Reader
// and loader threads count too, so their work is attributed to the phase it overlaps.
static TRACK: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static REALLOCS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);
static MOVED: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

struct Counting;
fn grow(size: usize, old: usize) {
    let live = LIVE.fetch_add(size as isize - old as isize, Relaxed) + size as isize - old as isize;
    PEAK.fetch_max(live, Relaxed);
    REQUESTED.fetch_add(size, Relaxed);
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if TRACK.load(Relaxed) && !p.is_null() {
            ALLOCS.fetch_add(1, Relaxed);
            grow(layout.size(), 0);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if TRACK.load(Relaxed) && !p.is_null() {
            ALLOCS.fetch_add(1, Relaxed);
            grow(layout.size(), 0);
        }
        p
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(p, layout, size) };
        if TRACK.load(Relaxed) && !next.is_null() {
            REALLOCS.fetch_add(1, Relaxed);
            if next != p {
                MOVED.fetch_add(layout.size().min(size), Relaxed);
            }
            grow(size, layout.size());
        }
        next
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        if TRACK.load(Relaxed) {
            FREES.fetch_add(1, Relaxed);
            LIVE.fetch_sub(layout.size() as isize, Relaxed);
        }
        unsafe { System.dealloc(p, layout) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;

#[derive(Clone, Copy, Debug, Default)]
struct Allocs {
    allocs: usize,
    reallocs: usize,
    frees: usize,
    requested: usize,
    moved: usize,
    /// Highest net bytes allocated during the phase and not yet freed.
    peak: usize,
}
fn start_tracking() {
    for c in [&ALLOCS, &REALLOCS, &FREES, &REQUESTED, &MOVED] {
        c.store(0, Relaxed);
    }
    LIVE.store(0, Relaxed);
    PEAK.store(0, Relaxed);
    TRACK.store(true, Relaxed);
}
fn stop_tracking() -> Allocs {
    TRACK.store(false, Relaxed);
    Allocs {
        allocs: ALLOCS.load(Relaxed),
        reallocs: REALLOCS.load(Relaxed),
        frees: FREES.load(Relaxed),
        requested: REQUESTED.load(Relaxed),
        moved: MOVED.load(Relaxed),
        peak: PEAK.load(Relaxed).max(0) as usize,
    }
}

/// Audio a phase processed: frames at a sample rate, for throughput and real-time factor.
#[derive(Clone, Copy, Debug, Default)]
struct Work {
    frames: u64,
    rate: u32,
}
fn work(frames: usize, rate: u32) -> Work {
    Work {
        frames: frames as u64,
        rate,
    }
}
const NONE: Work = Work { frames: 0, rate: 0 };

#[derive(Default)]
struct Phase {
    name: &'static str,
    ns: Vec<u64>,
    work: Work,
    allocs: Option<Allocs>,
    /// Per-callback times of the tracked-off runs, and each callback's deadline.
    callbacks: Vec<u64>,
    deadline_ns: u64,
}

struct Recorder {
    tracked: bool,
    phases: Vec<Phase>,
    checksum: u64,
}
impl Recorder {
    fn entry(&mut self, name: &'static str) -> &mut Phase {
        if let Some(i) = self.phases.iter().position(|p| p.name == name) {
            return &mut self.phases[i];
        }
        self.phases.push(Phase {
            name,
            ..Phase::default()
        });
        self.phases.last_mut().unwrap()
    }
    /// Times `f`, which returns its result and the audio it processed.
    fn phase<T>(&mut self, name: &'static str, f: impl FnOnce() -> (T, Work)) -> T {
        if self.tracked {
            start_tracking();
        }
        let start = Instant::now();
        let (value, work) = f();
        let ns = start.elapsed().as_nanos() as u64;
        let tracked = self.tracked.then(stop_tracking);
        let value = black_box(value);
        let p = self.entry(name);
        p.work = work;
        match tracked {
            Some(a) => p.allocs = Some(a),
            None => p.ns.push(ns),
        }
        value
    }
    /// Times every call of `block` separately, as an audio callback of `frames` frames
    /// at `rate`. `control` runs before each block, untimed and uncounted, for the work
    /// a game thread does between callbacks: commands, event polling.
    fn callbacks(
        &mut self,
        name: &'static str,
        count: usize,
        frames: usize,
        rate: u32,
        mut control: impl FnMut(usize),
        mut block: impl FnMut(usize),
    ) {
        let mut times = Vec::with_capacity(count);
        if self.tracked {
            start_tracking();
        }
        for i in 0..count {
            TRACK.store(false, Relaxed);
            control(i);
            TRACK.store(self.tracked, Relaxed);
            let start = Instant::now();
            block(i);
            times.push(start.elapsed().as_nanos() as u64);
        }
        let tracked = self.tracked.then(stop_tracking);
        let p = self.entry(name);
        p.work = work(count * frames, rate);
        p.deadline_ns = frames as u64 * 1_000_000_000 / rate as u64;
        match tracked {
            Some(a) => p.allocs = Some(a),
            None => {
                p.ns.push(times.iter().sum());
                p.callbacks.extend(times);
            }
        }
    }
    fn hash_samples(&mut self, samples: &[f32]) {
        for s in samples {
            self.checksum = (self.checksum ^ s.to_bits() as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    fn hash_audio(&mut self, audio: &Audio) {
        for c in audio.all_channels() {
            self.hash_samples(c);
        }
    }
    fn hash_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.checksum = (self.checksum ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
}

type Run = Box<dyn FnMut(&mut Recorder)>;
struct Case {
    name: String,
    /// Builds the fixture, outside every timing, and returns one repetition.
    setup: Box<dyn Fn() -> Run>,
}
fn case(name: impl Into<String>, setup: impl Fn() -> Run + 'static) -> Case {
    Case {
        name: name.into(),
        setup: Box::new(setup),
    }
}

// ---- Fixtures -------------------------------------------------------------------

/// Deterministic music-like test signal: decaying harmonic notes on a bass line,
/// with a little noise, so codecs and analysis see broadband changing content.
fn music(seconds: f64, rate: u32, channels: usize) -> Audio {
    let frames = (seconds * rate as f64) as usize;
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let note = rate as usize / 4;
    let scale = [261.63, 293.66, 329.63, 392.0, 440.0, 523.25, 587.33, 659.26];
    let mut data = vec![Vec::with_capacity(frames); channels];
    for i in 0..frames {
        let n = i / note;
        let t = (i % note) as f64 / rate as f64;
        let f = scale[(n * 5 + n / 8) % scale.len()];
        let env = (-t * 6.0).exp();
        let bass = 0.2 * (std::f64::consts::TAU * f / 4.0 * i as f64 / rate as f64).sin();
        let mut lead = 0.0;
        for h in 1..6 {
            lead += (std::f64::consts::TAU * f * h as f64 * t).sin() / h as f64;
        }
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let noise = (seed >> 40) as f64 / (1u64 << 24) as f64 - 0.5;
        for (c, channel) in data.iter_mut().enumerate() {
            let pan = if c % 2 == 0 { 0.8 } else { 1.0 };
            channel.push((bass + 0.3 * env * lead * pan + 0.02 * noise) as f32);
        }
    }
    Audio::new(data, rate).unwrap()
}

const CHIP: &str = "tempo 140
instrument lead wave=pulse duty=25 adsr=4,80,60,60 vibrato=12,170,200
instrument bass wave=triangle adsr=2,0,100,25
instrument arp wave=pulse duty=12 adsr=1,40,50,20
instrument perc wave=noise adsr=0,30,0,10
channel lead = @lead o5 l8 q7 L [c e g e >c< g e g]2 [d f a f >d< a f a]2
channel bass = @bass o2 l4 L [c c g g]2 [d d a a]2
channel arp = @arp o6 l16 L [c e g >c<]8 [d f a >d<]8
channel perc = @perc o4 l8 L [c r c c r c r c]4";

const SYNTHWAVE: &str = "tempo 110
instrument bass wave=saw voices=1 glide=80 legato=on adsr=5,120,60,40 filter=lowpass cutoff=500 resonance=160 filter-env=2400 filter-adsr=0,200,0,40
instrument pad wave=saw unison=5 detune=18 spread=90 voices=6 adsr=120,150,70,300 filter=lowpass cutoff=1800 resonance=90 keytrack=40 lfo-cutoff=beats,4,1,600
instrument lead wave=pulse voices=1 glide=60 legato=on adsr=5,80,80,100 lfo-pitch=5000,12 lfo-pan=beats,2,1,30
channel bass=@bass o2 l8 L [c c >c< g]4
channel pad=@pad o4 L {c e g}1 {f a >c}1
channel lead=@lead o5 l8 L [c e g >c< g e d g]2";

const DRUMS: &str = "tempo 120
instrument k drum=kick kit=electronic
instrument s drum=snare kit=electronic drum-seed=123
instrument h drum=closed-hat kit=electronic
instrument o drum=open-hat kit=electronic
instrument c drum=clap kit=electronic
channel kick = @k l8 L [c!127 r c!80 r c!110 c!70 r c!100]2
channel snare = @s L [r4 c4!100]4
channel hats = l8 L [@h c!90 c!50 c!80 @o c!80 @h c!90 c!50 c!80 r]2
channel clap = @c L [r2 c4!90 r4]2";

#[cfg(all(
    feature = "assets",
    feature = "file-stream",
    not(target_family = "wasm")
))]
fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("audiobox-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---- Cases ----------------------------------------------------------------------

const RATE: u32 = 48_000;
const BLOCK: usize = 256;

fn codec_cases(cases: &mut Vec<Case>, seconds: f64, tag: &str) {
    for (format, label) in [
        (AudioFormat::Wav, "wav"),
        (AudioFormat::Flac, "flac"),
        (AudioFormat::Mp3, "mp3"),
    ] {
        let layouts: &[(u32, usize, &str)] = if seconds <= 10.0 {
            &[(RATE, 2, ""), (44_100, 1, "-mono44k")]
        } else {
            &[(RATE, 2, "")]
        };
        for &(rate, channels, layout) in layouts {
            cases.push(case(
                format!("codec/{label}-decode/{tag}{layout}"),
                move || {
                    let bytes = encode(&music(seconds, rate, channels), format).unwrap();
                    Box::new(move |r: &mut Recorder| {
                        let audio = r.phase("decode", || {
                            let a = decode(&bytes).unwrap();
                            let w = work(a.frames(), a.sample_rate());
                            (a, w)
                        });
                        r.hash_audio(&audio);
                        r.phase("drop", || (drop(audio), NONE));
                    })
                },
            ));
        }
        cases.push(case(format!("codec/{label}-encode/{tag}"), move || {
            let audio = music(seconds, RATE, 2);
            Box::new(move |r: &mut Recorder| {
                let bytes = r.phase("encode", || {
                    (encode(&audio, format).unwrap(), work(audio.frames(), RATE))
                });
                r.hash_bytes(&bytes);
            })
        }));
        if format != AudioFormat::Wav || seconds <= 10.0 {
            cases.push(case(format!("codec/{label}-stream/{tag}"), move || {
                let bytes = encode(&music(seconds, RATE, 2), format).unwrap();
                let mut buffer = vec![0.0f32; 4096 * 2];
                Box::new(move |r: &mut Recorder| {
                    let mut decoder = r.phase("open", || (Decoder::new(&bytes[..]).unwrap(), NONE));
                    let first = r.phase("first-block", || {
                        let n = decoder.read(&mut buffer).unwrap();
                        (n, work(n, RATE))
                    });
                    r.hash_samples(&buffer[..first * 2]);
                    let mut total = 0;
                    let mut hash = Vec::new();
                    r.phase("steady", || {
                        loop {
                            let n = decoder.read(&mut buffer).unwrap();
                            if n == 0 {
                                break;
                            }
                            total += n;
                            hash.push(buffer[0]);
                        }
                        ((), work(total, RATE))
                    });
                    r.hash_samples(&hash);
                    r.phase("drop", || (drop(decoder), NONE));
                })
            }));
        }
    }
    if seconds <= 10.0 {
        cases.push(case(format!("codec/aiff-decode/{tag}"), move || {
            let bytes = encode(&music(seconds, RATE, 2), AudioFormat::Aiff).unwrap();
            Box::new(move |r: &mut Recorder| {
                let audio = r.phase("decode", || {
                    let a = decode(&bytes).unwrap();
                    let w = work(a.frames(), a.sample_rate());
                    (a, w)
                });
                r.hash_audio(&audio);
            })
        }));
    }
}

fn analysis_cases(cases: &mut Vec<Case>, seconds: f64, tag: &str) {
    for (fft, hop) in [(2048, 512), (1024, 128)] {
        cases.push(case(
            format!("analysis/spectrogram-{fft}-{hop}/{tag}"),
            move || {
                let mono = music(seconds, RATE, 1);
                Box::new(move |r: &mut Recorder| {
                    let samples = mono.channel_data(0).unwrap();
                    let s = r.phase("compute", || {
                        (
                            analyze::spectrogram(samples, RATE, fft, hop).unwrap(),
                            work(samples.len(), RATE),
                        )
                    });
                    for row in &s.magnitudes {
                        r.checksum = r.checksum.wrapping_add(row.len() as u64);
                        r.hash_samples(&[row[row.len() / 3] as f32]);
                    }
                })
            },
        ));
    }
    cases.push(case(format!("analysis/loudness/{tag}"), move || {
        let audio = music(seconds, RATE, 2);
        Box::new(move |r: &mut Recorder| {
            let l = r.phase("compute", || (audio.loudness(), work(audio.frames(), RATE)));
            r.hash_samples(&[l.integrated as f32]);
        })
    }));
    cases.push(case(format!("analysis/true-peak/{tag}"), move || {
        let audio = music(seconds, RATE, 2);
        Box::new(move |r: &mut Recorder| {
            let p = r.phase("compute", || {
                (audio.true_peak_db(), work(audio.frames(), RATE))
            });
            r.hash_samples(&[p as f32]);
        })
    }));
    type Transform = fn(&Audio) -> Audio;
    let transforms: [(&str, Transform); 5] = [
        ("resample-44k1", |a| a.resample(44_100).unwrap()),
        ("filter-lowpass", |a| {
            a.filter(FilterOptions::new(FilterType::Lowpass, 2000.0))
                .unwrap()
        }),
        ("limit", |a| a.limit(LimiterOptions::default()).unwrap()),
        ("tempo-1.25", |a| a.tempo(1.25).unwrap()),
        ("pitch-up3", |a| a.pitch(3.0).unwrap()),
    ];
    for (name, transform) in transforms {
        cases.push(case(format!("dsp/{name}/{tag}"), move || {
            let audio = music(seconds, RATE, 2);
            Box::new(move |r: &mut Recorder| {
                let out = r.phase("transform", || {
                    (transform(&audio), work(audio.frames(), RATE))
                });
                r.hash_audio(&out);
            })
        }));
    }
}

fn synth_cases(cases: &mut Vec<Case>, seconds: f64, tag: &str) {
    for (name, text, stereo) in [
        ("chip", CHIP, false),
        ("synthwave", SYNTHWAVE, true),
        ("drums", DRUMS, false),
    ] {
        cases.push(case(format!("synth/{name}/{tag}"), move || {
            let frames = (seconds * RATE as f64) as usize;
            let channels = if stereo { 2 } else { 1 };
            let mut out = vec![0.0f32; BLOCK * channels];
            Box::new(move |r: &mut Recorder| {
                let score = r.phase("parse", || (Score::parse(text).unwrap(), NONE));
                let options = RenderOptions {
                    loops: None,
                    ..RenderOptions::default()
                };
                let mut synth = r.phase("prepare", || {
                    let s = if stereo {
                        Synth::new_stereo(&score, RATE, &options)
                    } else {
                        Synth::new(&score, RATE, &options)
                    };
                    (s.unwrap(), NONE)
                });
                r.phase("first-block", || (synth.fill(&mut out), work(BLOCK, RATE)));
                r.hash_samples(&out);
                let mut hash = 0.0f32;
                r.callbacks(
                    "steady",
                    frames / BLOCK,
                    BLOCK,
                    RATE,
                    |_| {},
                    |_| {
                        synth.fill(&mut out);
                        hash += out[BLOCK / 2];
                    },
                );
                r.hash_samples(&[hash]);
                r.phase("drop", || (drop(synth), NONE));
            })
        }));
    }
    cases.push(case(format!("sfx/presets/{tag}"), move || {
        Box::new(move |r: &mut Recorder| {
            let presets = [
                Preset::Pickup,
                Preset::Laser,
                Preset::Explosion,
                Preset::PowerUp,
                Preset::Hit,
                Preset::Jump,
                Preset::Blip,
            ];
            let sounds = r.phase("render", || {
                let sounds: Vec<Audio> = (0..4u64)
                    .flat_map(|seed| presets.map(|p| Sfx::preset(p, seed).render(RATE).unwrap()))
                    .collect();
                let frames = sounds.iter().map(Audio::frames).sum();
                (sounds, work(frames, RATE))
            });
            for s in &sounds {
                r.hash_audio(s);
            }
        })
    }));
}

struct MixerSetup {
    voices: usize,
    source_rate: u32,
    pitched: bool,
    effects: bool,
    spatial: bool,
    automation: bool,
    burst: bool,
    layout: OutputLayout,
    irregular: bool,
}
const PLAIN: MixerSetup = MixerSetup {
    voices: 0,
    source_rate: RATE,
    pitched: false,
    effects: false,
    spatial: false,
    automation: false,
    burst: false,
    layout: OutputLayout::Stereo,
    irregular: false,
};

fn mixer_case(cases: &mut Vec<Case>, name: &str, seconds: f64, tag: &str, setup: MixerSetup) {
    cases.push(case(format!("mixer/{name}/{tag}"), move || {
        let tone = Arc::new(music(1.37, setup.source_rate, 1));
        let shot = Arc::new(music(0.05, setup.source_rate, 1));
        let channels = setup.layout.channels();
        let callbacks = (seconds * RATE as f64) as usize / BLOCK;
        let sizes = [1usize, 37, 128, 480, 1024, 256];
        let mut out = vec![0.0f32; 1024 * channels];
        let setup = MixerSetup { ..setup };
        Box::new(move |r: &mut Recorder| {
            let (mut mixer, mut handle) = r.phase("prepare", || {
                let config = MixerConfig {
                    sample_rate: RATE,
                    channels,
                    max_voices: if setup.burst { 32 } else { setup.voices.max(1) },
                    buses: vec!["fx".into(), "key".into()],
                    queue: 1024,
                    limiter: setup.effects.then(|| MasterLimiter {
                        lookahead_ms: 2.0,
                        ..MasterLimiter::default()
                    }),
                    ..MixerConfig::default()
                };
                (Mixer::new(config).unwrap(), NONE)
            });
            let fx = handle.bus("fx").unwrap();
            let key = handle.bus("key").unwrap();
            if setup.effects {
                handle
                    .set_effects(
                        fx,
                        &[
                            Effect::LowPass { cutoff_hz: 6000.0 },
                            Effect::Compressor(Compressor::ducking(key)),
                            Effect::Chorus(Chorus::default()),
                            Effect::Phaser(Phaser::default()),
                            Effect::Delay(Delay::default()),
                            Effect::Reverb(Reverb::default()),
                        ],
                    )
                    .unwrap();
            }
            let mut ids = Vec::new();
            for v in 0..setup.voices {
                let play = Play {
                    bus: if setup.effects && v % 8 == 0 { key } else { fx },
                    looping: Loop::Whole,
                    gain: 0.05,
                    pan: (v as f32 / setup.voices.max(1) as f32) * 2.0 - 1.0,
                    rate: if setup.pitched {
                        0.75 + (v % 7) as f32 * 0.1
                    } else {
                        1.0
                    },
                    emitter: setup.spatial.then(|| Emitter {
                        position: [(v as f32 - 16.0) * 2.0, 0.0, -10.0],
                        velocity: [3.0, 0.0, 0.0],
                        attenuation: Attenuation::default(),
                    }),
                    ..Play::default()
                };
                ids.push(handle.play(tone.clone(), &play).unwrap());
            }
            let mut hash = 0.0f32;
            if setup.irregular {
                let count = callbacks * BLOCK / (sizes.iter().sum::<usize>() / sizes.len());
                let mut frames_done = 0;
                r.callbacks(
                    "render",
                    count,
                    0,
                    RATE,
                    |_| {},
                    |i| {
                        let n = sizes[i % sizes.len()];
                        mixer.render(&mut out[..n * channels]);
                        hash += out[0];
                        frames_done += n;
                    },
                );
                // Irregular blocks have no single deadline: report throughput only.
                let p = r.entry("render");
                p.work = work(frames_done, RATE);
                p.deadline_ns = 0;
            } else {
                r.callbacks(
                    "render",
                    callbacks,
                    BLOCK,
                    RATE,
                    |i| {
                        let frame = i as u64 * BLOCK as u64;
                        if setup.automation && i % 4 == 0 {
                            for (k, &id) in ids.iter().enumerate() {
                                let gain = 0.03 + 0.02 * ((i / 4 + k) % 3) as f32;
                                let _ =
                                    handle
                                        .at(frame + 64)
                                        .set_gain(id, gain, Fade::linear(0.005));
                            }
                        }
                        if setup.spatial && i % 8 == 0 {
                            for (k, &id) in ids.iter().enumerate() {
                                let x = ((i / 8 + k) % 64) as f32 - 32.0;
                                let emitter = Emitter {
                                    position: [x, 1.0, -8.0],
                                    velocity: [6.0, 0.0, 0.0],
                                    attenuation: Attenuation::default(),
                                };
                                let _ =
                                    handle
                                        .at(frame)
                                        .set_emitter(id, emitter, Fade::linear(0.02));
                            }
                        }
                        if setup.burst {
                            for k in 0..16 {
                                let play = Play {
                                    bus: fx,
                                    gain: 0.05,
                                    priority: (k * 16) as u8,
                                    variation: Variation {
                                        pitch: 1.0,
                                        gain_db: 1.0,
                                    },
                                    ..Play::default()
                                };
                                let _ = handle.at(frame + k * 13).play(shot.clone(), &play);
                            }
                            // Events wait ten callbacks, so returns pile up as in a busy game.
                            if i % 10 == 9 {
                                while handle.poll_event().is_some() {}
                            }
                        }
                    },
                    |_| {
                        mixer.render(&mut out[..BLOCK * channels]);
                        hash += out[1];
                    },
                );
            }
            r.hash_samples(&[hash]);
            r.phase("drop", || {
                drop(mixer);
                while handle.poll_event().is_some() {}
                (drop(handle), NONE)
            });
        })
    }));
}

fn mixer_cases(cases: &mut Vec<Case>, seconds: f64, tag: &str) {
    mixer_case(cases, "empty", seconds, tag, PLAIN);
    for voices in [16, 64] {
        mixer_case(
            cases,
            &format!("voices-{voices}"),
            seconds,
            tag,
            MixerSetup { voices, ..PLAIN },
        );
    }
    mixer_case(
        cases,
        "voices-64-resampled",
        seconds,
        tag,
        MixerSetup {
            voices: 64,
            source_rate: 44_100,
            ..PLAIN
        },
    );
    mixer_case(
        cases,
        "voices-64-pitched",
        seconds,
        tag,
        MixerSetup {
            voices: 64,
            pitched: true,
            ..PLAIN
        },
    );
    mixer_case(
        cases,
        "effects-16",
        seconds,
        tag,
        MixerSetup {
            voices: 16,
            effects: true,
            ..PLAIN
        },
    );
    mixer_case(
        cases,
        "automation-16",
        seconds,
        tag,
        MixerSetup {
            voices: 16,
            automation: true,
            ..PLAIN
        },
    );
    mixer_case(
        cases,
        "spatial-32-5.1",
        seconds,
        tag,
        MixerSetup {
            voices: 32,
            spatial: true,
            layout: OutputLayout::Surround51,
            ..PLAIN
        },
    );
    mixer_case(
        cases,
        "burst-steal",
        seconds,
        tag,
        MixerSetup {
            burst: true,
            ..PLAIN
        },
    );
    mixer_case(
        cases,
        "irregular-16",
        seconds,
        tag,
        MixerSetup {
            voices: 16,
            irregular: true,
            ..PLAIN
        },
    );
    cases.push(case(format!("mixer/synth-stream/{tag}"), move || {
        let mut out = vec![0.0f32; BLOCK * 2];
        let callbacks = (seconds * RATE as f64) as usize / BLOCK;
        Box::new(move |r: &mut Recorder| {
            let (mut mixer, mut handle) = Mixer::new(MixerConfig::default()).unwrap();
            let score = Score::parse(CHIP).unwrap();
            let options = RenderOptions {
                loops: None,
                ..RenderOptions::default()
            };
            let synth = Synth::new(&score, RATE, &options).unwrap();
            handle.play(Sound::from(synth), &Play::default()).unwrap();
            let mut hash = 0.0f32;
            r.callbacks(
                "render",
                callbacks,
                BLOCK,
                RATE,
                |_| {},
                |_| {
                    mixer.render(&mut out);
                    hash += out[0];
                },
            );
            r.hash_samples(&[hash]);
        })
    }));
}

/// Native disk streaming and background loading; WebAssembly has no reader threads.
#[cfg(all(
    feature = "assets",
    feature = "file-stream",
    not(target_family = "wasm")
))]
fn stream_cases(cases: &mut Vec<Case>, seconds: f64, tag: &str) {
    for (format, label) in [(AudioFormat::Flac, "flac"), (AudioFormat::Mp3, "mp3")] {
        cases.push(case(format!("stream/file-{label}/{tag}"), move || {
            let path = temp_dir().join(format!("stream-{label}-{seconds}.{label}"));
            let bytes = encode(&music(seconds, RATE, 2), format).unwrap();
            std::fs::write(&path, bytes).unwrap();
            let mut out = vec![0.0f32; BLOCK * 2];
            Box::new(move |r: &mut Recorder| {
                let (mut stream, handle) =
                    r.phase("open", || (FileStream::open(&path).unwrap(), NONE));
                // Time to first PCM: retry reads until the reader publishes a frame.
                let first = r.phase("first-pcm", || {
                    let n = loop {
                        match stream.read(&mut out) {
                            Ok(n) => break n,
                            Err(e) if e.code == ErrorCode::Underrun => std::hint::spin_loop(),
                            Err(e) => panic!("{e}"),
                        }
                    };
                    (n, work(n, RATE))
                });
                r.hash_samples(&out[..first * 2]);
                let mut total = first;
                let mut underruns = 0u64;
                // A running sum in sample order does not depend on how reads split.
                let mut sum = 0.0f64;
                r.phase("sustained", || {
                    loop {
                        match stream.read(&mut out) {
                            Ok(0) => break,
                            Ok(n) => {
                                total += n;
                                sum = out[..n * 2].iter().fold(sum, |a, &s| a + s as f64);
                            }
                            Err(e) if e.code == ErrorCode::Underrun => {
                                underruns += 1;
                                std::thread::yield_now();
                            }
                            Err(e) => panic!("{e}"),
                        }
                    }
                    ((), work(total - first, RATE))
                });
                r.hash_samples(&[sum as f32]);
                r.checksum = r.checksum.wrapping_add(total as u64);
                // Underruns while draining as fast as possible show reader-bound
                // throughput, not a playback failure; report them in the summary.
                r.entry("sustained").deadline_ns = underruns;
                let middle = total as u64 / 2;
                r.phase("seek", || {
                    stream.seek(middle).unwrap();
                    let n = loop {
                        match stream.read(&mut out) {
                            Ok(n) => break n,
                            Err(e) if e.code == ErrorCode::Underrun => std::hint::spin_loop(),
                            Err(e) => panic!("{e}"),
                        }
                    };
                    (n, work(n, RATE))
                });
                r.phase("drop", || {
                    drop(stream);
                    (drop(handle), NONE)
                });
            })
        }));
    }
    cases.push(case(format!("assets/loader/{tag}"), move || {
        let path = temp_dir().join(format!("asset-{seconds}.flac"));
        let bytes = encode(&music(seconds, RATE, 2), AudioFormat::Flac).unwrap();
        std::fs::write(&path, bytes).unwrap();
        Box::new(move |r: &mut Recorder| {
            // Room for the 180 s sound, so the second load measures a cache hit.
            let config = LoaderConfig {
                cache_bytes: 256 * 1024 * 1024,
                max_input_bytes: 256 * 1024 * 1024,
                ..LoaderConfig::default()
            };
            let loader = r.phase("prepare", || (Loader::new(config).unwrap(), NONE));
            let sound = r.phase("load-miss", || {
                let s = loader.load_file(&path).unwrap().wait().unwrap();
                let w = work(s.frames(), RATE);
                (s, w)
            });
            r.hash_audio(&sound);
            let again = r.phase("load-hit", || {
                let s = loader.load_file(&path).unwrap().wait().unwrap();
                (s, NONE)
            });
            assert!(Arc::ptr_eq(&sound, &again));
            drop((sound, again));
            r.phase("drop", || (drop(loader), NONE));
        })
    }));
    cases.push(case(format!("assets/cache-churn/{tag}"), move || {
        let sounds: Vec<Arc<Audio>> = (0..64)
            .map(|i| Arc::new(music(0.25 + i as f64 * 0.01, RATE, 1)))
            .collect();
        let budget = sounds.iter().map(|s| s.frames() * 4).sum::<usize>() / 3;
        Box::new(move |r: &mut Recorder| {
            let mut hits = 0u64;
            r.phase("insert-get", || {
                let mut cache = AudioCache::new(budget);
                for round in 0..20 {
                    for (i, s) in sounds.iter().enumerate() {
                        let key = AssetKey::Name(format!("s{}", (i * 7 + round) % 64));
                        if cache.get(&key).is_some() {
                            hits += 1;
                        } else {
                            let _ = cache.insert(key, s.clone());
                        }
                    }
                }
                ((), NONE)
            });
            r.checksum = r.checksum.wrapping_add(hits);
        })
    }));
}

fn all_cases(quick: bool) -> Vec<Case> {
    let mut cases = Vec::new();
    let scales: &[(f64, &str)] = if quick {
        &[(10.0, "10s")]
    } else {
        &[(10.0, "10s"), (180.0, "180s")]
    };
    for &(seconds, tag) in scales {
        codec_cases(&mut cases, seconds, tag);
        analysis_cases(&mut cases, seconds, tag);
        synth_cases(&mut cases, seconds, tag);
        mixer_cases(
            &mut cases,
            seconds.min(60.0),
            if seconds > 10.0 { "60s" } else { tag },
        );
        #[cfg(all(
            feature = "assets",
            feature = "file-stream",
            not(target_family = "wasm")
        ))]
        stream_cases(&mut cases, seconds, tag);
    }
    cases
}

// ---- Reporting --------------------------------------------------------------------

fn command(program: &str, args: &[&str]) -> String {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn cpu() -> String {
    if cfg!(target_os = "macos") {
        return command("sysctl", &["-n", "machdep.cpu.brand_string"]);
    }
    if let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") {
        if let Some(line) = info.lines().find(|l| l.starts_with("model name")) {
            return line.split(':').nth(1).unwrap_or("").trim().to_string();
        }
    }
    std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown".into())
}

/// Resident set size of this process, in bytes, where the platform reports it without
/// dependencies: the high-water mark on Linux, the current size elsewhere. This counts
/// the whole process (fixtures, harness, allocator caches), not library buffers.
fn process_rss() -> Option<u64> {
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        return Some(kb * 1024);
    }
    if cfg!(unix) {
        let pid = std::process::id().to_string();
        let kb: u64 = command("ps", &["-o", "rss=", "-p", &pid]).parse().ok()?;
        return Some(kb * 1024);
    }
    None
}

fn provenance(runs: usize, quick: bool) -> Vec<(&'static str, String)> {
    let features = [
        ("synth", cfg!(feature = "synth")),
        ("sfx", cfg!(feature = "sfx")),
        ("mixer", cfg!(feature = "mixer")),
        ("music", cfg!(feature = "music")),
        ("events", cfg!(feature = "events")),
        ("assets", cfg!(feature = "assets")),
        ("file-stream", cfg!(feature = "file-stream")),
    ]
    .iter()
    .filter(|f| f.1)
    .map(|f| f.0)
    .collect::<Vec<_>>()
    .join("+");
    let dirty = !command("git", &["status", "--porcelain", "--untracked-files=no"]).is_empty();
    vec![
        ("crate", format!("audiobox {}", env!("CARGO_PKG_VERSION"))),
        (
            "commit",
            format!(
                "{}{}",
                command("git", &["rev-parse", "HEAD"]),
                if dirty { " (modified)" } else { "" }
            ),
        ),
        ("rustc", command("rustc", &["-V"])),
        (
            "profile",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .into(),
        ),
        ("features", features),
        (
            "target",
            format!(
                "{} {} ({})",
                std::env::consts::ARCH,
                std::env::consts::OS,
                std::env::consts::FAMILY
            ),
        ),
        ("cpu", cpu()),
        (
            "logical-cpus",
            std::thread::available_parallelism()
                .map_or(0, |n| n.get())
                .to_string(),
        ),
        (
            "runs",
            format!("1 warm-up + {runs} timed + 1 allocation-counted"),
        ),
        (
            "scale",
            if quick {
                "quick (10 s)"
            } else {
                "10 s and 180 s (mixer 60 s)"
            }
            .into(),
        ),
        (
            "unix-time",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
                .to_string(),
        ),
    ]
}

const COLUMNS: &str = "case,phase,runs,frames,rate,median_ms,min_ms,max_ms,mframes_per_s,realtime_x,allocs,reallocs,frees,requested_bytes,moved_bytes,peak_live_bytes,callbacks,p50_us,p99_us,p999_us,max_us,deadline_us,over_deadline,process_rss_bytes,checksum";

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn report(case: &str, phases: &mut [Phase], checksum: u64, rss: Option<u64>) -> Vec<String> {
    let mut rows = Vec::new();
    for p in phases.iter_mut() {
        p.ns.sort_unstable();
        p.callbacks.sort_unstable();
        let median = percentile(&p.ns, 0.5) as f64 / 1e6;
        let min = p.ns.first().copied().unwrap_or(0) as f64 / 1e6;
        let max = p.ns.last().copied().unwrap_or(0) as f64 / 1e6;
        let seconds = median / 1e3;
        let mfps = if seconds > 0.0 && p.work.frames > 0 {
            p.work.frames as f64 / seconds / 1e6
        } else {
            0.0
        };
        let rtf = if seconds > 0.0 && p.work.rate > 0 {
            p.work.frames as f64 / p.work.rate as f64 / seconds
        } else {
            0.0
        };
        let a = p.allocs.unwrap_or_default();
        let c = &p.callbacks;
        let us = |ns: u64| ns as f64 / 1e3;
        let (deadline, over) = if !c.is_empty() && p.deadline_ns > 0 {
            (
                format!("{:.1}", us(p.deadline_ns)),
                c.iter().filter(|&&t| t > p.deadline_ns).count().to_string(),
            )
        } else if c.is_empty() && p.deadline_ns > 0 {
            // A sustained stream phase stores its underrun retries here.
            (String::new(), format!("underruns={}", p.deadline_ns))
        } else {
            (String::new(), String::new())
        };
        rows.push(format!(
            "{case},{},{},{},{},{median:.4},{min:.4},{max:.4},{mfps:.3},{rtf:.1},{},{},{},{},{},{},{},{},{},{},{},{deadline},{over},{},{checksum:016x}",
            p.name,
            p.ns.len(),
            p.work.frames,
            p.work.rate,
            a.allocs,
            a.reallocs,
            a.frees,
            a.requested,
            a.moved,
            a.peak,
            c.len(),
            if c.is_empty() { String::new() } else { format!("{:.2}", us(percentile(c, 0.5))) },
            if c.is_empty() { String::new() } else { format!("{:.2}", us(percentile(c, 0.99))) },
            if c.is_empty() { String::new() } else { format!("{:.2}", us(percentile(c, 0.999))) },
            if c.is_empty() { String::new() } else { format!("{:.2}", us(*c.last().unwrap())) },
            rss.map_or(String::new(), |r| r.to_string()),
        ));
    }
    rows
}

fn summary(row: &str) {
    let f: Vec<&str> = row.split(',').collect();
    let mut line = format!("{:<38} {:<12} {:>10} ms", f[0], f[1], f[5]);
    if f[9] != "0.0" {
        line += &format!(" {:>9}x rt", f[9]);
    }
    line += &format!("  allocs {:>6}  peak {:>11} B", f[10], f[15]);
    if !f[17].is_empty() {
        line += &format!("  cb p50 {} p99 {} max {} us", f[17], f[18], f[20]);
        if !f[21].is_empty() {
            line += &format!(" (deadline {} us, {} over)", f[21], f[22]);
        }
    } else if !f[22].is_empty() {
        line += &format!("  {}", f[22]);
    }
    println!("{line}");
}

fn read_csv(path: &str) -> BTreeMap<(String, String), Vec<String>> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("case,"))
        .map(|l| {
            let f: Vec<String> = l.split(',').map(String::from).collect();
            ((f[0].clone(), f[1].clone()), f)
        })
        .collect()
}

fn compare(base: &str, new: &str) {
    let (a, b) = (read_csv(base), read_csv(new));
    println!(
        "{:<38} {:<12} {:>11} {:>11} {:>8} {:>16} {:>9}",
        "case", "phase", "base ms", "new ms", "new/base", "allocs base>new", "output"
    );
    for (key, old) in &a {
        let Some(now) = b.get(key) else { continue };
        let (x, y): (f64, f64) = (old[5].parse().unwrap_or(0.0), now[5].parse().unwrap_or(0.0));
        let ratio = if x > 0.0 {
            format!("{:.3}", y / x)
        } else {
            "-".into()
        };
        let same = if old[24] == now[24] {
            "same"
        } else {
            "CHANGED"
        };
        println!(
            "{:<38} {:<12} {:>11} {:>11} {:>8} {:>16} {:>9}",
            key.0,
            key.1,
            old[5],
            now[5],
            ratio,
            format!("{}>{}", old[10], now[10]),
            same
        );
    }
}

fn main() {
    let mut args = std::env::args().skip(1).filter(|a| a != "--bench");
    let (mut filters, mut skips) = (Vec::new(), Vec::new());
    let (mut runs, mut quick, mut list, mut csv) = (5usize, false, false, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--quick" => quick = true,
            "--list" => list = true,
            "--runs" => runs = args.next().and_then(|n| n.parse().ok()).expect("--runs N"),
            "--csv" => csv = Some(args.next().expect("--csv PATH")),
            "--skip" => skips.push(args.next().expect("--skip PATTERN")),
            "--compare" => {
                let base = args.next().expect("--compare BASE NEW");
                let new = args.next().expect("--compare BASE NEW");
                return compare(&base, &new);
            }
            _ => filters.push(arg),
        }
    }
    let cases: Vec<Case> = all_cases(quick)
        .into_iter()
        .filter(|c| filters.is_empty() || filters.iter().any(|f| c.name.contains(f.as_str())))
        .filter(|c| !skips.iter().any(|f| c.name.contains(f.as_str())))
        .collect();
    if list {
        for c in &cases {
            println!("{}", c.name);
        }
        return;
    }
    let mut lines: Vec<String> = provenance(runs, quick)
        .into_iter()
        .map(|(k, v)| format!("# {k}: {v}"))
        .collect();
    for l in &lines {
        println!("{l}");
    }
    lines.push(COLUMNS.into());
    for c in &cases {
        let mut run = (c.setup)();
        let mut warm = Recorder {
            tracked: false,
            phases: Vec::new(),
            checksum: 0xcbf2_9ce4_8422_2325,
        };
        run(&mut warm);
        let mut recorder = Recorder {
            phases: Vec::new(),
            ..warm
        };
        for _ in 0..runs {
            recorder.checksum = 0xcbf2_9ce4_8422_2325;
            run(&mut recorder);
            // Every repetition must produce the warm-up's output.
            assert_eq!(
                recorder.checksum, warm.checksum,
                "{}: output changed between runs",
                c.name
            );
        }
        recorder.tracked = true;
        recorder.checksum = 0xcbf2_9ce4_8422_2325;
        run(&mut recorder);
        drop(run);
        let checksum = recorder.checksum;
        for row in report(&c.name, &mut recorder.phases, checksum, process_rss()) {
            summary(&row);
            lines.push(row);
        }
        if let Some(path) = &csv {
            std::fs::write(path, lines.join("\n") + "\n").unwrap();
        }
    }
    #[cfg(all(
        feature = "assets",
        feature = "file-stream",
        not(target_family = "wasm")
    ))]
    let _ = std::fs::remove_dir_all(temp_dir());
}
