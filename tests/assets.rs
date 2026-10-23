#![cfg(feature = "assets")]
use audiobox::assets::{AssetKey, AudioCache};
use audiobox::{Audio, ErrorCode};
use std::sync::Arc;

fn key(name: &str) -> AssetKey {
    AssetKey::Name(name.into())
}

fn sound() -> Arc<Audio> {
    Arc::new(Audio::new(vec![vec![0.25; 128]], 48_000).unwrap())
}

fn entry_size() -> usize {
    let mut cache = AudioCache::new(usize::MAX);
    cache.insert(key("a"), sound()).unwrap();
    cache.used_bytes()
}

#[test]
fn cache_budget_and_lru_follow_reads() {
    let size = entry_size();
    let mut cache = AudioCache::new(2 * size);
    assert_eq!(cache.budget_bytes(), 2 * size);
    assert!(cache.is_empty());
    cache.insert(key("a"), sound()).unwrap();
    cache.insert(key("b"), sound()).unwrap();
    drop(cache.get(&key("a")).unwrap()); // b is now the oldest.
    cache.insert(key("c"), sound()).unwrap();
    assert!(cache.get(&key("b")).is_none());
    assert!(cache.get(&key("a")).is_some());
    assert!(cache.get(&key("c")).is_some());
    assert_eq!(cache.len(), 2);
    assert_eq!(cache.used_bytes(), 2 * size);
}

#[test]
fn eviction_skips_used_sounds_then_reclaims_them() {
    let size = entry_size();
    let mut cache = AudioCache::new(2 * size);
    let playing = sound();
    cache.insert(key("a"), playing.clone()).unwrap();
    cache.insert(key("b"), sound()).unwrap();
    cache.insert(key("c"), sound()).unwrap();
    assert!(cache.get(&key("b")).is_none());
    assert!(Arc::ptr_eq(&playing, &cache.get(&key("a")).unwrap()));
    assert_eq!(playing.channel_data(0).unwrap(), &[0.25; 128]);
    // Touch c, then release a: it becomes the oldest evictable entry.
    drop(cache.get(&key("c")));
    drop(playing);
    cache.insert(key("d"), sound()).unwrap();
    assert!(cache.get(&key("a")).is_none());
    assert_eq!(cache.used_bytes(), 2 * size);
}

