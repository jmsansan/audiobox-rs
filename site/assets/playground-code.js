// Effect parameters, their text form and the equivalent Rust. Shared by the
// playground and the site build, which compiles the generated Rust.

export const presets = ['Pickup', 'Laser', 'Explosion', 'PowerUp', 'Hit', 'Jump', 'Blip'];
export const presetLabels = ['Pickup', 'Laser', 'Explosion', 'Power-up', 'Hit', 'Jump', 'Blip'];

// Slider ranges are a comfortable subset of the library's limits. `scale: 'log'`
// sliders move in equal ratios; `off` marks the slider end that disables a filter.
export const fields = [
  { group: 'Pitch', key: 'frequency', label: 'Frequency', min: 20, max: 5000, scale: 'log', unit: 'Hz', def: 440 },
  { group: 'Pitch', key: 'slide', label: 'Slide', min: -16, max: 16, step: 0.05, unit: 'oct/s', def: 0 },
  { group: 'Pitch', key: 'slide-change', label: 'Slide change', min: -64, max: 64, step: 0.5, unit: 'oct/s²', def: 0 },
  { group: 'Pitch', key: 'vibrato-depth', label: 'Vibrato depth', min: 0, max: 12, step: 0.05, unit: 'st', def: 0 },
  { group: 'Pitch', key: 'vibrato-speed', label: 'Vibrato speed', min: 0, max: 40, step: 0.1, unit: 'Hz', def: 0 },
  { group: 'Pitch', key: 'arpeggio', label: 'Arpeggio', min: -24, max: 24, step: 1, unit: 'st', def: 0 },
  { group: 'Pitch', key: 'arpeggio-time', label: 'Arpeggio at', min: 0, max: 1, step: 0.005, unit: 's', def: 0 },
  { group: 'Pitch', key: 'repeat', label: 'Repeat every', min: 0, max: 1, step: 0.005, unit: 's', def: 0, zero: 'Off', floor: 0.01 },
  { group: 'Envelope', key: 'attack', label: 'Attack', min: 0, max: 1, step: 0.005, unit: 's', def: 0 },
  { group: 'Envelope', key: 'sustain', label: 'Sustain', min: 0, max: 1.5, step: 0.005, unit: 's', def: 0.1 },
  { group: 'Envelope', key: 'punch', label: 'Punch', min: 0, max: 1, step: 0.01, unit: '', def: 0 },
  { group: 'Envelope', key: 'decay', label: 'Decay', min: 0, max: 2, step: 0.005, unit: 's', def: 0.2 },
  { group: 'Tone', key: 'duty-sweep', label: 'Duty sweep', min: -4, max: 4, step: 0.05, unit: '/s', def: 0, pulse: true },
  { group: 'Tone', key: 'volume', label: 'Volume', min: 0, max: 1, step: 0.01, unit: '', def: 0.5 },
  { group: 'Filters', key: 'low-pass', label: 'Low-pass', min: 100, max: 20000, scale: 'log', unit: 'Hz', def: 0, off: 'max' },
  { group: 'Filters', key: 'low-pass-sweep', label: 'Low-pass sweep', min: -8, max: 8, step: 0.05, unit: 'oct/s', def: 0 },
  { group: 'Filters', key: 'high-pass', label: 'High-pass', min: 20, max: 8000, scale: 'log', unit: 'Hz', def: 0, off: 'min' },
  { group: 'Filters', key: 'high-pass-sweep', label: 'High-pass sweep', min: -8, max: 8, step: 0.05, unit: 'oct/s', def: 0 },
];

export const defaultEffect = () => ({
  wave: 'pulse', duty: '50', noise: 'long',
  ...Object.fromEntries(fields.map((f) => [f.key, f.def])),
});

const round = (value, digits = 3) => Number(value.toFixed(digits));

/** Reads the `key=value` text form into an effect, keeping unknown text for Rust to report. */
export function parseEffect(text) {
  const effect = defaultEffect();
  for (const line of text.split('\n')) {
    for (const word of line.split(';')[0].split(/\s+/)) {
      const [key, value] = word.split('=');
      if (value === undefined) continue;
      if (['wave', 'duty', 'noise'].includes(key)) effect[key] = value;
      else if (key in effect && Number.isFinite(+value)) effect[key] = +value;
    }
  }
  return effect;
}

