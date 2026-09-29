import { examples } from '../assets/examples.js';
import { rustCode } from '../assets/rust-code.js';
const snippets = Object.values(examples).map((example) => example.code);
const config = { gain: -3, fadeIn: 0.3, fadeOut: 0.6, filter: 0, frequency: 2000,
  mono: false, reverse: false, normalize: false, rate: 0, start: 0, end: 4 };
for (const format of [0, 1, 2]) {
  snippets.push(rustCode(config, { format }));
  snippets.push(rustCode({ ...config, filter: 1, mono: true, reverse: true,
    normalize: true, rate: 48000 }, { format, isFile: true, filename: 'my "recording".wav' }));
}
snippets.push(rustCode({ ...config, fadeIn: 0, fadeOut: 0, filter: 2 }, { format: 0 }));
process.stdout.write(JSON.stringify(snippets));
