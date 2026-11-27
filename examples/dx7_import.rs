//! Prints the voices of a DX7 file as FM patch text: a single voice, a 32-voice bank,
//! or several messages saved together. A bank saved without its message wrapper has
//! no header to recognise it by: pass `--raw` for one.
//! Run with `--features synth -- [--raw] FILE.syx`. Without a file it converts a
//! generated voice, so the example runs anywhere.
//!
//! No patch data ships with audiobox. The rights to a file you import are yours to
//! check.
use std::io::Read as _;

use audiobox::synth::{FmPatch, read_dx7, read_dx7_raw_bank, write_dx7_voice};

/// Larger files are refused: a megabyte is 255 banks.
const MAX_FILE_BYTES: u64 = 1 << 20;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let raw = args.iter().any(|a| a == "--raw");
    let path = args.iter().find(|a| *a != "--raw");
    let bytes = match path {
        Some(path) => {
            // One byte past the cap is read, so that a file that grew is still refused.
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take(MAX_FILE_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_FILE_BYTES {
                return Err(format!(
                    "{path} is larger than the {MAX_FILE_BYTES} bytes this example reads"
                )
                .into());
            }
            bytes
        }
        None => {
            let demo: FmPatch = "fm DEMO algorithm=5 feedback=6\n  op 1\n  op 2 ratio=14 level=58 velocity=7\n  op 3 detune=-5\n  op 5 detune=5\nend".parse()?;
            write_dx7_voice(&demo)?
        }
    };
    let read = if raw {
        read_dx7_raw_bank(&bytes)?
    } else {
        read_dx7(&bytes)?
    };
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
