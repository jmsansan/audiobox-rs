//! Measures prepared voice rendering cost. Run in release:
//!
//! `cargo run --release --example voice_bench --features voice`
//!
//! For each preset, rate, callback size and voice count it renders a whole
//! utterance per voice in callback-sized blocks and reports the mean and the
//! worst callback as a share of the callback's real-time deadline, plus the time
//! to plan and prepare one line. Results depend on the machine; record them with
//! the hardware and compiler.
use audiobox::voice::{
    GibberishOptions, PreparedUtterance, VoiceOptions, VoicePreset, VoiceProfile, plan_gibberish,
};
use std::hint::black_box;
use std::time::Instant;

const LINE: &str =
    "¿Qué tal, viajero? Hoy el camino es largo, pero llegaremos antes de la noche. ¡Vamos!";

fn main() -> audiobox::Result<()> {
    println!("preset,rate,block,voices,mean_pct,worst_pct,prepare_us");
    for preset in [
        VoicePreset::Robot,
        VoicePreset::SoftRounded,
        VoicePreset::AiryBreathy,
    ] {
        for rate in [44_100, 48_000] {
            let profile = VoiceProfile::preset(preset);
            let start = Instant::now();
            let plan = plan_gibberish(LINE, &profile, &GibberishOptions::default())?;
            let options = VoiceOptions {
                sample_rate: rate,
                ..VoiceOptions::default()
            };
            let voice = PreparedUtterance::new(&profile, &plan, &options)?;
            let prepare_us = start.elapsed().as_secs_f64() * 1e6;
            for block in [64usize, 256, 1024] {
                for voices in [1usize, 8, 32] {
                    let mut synths: Vec<_> = (0..voices).map(|_| voice.synth()).collect();
                    let mut out = vec![0.0f32; block];
                    let mut mix = vec![0.0f32; block];
                    let deadline = block as f64 / rate as f64;
                    let (mut total, mut worst, mut callbacks) = (0.0f64, 0.0f64, 0usize);
                    loop {
                        let t = Instant::now();
                        let mut any = 0;
                        mix.fill(0.0);
                        for synth in &mut synths {
                            let n = synth.fill(&mut out);
                            any += n;
                            for (m, s) in mix[..n].iter_mut().zip(&out) {
                                *m += s;
                            }
                        }
                        black_box(&mix);
                        let spent = t.elapsed().as_secs_f64();
                        if any == 0 {
                            break;
                        }
                        total += spent;
                        worst = worst.max(spent);
                        callbacks += 1;
                    }
                    println!(
                        "{preset},{rate},{block},{voices},{:.2},{:.2},{prepare_us:.0}",
                        100.0 * total / callbacks as f64 / deadline,
                        100.0 * worst / deadline
                    );
                }
            }
        }
    }
    Ok(())
}
