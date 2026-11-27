//! Prints the voices of a DX7 file as FM patch text: a single voice, a 32-voice bank,
//! several messages saved together, or a bank saved without its message wrapper.
//! Run with `--features synth -- FILE.syx`. Without a file it converts a generated
//! voice, so the example runs anywhere.
//!
//! No patch data ships with audiobox. The rights to a file you import are yours to
//! check.
use audiobox::synth::{DX7_RAW_BANK_BYTES, FmPatch, read_dx7, read_dx7_raw_bank, write_dx7_voice};

/// Larger files are refused before they are read: a megabyte is 255 banks.
const MAX_FILE_BYTES: u64 = 1 << 20;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = match std::env::args().nth(1) {
        Some(path) => {
            let size = std::fs::metadata(&path)?.len();
            if size > MAX_FILE_BYTES {
                return Err(format!(
                    "{path} is {size} bytes; this example reads files of at most {MAX_FILE_BYTES}"
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
    // A 4096-byte file with no message in it is a bank saved without its wrapper.
    let read = if bytes.len() == DX7_RAW_BANK_BYTES && !bytes.contains(&0xf0) {
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
