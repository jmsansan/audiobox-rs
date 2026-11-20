//! Prepared voices render, reset, seek, end and answer marker queries without
//! allocating or freeing, so they can run on an audio thread.
#![cfg(feature = "voice")]
use audiobox::voice::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}
fn counting() -> bool {
    COUNTING.try_with(Cell::get).unwrap_or(false)
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if counting() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if counting() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if counting() {
            FREES.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if counting() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;

fn guarded(f: impl FnOnce()) -> (usize, usize) {
    let (a, b) = (
        ALLOCATIONS.load(Ordering::SeqCst),
        FREES.load(Ordering::SeqCst),
    );
    COUNTING.with(|c| c.set(true));
    f();
    COUNTING.with(|c| c.set(false));
    (
        ALLOCATIONS.load(Ordering::SeqCst) - a,
        FREES.load(Ordering::SeqCst) - b,
    )
}

#[test]
fn prepared_rendering_allocates_and_frees_nothing() {
    let mut voices = Vec::new();
    for preset in VoicePreset::ALL {
        let profile = VoiceProfile::preset(preset);
        let plan = plan_gibberish(
            "¿Qué tal, viajero? ¡Vamos!",
            &profile,
            &GibberishOptions::default(),
        )
        .unwrap();
        for channels in [1, 2] {
            let options = VoiceOptions {
                channels,
                seed: 9,
                ..VoiceOptions::default()
            };
            voices.push(PreparedUtterance::new(&profile, &plan, &options).unwrap());
        }
    }
    let notes = [
        VocalNote::new(0, 4000, 220.0, Vowel::A).with_onset(Onset::Plosive),
        VocalNote::new(4000, 4000, 330.0, Vowel::E).legato(),
        VocalNote::new(9000, 4000, 262.0, Vowel::U),
    ];
    voices.push(
        PreparedUtterance::from_notes(&VoiceProfile::default(), &notes, &VoiceOptions::default())
            .unwrap(),
    );
    let mut buf = vec![0.0f32; 4096];
    for voice in &voices {
        let (allocs, frees) = guarded(|| {
            let mut synth = voice.synth();
            let mut found = 0usize;
            for size in [1usize, 7, 128, 2048].iter().cycle() {
                let n = synth.fill(&mut buf[..size * voice.channels()]);
                let at = synth.position();
                found += voice.marker_at(MarkerKind::Syllable, at).is_some() as usize;
                found += synth.markers().len();
                if n == 0 {
                    break;
                }
            }
            // End of stream, reset, cheap and replaying seeks, and drop.
            assert_eq!(synth.fill(&mut buf), 0);
            synth.reset();
            synth.fill(&mut buf[..64]);
            synth.seek(0).unwrap();
            synth.seek(1234).unwrap();
            synth.seek(u64::MAX).unwrap();
            synth.seek(0).unwrap();
            synth.fill(&mut buf);
            assert!(synth.peak_before_guard() > 0.0);
            assert!(found > 0 || voice.markers().is_empty());
            drop(synth);
        });
        assert_eq!((allocs, frees), (0, 0));
    }
}

#[cfg(feature = "mixer")]
#[test]
fn mixer_streaming_allocates_and_frees_nothing() {
    use audiobox::mixer::{Fade, Mixer, MixerConfig, Play};
    let (mut mixer, mut handle) = Mixer::new(MixerConfig::default()).unwrap();
    let profile = VoiceProfile::preset(VoicePreset::Robot);
    let plan = plan_gibberish("Bip bop.", &profile, &GibberishOptions::default()).unwrap();
    let voice = PreparedUtterance::new(&profile, &plan, &VoiceOptions::default()).unwrap();
    let first = handle.play(&voice, &Play::default()).unwrap();
    let looping = Play {
        looping: audiobox::mixer::Loop::Whole,
        ..Play::default()
    };
    handle.play(voice.synth(), &looping).unwrap();
    let mut out = vec![0.0f32; 512 * 2];
    let frames = voice.frames() as usize;
    // The mixer holds the last owners; finished voices go back to the handle.
    drop(voice);
    let (allocs, frees) = guarded(|| {
        // The first voice ends; the looping one seeks back to its start.
        for _ in 0..(frames * 3 / 512 + 2) {
            mixer.render(&mut out);
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(
        handle
            .voice_status(first)
            .is_none_or(|s| s.state == audiobox::mixer::VoiceState::Finished)
    );
    // Stopping the looping voice retires it without freeing on the audio side.
    handle.stop_all(Fade::NONE).unwrap();
    let stopped = guarded(|| {
        for _ in 0..8 {
            mixer.render(&mut out);
        }
    });
    assert_eq!(stopped, (0, 0));
    while handle.poll_event().is_some() {}
}
