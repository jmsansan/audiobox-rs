//! Native disk reader. Only the worker owns the file and decoder.
use super::DecoderOptions;
use crate::decoder::file::FileDecoder;
use crate::{AudioFormat, Error, ErrorCode, Result};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Resident and input admission limits for a native disk stream.
#[derive(Clone, Debug)]
pub struct FileStreamOptions {
    pub decoder: DecoderOptions,
    /// PCM ring capacity, 1..=1048576 frames. Default 16384.
    pub buffer_frames: usize,
    /// Encoded sliding window, 4096..=16777216 bytes. Default 1 MiB.
    /// FLAC frames must fit; STREAMINFO's nonzero maximum is checked on opening.
    pub encoded_buffer_bytes: usize,
    /// Conservative codec scratch budget, independent of duration. Default 16 MiB.
    /// Admission includes a fixed 128 KiB allowance plus 20 bytes per block sample.
    pub max_decoder_bytes: usize,
    /// Ring samples, generation tags and worker output scratch. Default 32 MiB.
    pub max_pcm_bytes: usize,
    /// Maximum RIFF chunks / FLAC metadata blocks, default 4096.
    pub max_header_chunks: usize,
    /// Encoded file admission limit, default 8 GiB. Files must fit usize too.
    pub max_file_bytes: u64,
}
impl Default for FileStreamOptions {
    fn default() -> Self {
        Self {
            decoder: DecoderOptions::default(),
            buffer_frames: 16384,
            encoded_buffer_bytes: 1024 * 1024,
            max_decoder_bytes: 16 * 1024 * 1024,
            max_pcm_bytes: 32 * 1024 * 1024,
            max_header_chunks: 4096,
            max_file_bytes: 8 * 1024 * 1024 * 1024,
        }
    }
}
impl FileStreamOptions {
    fn validate(&self) -> Result<()> {
        if !(1..=1_048_576).contains(&self.buffer_frames)
            || !(4096..=16 * 1024 * 1024).contains(&self.encoded_buffer_bytes)
            || !(1..=65536).contains(&self.decoder.chunk_frames)
            || self.max_header_chunks == 0
            || self.max_header_chunks > 65536
            || self.max_decoder_bytes == 0
            || self.max_decoder_bytes > 256 * 1024 * 1024
            || self.max_pcm_bytes == 0
            || self.max_pcm_bytes > 256 * 1024 * 1024
        {
            return Err(Error::invalid(
                "Invalid file-stream buffer or header limits",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct Info {
    channels: usize,
    rate: u32,
    frames: Option<u64>,
    format: AudioFormat,
}
/// Snapshot of the reader. Frame positions refer to source audio, not device time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileStreamStatus {
    /// Requested seek generation; zero is the initial read.
    pub generation: u64,
    /// Frames published in the PCM ring (possibly from an older seek generation).
    pub buffered_frames: usize,
    /// Read attempts that could not return even one frame. Not a count of silent samples.
    pub underruns: u64,
    pub cancelled: bool,
}
struct Ring {
    samples: Box<[AtomicU32]>,
    generations: Box<[AtomicU64]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    generation: AtomicU64,
    target: AtomicU64,
    /// (generation+1)<<8 | status, where 1=EOF and 2..=9 encode errors.
    terminal: AtomicU64,
    cancelled: AtomicBool,
    worker_alive: AtomicBool,
    underruns: AtomicU64,
    info: Info,
}
impl Ring {
    fn new(info: Info, capacity: usize) -> Self {
        Self {
            samples: (0..capacity * info.channels)
                .map(|_| AtomicU32::new(0))
                .collect(),
            generations: (0..capacity).map(|_| AtomicU64::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
            target: AtomicU64::new(0),
            terminal: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            worker_alive: AtomicBool::new(true),
            underruns: AtomicU64::new(0),
            info,
        }
    }
    fn capacity(&self) -> usize {
        self.generations.len()
    }
    fn next(&self, at: usize) -> usize {
        (at + 1) % (2 * self.capacity())
    }
    fn len(&self) -> usize {
        // A control-thread snapshot may race both counters; clamp for diagnostics.
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        (tail + 2 * self.capacity() - head) % (2 * self.capacity())
    }
    fn push(&self, frame: &[f32], generation: u64) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if (tail + 2 * self.capacity() - head) % (2 * self.capacity()) >= self.capacity() {
            return false;
        }
        let at = tail % self.capacity();
        for (c, &sample) in frame.iter().enumerate() {
            self.samples[at * self.info.channels + c].store(sample.to_bits(), Ordering::Relaxed);
        }
        self.generations[at].store(generation, Ordering::Relaxed);
        self.tail.store(self.next(tail), Ordering::Release);
        true
    }
    fn finish(&self, generation: u64, error: Option<ErrorCode>) {
        self.terminal.store(
            ((generation + 1) << 8) | error.map_or(1, |e| error_number(e) as u64 + 1),
            Ordering::Release,
        );
    }
}

/// A nonblocking, allocation-free consumer of a fixed PCM ring. Requires
/// `file-stream` on native targets; mixer integration additionally needs `mixer`.
///
/// Opening waits for header scanning and one prefilled ring, on a loading thread.
/// WAV/FLAC/MP3 files are read through a bounded encoded window, never loaded whole.
/// MP3 opening scans frame headers once; compressed seeks replay from the start on
/// the worker, retaining exact decoder state. They may take time for long files.
/// Metadata is skipped; this API exposes audio shape, not tags. Files must remain
/// unchanged while streaming. Loop restarts are asynchronous and can underrun.
///
/// Keep the returned [`FileStreamHandle`] on a control thread. Dropping the stream
/// only requests cancellation; dropping the handle cancels and joins the worker.
/// Construction, handle destruction and final stream destruction belong off the
/// callback (the mixer's returned-sound queue provides this for played sources).
pub struct FileStream {
    ring: Arc<Ring>,
    position: u64,
    generation: u64,
}
/// Owns reader lifetime and memory until destruction on a control/loading thread.
/// Dropping this cancels playback and waits for the reader; never drop in a callback.
pub struct FileStreamHandle {
    ring: Arc<Ring>,
    worker: Option<JoinHandle<()>>,
}
impl FileStream {
    pub fn open(path: impl AsRef<Path>) -> Result<(Self, FileStreamHandle)> {
        Self::with_options(path, &FileStreamOptions::default())
    }
    pub fn with_options(
        path: impl AsRef<Path>,
        options: &FileStreamOptions,
    ) -> Result<(Self, FileStreamHandle)> {
        options.validate()?;
        let path = path.as_ref().to_owned();
        let options = options.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("audiobox-file-reader".into())
            .spawn(move || {
                let result = FileDecoder::open(&path, &options);
                let mut decoder = match result {
                    Ok(d) => d,
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                };
                let info = Info {
                    channels: decoder.channels,
                    rate: decoder.rate,
                    frames: decoder.frames,
                    format: decoder.format,
                };
                let pcm_bytes = options
                    .buffer_frames
                    .checked_mul(info.channels * 4 + 8)
                    .and_then(|n| n.checked_add(options.decoder.chunk_frames * info.channels * 4));
                if pcm_bytes.is_none_or(|n| n > options.max_pcm_bytes) {
                    let _ = tx.send(Err(Error::limit("PCM ring/scratch exceeds budget")));
                    return;
                }
                let ring = Arc::new(Ring::new(info, options.buffer_frames));
                let _alive = Alive(ring.clone());
                let mut scratch = vec![0.0; options.decoder.chunk_frames * info.channels];
                let initial = fill_initial(&mut decoder, &ring, &mut scratch);
                if let Err(e) = initial {
                    let _ = tx.send(Err(e));
                    return;
                }
                if tx.send(Ok(ring.clone())).is_err() {
                    return;
                }
                run(&mut decoder, &ring, &mut scratch);
            })?;
        match rx.recv() {
            Ok(Ok(ring)) => Ok((
                Self {
                    ring: ring.clone(),
                    position: 0,
                    generation: 0,
                },
                FileStreamHandle {
                    ring,
                    worker: Some(worker),
                },
            )),
            result => {
                let _ = worker.join();
                Err(match result {
                    Ok(Err(e)) => e,
                    _ => Error::new(ErrorCode::Io, "Reader exited during opening"),
                })
            }
        }
    }
    pub fn sample_rate(&self) -> u32 {
        self.ring.info.rate
    }
    pub fn channels(&self) -> usize {
        self.ring.info.channels
    }
    pub fn frames(&self) -> Option<u64> {
        self.ring.info.frames
    }
    pub fn format(&self) -> AudioFormat {
        self.ring.info.format
    }
    /// Index of the next delivered source frame. A seek updates it immediately.
    pub fn position(&self) -> u64 {
        self.position
    }
    /// Drain available complete frames. Returns 0 only for actual EOF (or a buffer
    /// shorter than one frame). A partial read is returned before any pending/error
    /// state. `Underrun` means retry later; the destination is untouched on that error.
    /// Error messages are empty so all read paths, including errors, allocate nothing.
    pub fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        let ch = self.channels();
        let wanted = out.len() / ch;
        if wanted == 0 {
            return Ok(0);
        }
        let ring = &self.ring;
        let mut written = 0;
        // Bound stale-generation draining to one ring even if seeks race the reader.
        for _ in 0..ring.capacity() {
            if written == wanted {
                break;
            }
            let head = ring.head.load(Ordering::Relaxed);
            let tail = ring.tail.load(Ordering::Acquire);
            if head == tail {
                break;
            }
            let at = head % ring.capacity();
            if ring.generations[at].load(Ordering::Relaxed) == self.generation {
                for c in 0..ch {
                    out[written * ch + c] =
                        f32::from_bits(ring.samples[at * ch + c].load(Ordering::Relaxed));
                }
                written += 1;
            }
            ring.head.store(ring.next(head), Ordering::Release);
        }
        self.position += written as u64;
        if written > 0 {
            return Ok(written);
        }
        if ring.cancelled.load(Ordering::Acquire) {
            return Err(empty_error(ErrorCode::Io));
        }
        let terminal = ring.terminal.load(Ordering::Acquire);
        // This acquire observes every frame preceding the terminal publication.
        let empty = ring.head.load(Ordering::Relaxed) == ring.tail.load(Ordering::Acquire);
        if empty && terminal >> 8 == self.generation + 1 {
            if terminal & 255 == 1 {
                return Ok(0);
            }
            if terminal & 255 > 1 {
                return Err(empty_error(number_error((terminal & 255) as u8 - 1)));
            }
        }
        if !ring.worker_alive.load(Ordering::Acquire) {
            return Err(empty_error(ErrorCode::Io));
        }
        ring.underruns.fetch_add(1, Ordering::Relaxed);
        Err(empty_error(ErrorCode::Underrun))
    }
    /// Queue a seek without waiting, allocating or touching the file. Old queued
    /// frames are discarded by generation. Next read can underrun while seeking.
    pub fn seek(&mut self, frame: u64) -> Result<()> {
        if self.generation >= (u64::MAX >> 8) - 1 {
            return Err(empty_error(ErrorCode::LimitExceeded));
        }
        let frame = self.frames().map_or(frame, |n| frame.min(n));
        self.position = frame;
        self.generation += 1;
        self.ring.target.store(frame, Ordering::Relaxed);
        self.ring
            .generation
            .store(self.generation, Ordering::Release);
        Ok(())
    }
    pub fn rewind(&mut self) -> Result<()> {
        self.seek(0)
    }
}
impl Drop for FileStream {
    fn drop(&mut self) {
        self.ring.cancelled.store(true, Ordering::Release);
    }
}
impl FileStreamHandle {
    pub fn status(&self) -> FileStreamStatus {
        FileStreamStatus {
            generation: self.ring.generation.load(Ordering::Acquire),
            buffered_frames: self.ring.len().min(self.ring.capacity()),
            underruns: self.ring.underruns.load(Ordering::Relaxed),
            cancelled: self.ring.cancelled.load(Ordering::Acquire),
        }
    }
    /// Cancel the reader; joining happens when the handle is dropped off-thread.
    pub fn cancel(&self) {
        self.ring.cancelled.store(true, Ordering::Release);
    }
}
impl Drop for FileStreamHandle {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
struct Alive(Arc<Ring>);
impl Drop for Alive {
    fn drop(&mut self) {
        self.0.worker_alive.store(false, Ordering::Release);
    }
}
fn empty_error(code: ErrorCode) -> Error {
    Error {
        code,
        message: String::new(),
    }
}
fn error_number(code: ErrorCode) -> u8 {
    match code {
        ErrorCode::UnsupportedFormat => 1,
        ErrorCode::DecodeError => 2,
        ErrorCode::EncodeError => 3,
        ErrorCode::InvalidArgument => 4,
        ErrorCode::LimitExceeded => 5,
        ErrorCode::Io => 6,
        ErrorCode::Syntax => 7,
        ErrorCode::Underrun => 8,
    }
}
fn number_error(n: u8) -> ErrorCode {
    match n {
        1 => ErrorCode::UnsupportedFormat,
        2 => ErrorCode::DecodeError,
        3 => ErrorCode::EncodeError,
        4 => ErrorCode::InvalidArgument,
        5 => ErrorCode::LimitExceeded,
        7 => ErrorCode::Syntax,
        8 => ErrorCode::Underrun,
        _ => ErrorCode::Io,
    }
}
fn fill_initial(decoder: &mut FileDecoder, ring: &Ring, scratch: &mut [f32]) -> Result<()> {
    while ring.len() < ring.capacity() {
        let want = (ring.capacity() - ring.len()).min(scratch.len() / decoder.channels);
        let n = decoder.read(&mut scratch[..want * decoder.channels])?;
        if n == 0 {
            ring.finish(0, None);
            break;
        }
        for frame in scratch[..n * decoder.channels].chunks_exact(decoder.channels) {
            assert!(ring.push(frame, 0));
        }
    }
    Ok(())
}
fn run(decoder: &mut FileDecoder, ring: &Ring, scratch: &mut [f32]) {
    let (mut generation, mut offset, mut ready) = (0, 0, 0);
    loop {
        if ring.cancelled.load(Ordering::Acquire) {
            return;
        }
        let requested = ring.generation.load(Ordering::Acquire);
        if requested != generation {
            generation = requested;
            offset = 0;
            ready = 0;
            ring.terminal.store(0, Ordering::Release);
            let target = ring.target.load(Ordering::Relaxed);
            let result = decoder.seek(target, || {
                ring.cancelled.load(Ordering::Acquire)
                    || ring.generation.load(Ordering::Acquire) != generation
            });
            if let Err(e) = result {
                ring.finish(generation, Some(e.code));
            }
            continue;
        }
        if offset < ready {
            let ch = decoder.channels;
            if ring.push(&scratch[offset * ch..(offset + 1) * ch], generation) {
                offset += 1;
                continue;
            }
        } else if ring.terminal.load(Ordering::Acquire) == 0 {
            match decoder.read(scratch) {
                Ok(0) => ring.finish(generation, None),
                Ok(n) => {
                    offset = 0;
                    ready = n;
                    continue;
                }
                Err(e) => ring.finish(generation, Some(e.code)),
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(feature = "mixer")]
impl crate::mixer::Stream for FileStream {
    fn sample_rate(&self) -> u32 {
        self.sample_rate()
    }
    fn channels(&self) -> usize {
        self.channels()
    }
    fn frames(&self) -> Option<u64> {
        self.frames()
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        self.read(out)
    }
    fn seek(&mut self, frame: u64) -> Result<()> {
        self.seek(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(capacity: usize) -> FileStream {
        FileStream {
            ring: Arc::new(Ring::new(
                Info {
                    channels: 1,
                    rate: 8000,
                    frames: None,
                    format: AudioFormat::Wav,
                },
                capacity,
            )),
            position: 0,
            generation: 0,
        }
    }
    #[test]
    fn underrun_does_not_touch_output_or_turn_into_eof() {
        let mut stream = source(3);
        let mut out = [7.0; 4];
        let e = stream.read(&mut out).unwrap_err();
        assert_eq!(e.code, ErrorCode::Underrun);
        assert!(e.message.is_empty());
        assert_eq!(out, [7.0; 4]);
        assert_eq!(stream.position(), 0);
        assert!(stream.ring.push(&[0.5], 0));
        stream.ring.finish(0, None);
        assert_eq!(stream.read(&mut out).unwrap(), 1);
        assert_eq!(out, [0.5, 7.0, 7.0, 7.0]);
        assert_eq!(stream.read(&mut out).unwrap(), 0);
    }
    #[test]
    fn stale_audio_and_terminal_are_ignored_after_seek() {
        let mut stream = source(3);
        for _ in 0..3 {
            assert!(stream.ring.push(&[-1.0], 0));
        }
        stream.ring.finish(0, Some(ErrorCode::DecodeError));
        stream.seek(100).unwrap();
        assert_eq!(
            stream.read(&mut [0.0; 10]).unwrap_err().code,
            ErrorCode::Underrun
        );
        assert!(stream.ring.push(&[0.75], 1));
        stream.ring.finish(1, None);
        let mut out = [0.0; 2];
        assert_eq!(stream.read(&mut out).unwrap(), 1);
        assert_eq!(out[0], 0.75);
        assert_eq!(stream.position(), 101);
        assert_eq!(stream.read(&mut out).unwrap(), 0);
    }
    #[test]
    fn worker_disconnection_and_terminal_errors_are_stable() {
        let mut stream = source(1);
        stream.ring.finish(0, Some(ErrorCode::DecodeError));
        for _ in 0..3 {
            assert_eq!(
                stream.read(&mut [0.0]).unwrap_err().code,
                ErrorCode::DecodeError
            );
        }
        stream.seek(0).unwrap();
        stream.ring.worker_alive.store(false, Ordering::Release);
        assert_eq!(stream.read(&mut [0.0]).unwrap_err().code, ErrorCode::Io);
    }
    #[test]
    fn odd_ring_wraps_and_eof_never_overtakes_the_last_frame() {
        let mut stream = source(7);
        let ring = stream.ring.clone();
        let worker = thread::spawn(move || {
            for i in 0..50000 {
                while !ring.push(&[i as f32], 0) {
                    thread::yield_now();
                }
            }
            ring.finish(0, None);
        });
        let mut expected = 0;
        let mut buffer = [0.0; 11];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    for &v in &buffer[..n] {
                        assert_eq!(v, expected as f32);
                        expected += 1;
                    }
                }
                Err(e) if e.code == ErrorCode::Underrun => thread::yield_now(),
                Err(e) => panic!("{e:?}"),
            }
        }
        worker.join().unwrap();
        assert_eq!(expected, 50000);
    }
}
