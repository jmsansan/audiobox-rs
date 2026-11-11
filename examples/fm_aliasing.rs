//! Measures the folded (aliased) energy of FM patches. Run with
//! `--release --features synth`. `docs/fm-aliasing.md` explains the method.
//!
//! Each patch holds one note. Its true partials lie on multiples of half the note's
//! frequency (every patch here uses whole or half ratios), so energy between those
//! lines is energy that folded at half the sample rate. The figure is that energy
//! below 0.4 of the sample rate (19.2 kHz at 48 kHz), relative to the whole signal,
//! from a Blackman-Harris window over 65536 samples. About -90 dB is the floor.
//! "Ideal 2x" renders at twice the rate and resamples down with the crate's
//! resampler; `oversample=2` is the instrument's own option.
use audiobox::synth::{RenderOptions, Score};

const PATCHES: [(&str, &str); 6] = [
    ("sine", "fm p\nop 1\nend"),
    (
        "bright bell",
        "fm p algorithm=1\nop 1 ratio=1\nop 2 ratio=3,50 level=85\nend",
    ),
    (
        "bright bell, scaled",
        "fm p algorithm=1\nop 1 ratio=1\nop 2 ratio=3,50 level=85 scaling=39,0,25\nend",
    ),
    (
        "feedback bass",
        "fm p algorithm=2 feedback=5\nop 1 ratio=1\nop 2 ratio=1 level=80\nend",
    ),
    (
        "six-carrier organ",
        "fm p algorithm=32 feedback=4\nop 1 ratio=1\nop 2 ratio=2\nop 3 ratio=3\nop 4 ratio=4\nop 5 ratio=6\nop 6 ratio=8\nend",
    ),
    (
        "electric piano",
        "fm p algorithm=5 feedback=4\nop 1 ratio=1\nop 2 ratio=14 level=58\nop 3 ratio=1\nop 4 ratio=1 level=82\nop 5 ratio=1\nop 6 ratio=1 level=76\nend",
    ),
];
const SIZE: usize = 65536;

fn render(patch: &str, octave: u8, rate: u32) -> audiobox::Result<audiobox::Audio> {
    let text = format!("tempo 30\n{patch}\nchannel a=@p q8 o{octave} c1");
    let options = RenderOptions {
        gain: 1.0,
        ..RenderOptions::default()
    };
    Score::parse(&text)?.render_with(rate, &options)
}

fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -std::f64::consts::TAU / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (angle * k as f64).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                (re[a], im[a]) = (re[a] + tr, im[a] + ti);
            }
        }
        len <<= 1;
    }
}

/// Folded energy relative to the signal, in dB.
fn folded(samples: &[f32], rate: u32, note_hz: f64) -> f64 {
    let body = &samples[rate as usize..rate as usize + SIZE];
    let mut re: Vec<f64> = body
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let t = std::f64::consts::TAU * i as f64 / SIZE as f64;
            let window =
                0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos();
            x as f64 * window
        })
        .collect();
    let mut im = vec![0.0; SIZE];
    fft(&mut re, &mut im);
    let spacing = note_hz / 2.0 * SIZE as f64 / rate as f64;
    let (mut on, mut off) = (0.0, 0.0);
    for bin in 1..SIZE / 2 {
        let power = re[bin] * re[bin] + im[bin] * im[bin];
        let nearest = (bin as f64 / spacing).round() * spacing;
        if (bin as f64 - nearest).abs() <= 6.0 {
            on += power;
        } else if (bin as f64) < 0.4 * SIZE as f64 {
            off += power;
        }
    }
    10.0 * (off / (on + off)).max(1e-30).log10()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "| Patch | Note | 44.1 kHz | 48 kHz | 96 kHz | 48 kHz, ideal 2x | 48 kHz, `oversample=2` |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    for (name, patch) in PATCHES {
        for octave in [2u8, 4, 5, 6, 7] {
            let note_hz = 440.0 * 2f64.powf((octave as f64 - 4.0) - 0.75);
            let mut row = format!("| {name} | C{octave} |");
            for rate in [44100u32, 48000, 96000] {
                let audio = render(patch, octave, rate)?;
                row += &format!(" {:.0} dB |", folded(audio.channel_data(0)?, rate, note_hz));
            }
            let twice = render(patch, octave, 96000)?.resample(48000)?;
            row += &format!(
                " {:.0} dB |",
                folded(twice.channel_data(0)?, 48000, note_hz)
            );
            let over = render(
                &patch.replacen("fm p", "fm p oversample=2", 1),
                octave,
                48000,
            )?;
            row += &format!(" {:.0} dB |", folded(over.channel_data(0)?, 48000, note_hz));
            println!("{row}");
        }
    }
    Ok(())
}
