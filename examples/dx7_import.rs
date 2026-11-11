//! Prints the voices of a DX7 single-voice or 32-voice file as FM patch text.
//! Run with `--features synth -- FILE.syx`. Without a file it converts a generated
//! voice, so the example runs anywhere.
//!
//! No patch data ships with audiobox. The rights to a file you import are yours to
//! check.
use audiobox::synth::{DX7_BANK_BYTES, FmPatch, read_dx7, write_dx7_voice};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = match std::env::args().nth(1) {
        Some(path) => {
            let size = std::fs::metadata(&path)?.len();
            if size > DX7_BANK_BYTES as u64 {
                return Err(format!(
                    "{path} is {size} bytes; DX7 voice data is at most {DX7_BANK_BYTES}"
                )
                .into());
            }
            std::fs::read(&path)?
        }
        None => {
            let demo: FmPatch = "fm DEMO algorithm=5 feedback=6\n  op 1\n  op 2 ratio=14 level=58 velocity=7\n  op 3 detune=-5\n  op 5 detune=5\nend".parse()?;
            write_dx7_voice(&demo)?
        }
    };
    let read = read_dx7(&bytes)?;
    for warning in &read.warnings {
        match warning.voice {
            Some(index) => eprintln!("warning, voice {}: {}", index + 1, warning.message),
            None => eprintln!("warning: {}", warning.message),
        }
    }
    for (index, voice) in read.voices.iter().enumerate() {
        println!(
            "; voice {} of {}, originally \"{}\"",
            index + 1,
            read.voices.len(),
            voice.original_name
        );
        println!("{}", voice.patch);
    }
    Ok(())
}
