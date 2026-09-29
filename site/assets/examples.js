export const examples = {
  edit: { file: 'prepare.rs', description: 'Extract a clip, make it mono, resample to 16 kHz, and normalize its peak. The input stays untouched.', code: `use audiobox::{AudioFormat, NormalizeOptions,
    decode, encode};

fn prepare(bytes: &[u8]) -> audiobox::Result<Vec<u8>> {
    let audio = decode(bytes)?;
    let ready = audio
        .cut(12.0, 30.0)?
        .to_mono()
        .resample(16_000)?
        .normalize(NormalizeOptions::default())?;

    encode(&ready, AudioFormat::Wav)
}` },
  convert: { file: 'convert.rs', description: 'Decode an MP3 and write a lossless FLAC. Container detection and the codecs are built into the crate.', code: `use audiobox::{EncodeOptions,
    read_audio_file, write_audio_file};

fn main() -> audiobox::Result<()> {
    let audio = read_audio_file("recording.mp3")?;

    write_audio_file(
        "recording.flac",
        &audio,
        &EncodeOptions::default(),
    )?;
    Ok(())
}` },
  analyze: { file: 'analyze.rs', description: 'Check levels and integrated loudness, then extract waveform data for your own interface.', code: `use audiobox::read_audio_file;

fn main() -> audiobox::Result<()> {
    let audio = read_audio_file("recording.wav")?;
    let levels = audio.loudness();
    let waveform = audio.waveform(512)?;

    println!("Peak: {:.1} dBFS", audio.peak_db());
    println!("RMS: {:.1} dBFS", audio.rms_db());
    println!("Integrated: {:.1} LUFS", levels.integrated);
    println!("Waveform buckets: {}", waveform.min.len());
    Ok(())
}` },
  stream: { file: 'pcm.rs', description: 'Feed raw PCM in any byte chunks. The decoder retains incomplete frames until the next push.', code: `use audiobox::{Endian, Limits, SampleFormat};
use audiobox::stream::PcmDecoder;

fn main() -> audiobox::Result<()> {
    let mut decoder = PcmDecoder::new(
        SampleFormat::S16, Endian::Little,
        2, 48_000, Limits::default(),
    )?;

    let bytes = std::fs::read("stereo.pcm")?;
    for chunk in bytes.chunks(4096) {
        let audio = decoder.push(chunk)?;
        println!("Decoded {} frames", audio.frames());
    }
    decoder.finish()?;
    Ok(())
}` }
};
