use audiobox::{EncodeOptions, TimePosition, read_audio_file, write_audio_file};
fn main() -> audiobox::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: cargo run --example convert -- input.mp3 output.flac");
        std::process::exit(2);
    }
    let audio = read_audio_file(&args[1])?;
    println!("{audio}");
    let clip = audio.cut(
        TimePosition::Frames(0),
        TimePosition::Frames(audio.frames() as i64),
    )?;
    write_audio_file(&args[2], &clip, &EncodeOptions::default())
}
