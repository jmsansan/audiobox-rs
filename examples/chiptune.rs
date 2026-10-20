//! Renders a short 8-bit song to a WAV file.
//!
//! ```sh
//! cargo run --example chiptune --features synth -- song.wav
//! ```
use audiobox::synth::Score;
use audiobox::{EncodeOptions, write_audio_file};

const SONG: &str = "
tempo 140
ticks 48

instrument lead  wave=pulse duty=25,50 duty-step=60 adsr=4,90,60,60 vibrato=14,170,220
instrument harm  wave=pulse duty=12.5 adsr=2,40,40,30 volume=9 arpeggio=0,4,7 arpeggio-step=33
instrument bass  wave=triangle adsr=2,0,100,25
instrument kick  wave=triangle adsr=0,90,0,0 slide=-6000
instrument hat   wave=noise noise=long adsr=0,35,0,0 volume=7
instrument snare wave=noise adsr=0,120,0,0 volume=11

pattern hook = e8 g8 >c4< b8 g8 e4 d8 f8 a4 g8 f8 d4
pattern beat = [@kick o3 c8 @hat o7 c8 @snare o5 c8 @hat o7 c8]2

channel lead  = @lead o5 q7 L [$hook]2 e8 g8 >c8 e8 d4 c4< b2. r4 $hook
channel harm  = @harm o4 l2 L [c f g c]2 [c f g g]2
channel bass  = @bass o2 l8 L [[c c >c< c]2 [f f >f< f]2 [g g >g< g]2 [c c >c< c]2]2
channel drums = L [$beat]8
";

fn main() -> audiobox::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "chiptune.wav".into());
    let score: Score = SONG.parse()?;
    let audio = score.render(44_100)?;
    write_audio_file(&path, &audio, &EncodeOptions::default())?;
    println!(
        "Wrote {path}: {:.1} s, {} channels in the score",
        audio.duration(),
        score.channels().len()
    );
    Ok(())
}
