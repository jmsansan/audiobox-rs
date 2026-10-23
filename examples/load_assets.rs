//! Load an audio file on a worker, then reuse the cached sound without decoding again.
//! Run: cargo run --example load_assets --features assets -- path/to/sound.wav
#[cfg(not(target_family = "wasm"))]
fn main() -> audiobox::Result<()> {
    use audiobox::assets::{Loader, LoaderConfig};
    use std::sync::Arc;

    let path = std::env::args_os().nth(1).ok_or_else(|| {
        audiobox::Error::new(audiobox::ErrorCode::InvalidArgument, "Pass an audio path")
    })?;
    let loader = Loader::new(LoaderConfig::default())?;
    let loading = loader.load_file(&path)?;
    // A game can poll try_take once per frame instead. Waiting belongs off the
    // audio thread, as do loader submission and destruction.
    let sound = loading.wait()?;
    println!(
        "{} frames, {} channels at {} Hz",
        sound.frames(),
        sound.channels(),
        sound.sample_rate()
    );
    let again = loader.load_file(path)?.wait()?;
    println!("Shared cached sound: {}", Arc::ptr_eq(&sound, &again));
    // Pass sound to MixerHandle::play on the game's control thread. The host
    // supplies its own audio device and calls Mixer::render in the device callback.
    Ok(())
}

#[cfg(target_family = "wasm")]
fn main() {
    eprintln!("The thread-based loader requires a native target; AudioCache supports WASM.");
}
