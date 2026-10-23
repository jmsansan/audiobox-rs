use super::{AssetKey, AudioCache};
use crate::{Audio, DecodeOptions, Error, ErrorCode, Result, decode_with_options};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};

/// Bounds shared by every request to a [`Loader`].
#[derive(Clone, Debug)]
pub struct LoaderConfig {
    /// Retained cache-data budget in bytes. Default 64 MiB; zero disables caching.
    pub cache_bytes: usize,
    /// Requests waiting behind the single active decode. Default 16; must be > 0.
    pub queue_capacity: usize,
    /// Encoded bytes per request, checked before reading an entire file. Default 64 MiB.
    /// Decoder buffers/output are separately bounded by `decode.limits`.
    pub max_input_bytes: usize,
    /// Fixed for this loader so cached data cannot bypass per-request decode limits.
    pub decode: DecodeOptions,
}

impl Default for LoaderConfig {
    fn default() -> Self {
        Self {
            cache_bytes: 64 * 1024 * 1024,
            queue_capacity: 16,
            max_input_bytes: 64 * 1024 * 1024,
            decode: DecodeOptions::default(),
        }
    }
}

/// A one-shot result of a queued file read/decode. Dropping it does not cancel work.
/// A completed result pins its cached sound until taken or dropped.
pub struct LoadHandle {
    receiver: Receiver<Result<Arc<Audio>>>,
    taken: bool,
}

impl LoadHandle {
    /// Take the result once, without waiting. `None` means pending or already taken.
    /// Worker disconnection is delivered once as `ErrorCode::Io`.
    /// This uses std channels and is not an audio-callback API.
    pub fn try_take(&mut self) -> Option<Result<Arc<Audio>>> {
        if self.taken {
            return None;
        }
        let result = match self.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(disconnected()),
        };
        self.taken = true;
        Some(result)
    }

    /// Wait for this request on a loading/control thread. After `try_take` consumed
    /// the result this returns `InvalidArgument`.
    pub fn wait(self) -> Result<Arc<Audio>> {
        if self.taken {
            return Err(Error::invalid("Load result already taken"));
        }
        self.receiver.recv().map_err(|_| disconnected())?
    }
}

enum Input {
    Bytes(Vec<u8>),
    File(PathBuf),
}

struct Request {
    key: AssetKey,
    input: Input,
    result: SyncSender<Result<Arc<Audio>>>,
}

/// One background worker with a bounded request queue and a private [`AudioCache`].
///
/// Submission never waits for queue space, but may allocate/free memory and uses
/// std channels. All methods, polling and destruction belong off the audio thread.
/// Dropping the loader closes its queue and **waits** for accepted requests to finish;
/// their handles remain usable. A dropped handle does not cancel its decode/cache
/// admission. No async runtime or mixer feature is required.
pub struct Loader {
    sender: Option<SyncSender<Request>>,
    worker: Option<JoinHandle<()>>,
    max_input_bytes: usize,
}

impl Loader {
    /// Spawn the worker. Thread creation failures return `Io`.
    pub fn new(config: LoaderConfig) -> Result<Self> {
        if config.queue_capacity == 0 {
            return Err(Error::invalid("Loader queue capacity must be positive"));
        }
        let (sender, receiver) = mpsc::sync_channel(config.queue_capacity);
        let max_input_bytes = config.max_input_bytes;
        let worker = thread::Builder::new()
            .name("audiobox-loader".into())
            .spawn(move || run(receiver, config))?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            max_input_bytes,
        })
    }

    /// Queue owned encoded bytes under a name. Reusing a cached name returns its
    /// original sound. Input size is checked even on cache hits; bytes are not copied.
    pub fn load_bytes(&self, name: impl Into<String>, bytes: Vec<u8>) -> Result<LoadHandle> {
        if bytes.len() > self.max_input_bytes {
            return Err(Error::limit("Encoded input exceeds the loader limit"));
        }
        self.submit(AssetKey::Name(name.into()), Input::Bytes(bytes))
    }

    /// Queue a file by its absolute path. Resolving a relative path uses the current
    /// directory now; file I/O and size/decode errors arrive through the handle.
    pub fn load_file(&self, path: impl AsRef<Path>) -> Result<LoadHandle> {
        let path = path.as_ref();
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            std::path::absolute(path)?
        };
        self.submit(AssetKey::Path(path.clone()), Input::File(path))
    }

    fn submit(&self, key: AssetKey, input: Input) -> Result<LoadHandle> {
        let (result, receiver) = mpsc::sync_channel(1);
        let request = Request { key, input, result };
        match self.sender.as_ref().unwrap().try_send(request) {
            Ok(()) => Ok(LoadHandle {
                receiver,
                taken: false,
            }),
            Err(TrySendError::Full(_)) => Err(Error::limit("Loader request queue is full")),
            Err(TrySendError::Disconnected(_)) => Err(disconnected()),
        }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn disconnected() -> Error {
    Error::new(ErrorCode::Io, "Audio loader worker disconnected")
}

fn run(receiver: Receiver<Request>, config: LoaderConfig) {
    let mut cache = AudioCache::new(config.cache_bytes);
    for request in receiver {
        let result = if let Some(audio) = cache.get(&request.key) {
            Ok(audio)
        } else {
            load(request.input, &config).map(|audio| {
                let audio = Arc::new(audio);
                // Retention is optional: full/pinned caches must not fail a load.
                let _ = cache.insert(request.key, Arc::clone(&audio));
                audio
            })
        };
        // One result slot, initially empty: sending never waits for the caller.
        let _ = request.result.send(result);
    }
}

fn load(input: Input, config: &LoaderConfig) -> Result<Audio> {
    let bytes = match input {
        Input::Bytes(bytes) => bytes,
        Input::File(path) => {
            let file = File::open(path)?;
            if file.metadata()?.len() > config.max_input_bytes as u64 {
                return Err(Error::limit("Encoded file exceeds the loader limit"));
            }
            let mut bytes = Vec::new();
            // The extra byte catches a file that grows after the metadata check.
            file.take((config.max_input_bytes as u64).saturating_add(1))
                .read_to_end(&mut bytes)?;
            if bytes.len() > config.max_input_bytes {
                return Err(Error::limit("Encoded file exceeds the loader limit"));
            }
            bytes
        }
    };
    decode_with_options(&bytes, &config.decode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_queue_and_disconnected_worker_report_errors() {
        // No worker consumes this queue, making saturation independent of timing.
        let (sender, receiver) = mpsc::sync_channel(1);
        let loader = Loader {
            sender: Some(sender),
            worker: None,
            max_input_bytes: 16,
        };
        let mut first = loader.load_bytes("first", vec![]).unwrap();
        assert!(first.try_take().is_none());
        assert_eq!(
            loader.load_bytes("full", vec![]).err().unwrap().code,
            ErrorCode::LimitExceeded
        );
        drop(receiver);
        assert_eq!(first.try_take().unwrap().unwrap_err().code, ErrorCode::Io);
        assert!(first.try_take().is_none());
        assert_eq!(first.wait().unwrap_err().code, ErrorCode::InvalidArgument);
        assert_eq!(
            loader.load_bytes("closed", vec![]).err().unwrap().code,
            ErrorCode::Io
        );
    }
}