#[test]
fn failed_admission_and_pinned_replacement_preserve_the_cache() {
    let size = entry_size();
    let mut cache = AudioCache::new(2 * size);
    let playing = sound();
    cache.insert(key("a"), sound()).unwrap();
    cache.insert(key("b"), playing.clone()).unwrap();
    // Needs more than the one unused victim; do not partially evict a.
    let larger = Arc::new(Audio::new(vec![vec![0.0; 256]], 48_000).unwrap());
    assert_eq!(
        cache.insert(key("c"), larger).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    assert!(cache.get(&key("a")).is_some());
    assert_eq!(cache.used_bytes(), 2 * size);
    assert_eq!(
        cache.insert(key("b"), sound()).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    cache.insert(key("b"), playing.clone()).unwrap();
    assert_eq!(cache.used_bytes(), 2 * size);
    assert!(Arc::ptr_eq(&playing, &cache.get(&key("b")).unwrap()));
    drop(playing);
    let replacement = sound();
    cache.insert(key("b"), replacement.clone()).unwrap();
    assert!(Arc::ptr_eq(&replacement, &cache.get(&key("b")).unwrap()));
    assert_eq!(cache.len(), 2);
}

#[test]
fn cache_accounts_for_retained_capacities_keys_and_metadata() {
    let size = entry_size();
    let mut samples = Vec::with_capacity(4096);
    samples.resize(128, 0.25);
    let audio = Arc::new(Audio::new(vec![samples], 48_000).unwrap());
    let mut cache = AudioCache::new(size);
    assert_eq!(
        cache.insert(key("a"), audio).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    let huge_name = "a".repeat(size);
    assert_eq!(
        cache.insert(key(&huge_name), sound()).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    let bytes = audiobox::encode_with_options(
        &sound(),
        audiobox::AudioFormat::Wav,
        &audiobox::EncodeOptions {
            metadata: [("comment".into(), "x".repeat(size))].into(),
            ..Default::default()
        },
    )
    .unwrap();
    let tagged = Arc::new(audiobox::decode(&bytes).unwrap());
    assert_eq!(
        cache.insert(key("a"), tagged).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    assert!(cache.is_empty());
    let mut disabled = AudioCache::new(0);
    let empty = Arc::new(Audio::new(vec![vec![]], 48_000).unwrap());
    assert_eq!(
        disabled.insert(key("a"), empty).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(disabled.used_bytes(), 0);
}

#[test]
fn weak_references_pin_entries_until_they_are_dropped() {
    let mut cache = AudioCache::new(entry_size());
    let audio = sound();
    let weak = Arc::downgrade(&audio);
    cache.insert(key("a"), audio).unwrap();
    assert_eq!(
        cache.insert(key("b"), sound()).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(weak.upgrade().unwrap().frames(), 128);
    drop(weak);
    cache.insert(key("b"), sound()).unwrap();
    assert!(cache.get(&key("a")).is_none());
}

#[cfg(not(target_family = "wasm"))]
mod native {
    use super::*;
    use audiobox::assets::{Loader, LoaderConfig};
    use audiobox::{AudioFormat, DecodeOptions, Limits, decode_with_options, encode};

    #[test]
    fn background_decode_matches_whole_decode_for_supported_formats() {
        let audio = Audio::new(vec![vec![0.25; 2304], vec![-0.125; 2304]], 48_000).unwrap();
        let loader = Loader::new(LoaderConfig::default()).unwrap();
        for format in [
            AudioFormat::Wav,
            AudioFormat::Aiff,
            AudioFormat::Caf,
            AudioFormat::Au,
            AudioFormat::Flac,
            AudioFormat::Mp3,
        ] {
            let bytes = encode(&audio, format).unwrap();
            let expected = decode_with_options(&bytes, &DecodeOptions::default()).unwrap();
            let actual = loader
                .load_bytes(format!("{format:?}"), bytes)
                .unwrap()
                .wait()
                .unwrap();
            assert_eq!(actual.sample_rate(), expected.sample_rate());
            assert_eq!(actual.all_channels(), expected.all_channels());
            assert_eq!(actual.info().format, expected.info().format);
            assert_eq!(actual.info().metadata, expected.info().metadata);
        }
    }

    #[test]
    fn cached_loads_share_audio_and_full_or_disabled_cache_still_loads() {
        let bytes = encode(&sound(), AudioFormat::Wav).unwrap();
        for budget in [0, entry_size(), usize::MAX] {
            let loader = Loader::new(LoaderConfig {
                cache_bytes: budget,
                ..Default::default()
            })
            .unwrap();
            let a = loader
                .load_bytes("a", bytes.clone())
                .unwrap()
                .wait()
                .unwrap();
            let b = loader
                .load_bytes("b", bytes.clone())
                .unwrap()
                .wait()
                .unwrap();
            let again = loader
                .load_bytes("a", bytes.clone())
                .unwrap()
                .wait()
                .unwrap();
            assert_eq!(a.all_channels(), b.all_channels());
            assert_eq!(Arc::ptr_eq(&a, &again), budget > 0);
        }
    }

    #[test]
    fn pending_results_pin_cache_entries_and_drop_drains_requests() {
        let bytes = encode(&sound(), AudioFormat::Wav).unwrap();
        let loader = Loader::new(LoaderConfig {
            cache_bytes: entry_size(),
            ..Default::default()
        })
        .unwrap();
        let a = loader.load_bytes("a", bytes.clone()).unwrap();
        let b = loader.load_bytes("b", bytes.clone()).unwrap();
        let again = loader.load_bytes("a", bytes).unwrap();
        // Join drains all accepted jobs without needing consumers to receive first.
        drop(loader);
        let a = a.wait().unwrap();
        let again = again.wait().unwrap();
        assert!(Arc::ptr_eq(&a, &again));
        assert_eq!(b.wait().unwrap().frames(), 128);
    }

    #[test]
    fn errors_reach_handles_and_input_and_decode_limits_apply() {
        let loader = Loader::new(LoaderConfig::default()).unwrap();
        assert_eq!(
            loader
                .load_bytes("bad", b"invalid".to_vec())
                .unwrap()
                .wait()
                .unwrap_err()
                .code,
            ErrorCode::UnsupportedFormat
        );
        assert_eq!(
            loader
                .load_file("tests/fixtures/does-not-exist.wav")
                .unwrap()
                .wait()
                .unwrap_err()
                .code,
            ErrorCode::Io
        );
        let limited = Loader::new(LoaderConfig {
            max_input_bytes: 16,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            limited.load_bytes("big", vec![0; 17]).err().unwrap().code,
            ErrorCode::LimitExceeded
        );
        assert_eq!(
            limited
                .load_file("tests/fixtures/typescript.wav")
                .unwrap()
                .wait()
                .unwrap_err()
                .code,
            ErrorCode::LimitExceeded
        );
        let limited = Loader::new(LoaderConfig {
            decode: DecodeOptions {
                limits: Limits {
                    max_frames: 1,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            limited
                .load_bytes("large", encode(&sound(), AudioFormat::Wav).unwrap())
                .unwrap()
                .wait()
                .unwrap_err()
                .code,
            ErrorCode::LimitExceeded
        );
        assert_eq!(
            Loader::new(LoaderConfig {
                queue_capacity: 0,
                ..Default::default()
            })
            .err()
            .unwrap()
            .code,
            ErrorCode::InvalidArgument
        );
    }

    #[test]
    fn file_keys_are_absolute_and_distinct_from_names_and_polling_is_one_shot() {
        let path = std::path::absolute("tests/fixtures/typescript.wav").unwrap();
        let expected = audiobox::decode(&std::fs::read(&path).unwrap()).unwrap();
        let loader = Loader::new(LoaderConfig::default()).unwrap();
        let relative = loader
            .load_file("tests/fixtures/typescript.wav")
            .unwrap()
            .wait()
            .unwrap();
        let mut absolute = loader.load_file(&path).unwrap();
        let named = loader
            .load_bytes(
                path.to_string_lossy(),
                encode(&sound(), AudioFormat::Wav).unwrap(),
            )
            .unwrap();
        // Draining provides a deterministic completed handle for the poll API.
        drop(loader);
        let absolute_audio = absolute.try_take().unwrap().unwrap();
        assert!(absolute.try_take().is_none());
        assert!(Arc::ptr_eq(&relative, &absolute_audio));
        assert_eq!(relative.all_channels(), expected.all_channels());
        assert_eq!(named.wait().unwrap().frames(), 128);
    }

    #[cfg(feature = "mixer")]
    #[test]
    fn mixer_voice_pins_a_cached_sound_through_playback() {
        use audiobox::mixer::{Mixer, MixerConfig, Play};
        let mut cache = AudioCache::new(entry_size());
        cache.insert(key("a"), sound()).unwrap();
        let (mut mixer, mut handle) = Mixer::new(MixerConfig {
            channels: 1,
            ..Default::default()
        })
        .unwrap();
        handle
            .play(cache.get(&key("a")).unwrap(), &Play::default())
            .unwrap();
        assert_eq!(
            cache.insert(key("b"), sound()).unwrap_err().code,
            ErrorCode::LimitExceeded
        );
        let mut out = [0.0; 256];
        mixer.render(&mut out);
        assert_eq!(&out[..128], &[0.25; 128]);
        while handle.poll_event().is_some() {}
        cache.insert(key("b"), sound()).unwrap();
        assert!(cache.get(&key("a")).is_none());
    }
}