/** Writes an effect in the library's text form: the waveform, then every parameter that is set. */
export function effectText(effect) {
  const lines = [effect.wave === 'pulse' ? `wave=pulse duty=${effect.duty}`
    : effect.wave === 'noise' ? `wave=noise noise=${effect.noise}` : `wave=${effect.wave}`];
  const always = ['frequency', 'attack', 'sustain', 'decay', 'volume'];
  for (const f of fields) {
    const value = effect[f.key];
    if (f.pulse && effect.wave !== 'pulse') continue;
    if (always.includes(f.key) || value !== 0) lines.push(`${f.key}=${round(value)}`);
  }
  return lines.join('\n');
}

/** Rust that renders the effect: from its preset and seed when unchanged, otherwise from its text. */
export function effectRust(effect, preset) {
  if (preset) {
    return `use audiobox::sfx::{Effect, Preset};

fn main() -> audiobox::Result<()> {
    let effect = Effect::preset(Preset::${presets[preset.kind]}, ${preset.seed});
    let audio = effect.render(44_100)?;
    audiobox::write_audio_file("${presets[preset.kind].toLowerCase()}.wav", &audio, &Default::default())?;
    Ok(())
}`;
  }
  const text = effectText(effect).replaceAll('\n', ' ');
  return `use audiobox::sfx::Effect;

fn main() -> audiobox::Result<()> {
    // Keep effects as text in your repository and parse them at load time.
    let effect: Effect = "${text}".parse()?;
    let audio = effect.render(44_100)?;
    audiobox::write_audio_file("effect.wav", &audio, &Default::default())?;
    Ok(())
}`;
}

export function scoreRust() {
  return `use audiobox::synth::{RenderOptions, Score, Synth};

fn main() -> audiobox::Result<()> {
    let score: Score = std::fs::read_to_string("song.txt")?.parse()?;
    // Offline, into Audio:
    let audio = score.render(44_100)?;
    audiobox::write_audio_file("song.wav", &audio, &Default::default())?;
    // Or chunk by chunk, looping forever, for a game's mixer:
    let options = RenderOptions { loops: None, ..RenderOptions::default() };
    let mut synth = Synth::new(&score, 48_000, &options)?;
    let mut buffer = [0.0f32; 512];
    synth.fill(&mut buffer);
    Ok(())
}`;
}

export const songs = {
  overworld: { label: 'Overworld', text: `; An overworld theme: lead, harmony, bass and drums.
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
` },
  cave: { label: 'Cave', text: `; A slow minor theme with a short-noise drip.
tempo 96

instrument pad  wave=triangle adsr=80,200,70,300 vibrato=8,240,400
instrument lead wave=pulse duty=12.5 adsr=10,120,50,200 volume=10 vibrato=20,200,300
instrument drip wave=noise noise=short adsr=0,60,0,0 volume=6

channel pad  = @pad o3 l1 L a f g e a e
channel lead = @lead o5 l4 L r1 a8 b8 >c4 e4 d4< b2 a4 g4 a1 r2 e4 g4 a2. r4
channel drip = @drip o6 l8 L [r4 c r r2 r4 >c< r]4
` },
  fanfare: { label: 'Victory', text: `; A short victory fanfare that does not loop.
tempo 160

instrument brass wave=saw adsr=5,80,70,120 volume=11 vibrato=10,150,150
instrument harm  wave=pulse duty=25 adsr=5,80,60,120 volume=8
instrument bass  wave=triangle adsr=2,0,100,60

channel brass = @brass o5 l16 c c c c4. <a-4 b-4 >c8. <b-16 >c2
channel harm  = @harm o4 l16 e e e e4. f4 g4 a8. g16 a2
channel bass  = @bass o3 l4 c c f g c2
` },
};

export const cheats = [
  ['c d e f g a b', 'Notes; + or # sharp, - flat'],
  ['r', 'Rest'],
  ['4 8. %12', 'Quarter, dotted eighth, 12 ticks'],
  ['^8   c4&c8', 'Ties'],
  ['o4 > <', 'Octave, up, down'],
  ['l8 v12 q6', 'Default length, volume, gate'],
  ['@lead', 'Select an instrument'],
  ['[ … | … ]3', 'Repeat; skip after | the last time'],
  ['$name', 'Play a pattern'],
  ['L', 'Loop point'],
];
