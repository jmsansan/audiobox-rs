//! Background loading and a cache of decoded sounds. Requires `assets`.
//!
//! [`AudioCache`] retains shared decoded sounds within a byte budget. Accessing a
//! sound makes it most recently used. Admission evicts the least recently used
//! sounds whose only reference is the cache's; a caller, pending load result
//! or mixer voice holding an [`Arc<Audio>`] pins that entry. Failed admission leaves
//! existing entries intact. Sharing one `Arc` under multiple keys pins those entries
//! conservatively until the other references disappear. Weak references also pin an
//! entry, so a concurrent weak upgrade cannot race eviction.
//!
//! The budget accounts for sample and channel vector capacities, the `Audio` and
//! entry structs, and key/metadata string storage. It excludes allocator bookkeeping,
//! collection spare capacity and metadata tree nodes. It is a retained-data budget,
//! not a bound on process memory: encoded inputs, decoder scratch space and sounds
//! held outside the cache have separate lifetimes.
//!
//! On native targets, `Loader` owns one worker and one cache. File reads and
//! decoding happen on that worker. Requests use a bounded queue; queue saturation
//! returns [`ErrorCode::LimitExceeded`](crate::ErrorCode::LimitExceeded). A successful
//! decode is returned even when it cannot be cached. Keys identify immutable assets:
//! a cache hit ignores newly supplied bytes or changes to a file. Use a new key or
//! a new loader when content changes. Relative paths are made absolute at submission
//! using [`std::path::absolute`], without resolving symlinks. Paths that remain
//! differently spelled after that conversion are distinct keys.
//!
//! All cache and loader operations belong on a game's control/loading thread, never
//! in the audio callback. Pass the resulting `Arc<Audio>` to the mixer. The cache
//! works on WebAssembly; the thread-based loader is only available on native targets.
//!
//! ```
//! # #[cfg(not(target_family = "wasm"))]
//! # fn main() -> audiobox::Result<()> {
//! use audiobox::assets::{Loader, LoaderConfig};
//! # use audiobox::{Audio, AudioFormat, encode};
//! # let bytes = encode(&Audio::new(vec![vec![0.25; 128]], 48_000)?, AudioFormat::Wav)?;
//! let loader = Loader::new(LoaderConfig::default())?;
//! let mut loading = loader.load_bytes("jump", bytes)?;
//! // Once per game frame, on the control thread:
//! if let Some(result) = loading.try_take() {
//!     let sound = result?; // Arc<Audio>, ready for MixerHandle::play.
//! }
//! # Ok(())
//! # }
//! # #[cfg(target_family = "wasm")]
//! # fn main() {}
//! ```
use crate::{Audio, Error, Result};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(not(target_family = "wasm"))]
mod loader;
#[cfg(not(target_family = "wasm"))]
pub use loader::{LoadHandle, Loader, LoaderConfig};

/// Names and file paths occupy separate namespaces. Paths are compared literally.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssetKey {
    Name(String),
    Path(PathBuf),
}

impl AssetKey {
    fn bytes(&self) -> usize {
        match self {
            Self::Name(name) => name.capacity(),
            Self::Path(path) => path.capacity(),
        }
    }
}

struct Entry {
    key: AssetKey,
    audio: Arc<Audio>,
    bytes: usize,
}

/// A least-recently-used cache that skips entries with external references.
/// See the module documentation for budget accounting and thread responsibilities.
pub struct AudioCache {
    entries: VecDeque<Entry>, // Oldest first; no wrapping timestamp counter.
    budget: usize,
    used: usize,
}

impl AudioCache {
    /// An empty cache with a retained-data budget in bytes. Zero disables admission.
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            budget: budget_bytes,
            used: 0,
        }
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget
    }

    /// Accounted bytes of retained entries, including those pinned by callers.
    pub fn used_bytes(&self) -> usize {
        self.used
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get a shared sound and make its entry most recently used.
    pub fn get(&mut self, key: &AssetKey) -> Option<Arc<Audio>> {
        let at = self.entries.iter().position(|entry| &entry.key == key)?;
        let entry = self.entries.remove(at).unwrap();
        let audio = Arc::clone(&entry.audio);
        self.entries.push_back(entry);
        Some(audio)
    }

    /// Admit a sound, evicting unused entries in LRU order as needed.
    ///
    /// Returns `LimitExceeded` without changing entries when the sound is too large,
    /// all sufficient victims are pinned, or replacing a different pinned sound.
    /// Inserting the same `Arc` at its existing key only refreshes recency.
    pub fn insert(&mut self, key: AssetKey, audio: Arc<Audio>) -> Result<()> {
        let existing = self.entries.iter().position(|entry| entry.key == key);
        if let Some(at) = existing {
            let entry = &mut self.entries[at];
            if Arc::ptr_eq(&entry.audio, &audio) {
                let entry = self.entries.remove(at).unwrap();
                self.entries.push_back(entry);
                return Ok(());
            }
            if Arc::get_mut(&mut entry.audio).is_none() {
                return Err(Error::limit("Cannot replace a sound still in use"));
            }
        }
        let bytes = entry_bytes(&key, &audio)?;
        if bytes > self.budget {
            return Err(Error::limit("Sound exceeds the cache budget"));
        }
        let old_bytes = existing.map_or(0, |at| self.entries[at].bytes);
        let retained = self.used - old_bytes;
        let needed = retained.saturating_sub(self.budget - bytes);
        let available: usize = self
            .entries
            .iter_mut()
            .enumerate()
            .filter_map(|(at, entry)| {
                (Some(at) != existing && Arc::get_mut(&mut entry.audio).is_some())
                    .then_some(entry.bytes)
            })
            .sum();
        if available < needed {
            return Err(Error::limit("Cache budget is pinned by sounds in use"));
        }
        if let Some(at) = existing {
            self.entries.remove(at);
            self.used -= old_bytes;
        }
        while self.used > self.budget - bytes {
            let at = self
                .entries
                .iter_mut()
                .position(|entry| Arc::get_mut(&mut entry.audio).is_some())
                .unwrap();
            self.used -= self.entries.remove(at).unwrap().bytes;
        }
        self.entries.push_back(Entry { key, audio, bytes });
        self.used += bytes;
        Ok(())
    }
}

fn entry_bytes(key: &AssetKey, audio: &Audio) -> Result<usize> {
    let mut bytes = size_of::<Entry>() + size_of::<Audio>();
    let mut add = |n: usize| -> Result<()> {
        bytes = bytes
            .checked_add(n)
            .ok_or_else(|| Error::limit("Cache size overflow"))?;
        Ok(())
    };
    add(key.bytes())?;
    add(audio.data.capacity() * size_of::<Vec<f32>>())?;
    for channel in &audio.data {
        add(channel.capacity() * size_of::<f32>())?;
    }
    for (key, value) in &audio.info.metadata {
        add(size_of::<(String, String)>())?;
        add(key.capacity())?;
        add(value.capacity())?;
    }
    for (_, value) in audio.info.tags.fields() {
        if let Some(value) = value {
            add(value.capacity())?;
        }
    }
    if let Some(cover) = &audio.info.tags.cover {
        add(cover.mime_type.capacity())?;
        add(cover.data.capacity())?;
    }
    Ok(bytes)
}
