//! Renders every built-in FM preset and prints what can be measured about it.
//! Run with `--features synth -- OUT_DIR`. `docs/fm-presets.md` describes the patches.
//!
//! Each preset plays a short phrase in its own register, soft and then loud, into
//! `OUT_DIR/NAME.wav`. The table gives, for one note at two velocities, the peak
//! level and the spectral centroid (a rough measure of brightness), taken 20 ms into
//! struck sounds, 0.6 s into the brass and organ and 2 s into the strings.
use audiobox::synth::{FM_PRESETS, RenderOptions, Score};
use audiobox::{EncodeOptions, write_audio_file};
use std::path::PathBuf;

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

/// Power-weighted mean frequency of 4096 Hann-windowed samples from `from` seconds in.
fn centroid(samples: &[f32], rate: f64, from: f64) -> f64 {
    let start = (rate * from) as usize;
    let body = &samples[start..(start + 4096).min(samples.len())];
    let n = body.len();
    let (mut weighted, mut total) = (0.0, 0.0);
    for bin in 1..n / 2 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &x) in body.iter().enumerate() {
            let window = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            let angle = std::f64::consts::TAU * (bin * i % n) as f64 / n as f64;
            re += x as f64 * window * angle.cos();
            im += x as f64 * window * angle.sin();
        }
        let power = re * re + im * im;
        weighted += power * bin as f64 * rate / n as f64;
        total += power;
    }
    weighted / total.max(1e-30)
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
    println!("| Preset | Peak at 60 | Peak at 127 | Centroid at 60 | Centroid at 127 | Seconds |");
    println!("| --- | --- | --- | --- | --- | --- |");
    for name in FM_PRESETS {
        let (notes, single) = phrase(name);
        let mut row = format!("| `{name}` |");
        let mut cells = Vec::new();
        let mut seconds = 0.0;
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
            cells.push((peak, centroid(samples, 48000.0, from)));
            seconds = audio.frames() as f64 / 48000.0;
        }
        row += &format!(
            " {:.2} | {:.2} | {:.0} Hz | {:.0} Hz | {seconds:.1} |",
            cells[0].0, cells[1].0, cells[0].1, cells[1].1
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
        write_audio_file(
            dir.join(format!("{name}.wav")),
            &audio,
            &EncodeOptions::default(),
        )?;
    }
    println!("\nWrote {} files to {}", FM_PRESETS.len(), dir.display());
    Ok(())
}
