//! Disk I/O stays on the reader; the host supplies device output in a real game.
use audiobox::mixer::{Loop, Mixer, MixerConfig, Play, Sound};
use audiobox::stream::FileStream;
use audiobox::{Audio, EncodeOptions, Result, write_audio_file};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let input = if let Some(path) = args.next() {
        PathBuf::from(path)
    } else {
        let path = PathBuf::from("target/disk-input.flac");
        std::fs::create_dir_all("target")?;
        let tone = (0..48000)
            .map(|n| (n as f32 * std::f32::consts::TAU * 220.0 / 48000.0).sin() * 0.4)
            .collect();
        write_audio_file(
            &path,
            &Audio::new(vec![tone], 48000)?,
            &EncodeOptions::default(),
        )?;
        path
    };
    let output = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "target/disk-stream.wav".into()),
    );
    let (source, reader) = FileStream::open(&input)?;
    let (rate, channels) = (source.sample_rate(), source.channels());
    let (mut mixer, mut control) = Mixer::new(MixerConfig {
        sample_rate: rate,
        channels,
        ..Default::default()
    })?;
    control.play(
        Sound::stream(source),
        &Play {
            looping: Loop::Whole,
            gain: 0.7,
            ..Default::default()
        },
    )?;
    // Offline demonstration paced in small callback-sized blocks. Device callbacks
    // call render directly; they never sleep or perform the export below.
    let mut samples = vec![0.0; rate as usize * 5 * channels];
    for block in samples.chunks_mut(256 * channels) {
        mixer.render(block);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let data = (0..channels)
        .map(|c| samples.iter().skip(c).step_by(channels).copied().collect())
        .collect();
    let audio = Audio::new(data, rate)?;
    write_audio_file(&output, &audio, &EncodeOptions::default())?;
    println!(
        "{}: {} frames; {} underrun read attempts (loop seeks can leave gaps)",
        output.display(),
        audio.frames(),
        reader.status().underruns
    );
    drop(mixer);
    while control.poll_event().is_some() {}
    drop(control);
    drop(reader);
    Ok(())
}
