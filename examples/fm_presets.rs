//! Renders every built-in FM preset and prints what can be measured about it.
//! Run with `--features synth -- OUT_DIR`. `docs/fm-presets.md` describes the patches.
//!
//! Each preset plays a short phrase in its own register, soft and then loud, into
//! `OUT_DIR/NAME.wav`. The files are brought to the same loudness, so they compare
//! tone; the table gives each preset's own level. The table gives, for one note at two velocities, the peak
//! level, the A-weighted loudness and the spectral centroid (a rough measure of
//! brightness), taken 20 ms into
//! struck sounds, 0.6 s into the brass and organ and 2 s into the strings.
use audiobox::synth::{FM_PRESETS, RenderOptions, Score};
use audiobox::{EncodeOptions, write_audio_file};
use std::path::PathBuf;

/// Integrated loudness every listening file is brought to.
const LISTENING_LUFS: f64 = -18.0;

/// The phrase each preset plays (`$` marks where a velocity goes), and the note it
/// is measured on.
fn phrase(name: &str) -> (&'static str, &'static str) {
    match name {
        "bass-round" | "bass-hard" => ("o2 l8 c$ c$ >c$< c$ e-$ e-$ g$ b-$ >c2$<", "o2 c1"),
        "brass" | "strings" => ("o4 l2 {c e g}$ {a >c e<}$ l1 {f a >c<}$", "o4 c1"),
        "organ" => ("o4 l4 {c e g}$ {c f a}$ {d g b}$ {c e g >c<}2.$", "o4 c1"),
        "bell" | "marimba" => ("o5 l8 c$ e$ g$ >c$< g$ e$ c4$ {c g >e<}1$", "o5 c1"),
        "metal" => ("o4 l8 c$ r c$ c$ r c4.$ o6 c2$", "o4 c1"),
        _ => ("o4 l8 c$ e$ g$ >c$< {c e g b}2$ {a >c< e g}2$", "o4 c1"),
    }
}

/// From 8192 Hann-windowed samples starting `from` seconds in: the power-weighted
/// mean frequency, and the A-weighted level in dB relative to a full-scale sine, which
/// follows how loud the sound seems better than its peak does.
fn measure(samples: &[f32], rate: f64, from: f64) -> (f64, f64) {
    let start = (rate * from) as usize;
    let body = &samples[start..(start + 8192).min(samples.len())];
    let n = body.len();
    let (mut weighted, mut total, mut heard) = (0.0, 0.0, 0.0);
    for bin in 1..n / 2 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &x) in body.iter().enumerate() {
            let window = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            let angle = std::f64::consts::TAU * (bin * i % n) as f64 / n as f64;
            re += x as f64 * window * angle.cos();
            im += x as f64 * window * angle.sin();
        }
        let (power, hz) = (re * re + im * im, bin as f64 * rate / n as f64);
        weighted += power * hz;
        total += power;
        let f2 = hz * hz;
        let a = 12194f64.powi(2) * f2 * f2
            / ((f2 + 20.6f64.powi(2))
                * ((f2 + 107.7f64.powi(2)) * (f2 + 737.9f64.powi(2))).sqrt()
                * (f2 + 12194f64.powi(2)))
            * 1.2589;
        heard += power * a * a;
    }
    // Mean square of the windowed signal from its spectrum; a full-scale sine is 0.5.
    let mean_square = 2.0 * heard / (n as f64 * 0.375 * n as f64);
    (
        weighted / total.max(1e-30),
        10.0 * (mean_square / 0.5).max(1e-12).log10(),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/fm-presets".into()),
    );
    std::fs::create_dir_all(&dir)?;
    let options = RenderOptions {
        gain: 1.0,
        ..RenderOptions::default()
    };
    println!(
        "| Preset | Peak at 60 | Peak at 127 | Loudness at 60 | Loudness at 127 | Centroid at 60 | Centroid at 127 |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    let mut levels = Vec::new();
    for name in FM_PRESETS {
        let (notes, single) = phrase(name);
        let mut row = format!("| `{name}` |");
        let mut cells = Vec::new();
        for velocity in [60, 127] {
            let text =
                format!("tempo 60\nfm p preset={name}\nend\nchannel a=@p q8 {single}!{velocity}");
            let audio = Score::parse(&text)?.render_with(48000, &options)?;
            let samples = audio.channel_data(0)?;
            let peak = samples.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            // Struck sounds are brightest at once; swelling ones after their attack.
            let from = match name {
                "strings" => 2.0,
                "brass" | "organ" => 0.6,
                _ => 0.02,
            };
            cells.push((peak, measure(samples, 48000.0, from)));
        }
        row += &format!(
            " {:.2} | {:.2} | {:.0} dB | {:.0} dB | {:.0} Hz | {:.0} Hz |",
            cells[0].0, cells[1].0, cells[0].1.1, cells[1].1.1, cells[0].1.0, cells[1].1.0
        );
        println!("{row}");
        // The phrase, soft then loud.
        let (soft, loud) = (notes.replace('$', "!55"), notes.replace('$', "!120"));
        let text = format!("tempo 104\nfm p preset={name}\nend\nchannel a=@p {soft} r2 {loud}");
        let audio = Score::parse(&text)?.render_stereo_with(
            48000,
            &RenderOptions {
                gain: 0.22,
                ..RenderOptions::default()
            },
        )?;
        // Bring every file to the same loudness, so the listener compares tone and
        // not level; a file whose peaks would clip is left a little quieter.
        let wanted = 10f64.powf((LISTENING_LUFS - audio.loudness().integrated) / 20.0);
        let audio = audio.gain(wanted.min(0.97 / audio.peak().max(1e-9)))?;
        levels.push(format!("{name} {:.1}", audio.loudness().integrated));
        write_audio_file(
            dir.join(format!("{name}.wav")),
            &audio,
            &EncodeOptions::default(),
        )?;
    }
    println!("\nWrote {} files to {}", FM_PRESETS.len(), dir.display());
    println!("Loudness of each file in LUFS: {}", levels.join(", "));
    Ok(())
}
