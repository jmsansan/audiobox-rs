export const $ = (id) => document.getElementById(id);
// Dark is the default theme; the choice is remembered. The page's head applies it
// before the first paint, and this keeps the toggle, the browser color and the
// waveforms in step.
const themeColors = { dark: '#0e1324', light: '#edf1fb' };
export const theme = () => (document.documentElement.dataset.theme === 'light' ? 'light' : 'dark');
function applyTheme(next, save) {
  document.documentElement.dataset.theme = next;
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', themeColors[next]);
  $('theme-toggle')?.setAttribute('aria-label', next === 'dark' ? 'Switch to light theme' : 'Switch to dark theme');
  if (save) { try { localStorage.setItem('audiobox-theme', next); } catch { /* Private browsing. */ } }
  window.dispatchEvent(new Event('themechange'));
}
applyTheme(theme(), false);
$('theme-toggle')?.addEventListener('click', () => applyTheme(theme() === 'dark' ? 'light' : 'dark', true));
const cssColor = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
const escaped = (text) => text.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
export function highlight(element, text = element.textContent) {
  const tokens = /("(?:\\.|[^"\\])*"|\/\/[^\n]*|\b(?:use|fn|let|mut|pub|for|in|if|else|return|impl|match|as|const)\b|\b(?:Audio|AudioFormat|Result|Vec|NormalizeOptions|NormalizeUnit|EncodeOptions|DecodeOptions|Limits|FadeCurve|FilterOptions|FilterType|TimePosition|SampleFormat|Endian|PcmDecoder|PcmEncoder|Resampler|Decoder|Effect|Preset|Score|Synth|RenderOptions|Mixer|MixerConfig|Play|Fade|Loop|Arc|Default)\b|\b\d[\d_.]*(?:f32|f64|u32)?\b)/g;
  let cursor = 0;
  let html = '';
  for (const match of text.matchAll(tokens)) {
    html += escaped(text.slice(cursor, match.index));
    const token = match[0];
    const kind = token.startsWith('//') ? 'comment' : token.startsWith('"') ? 'string' : /^\d/.test(token) ? 'number' : /^[A-Z]/.test(token) ? 'type' : 'keyword';
    html += `<span class="tok-${kind}">${escaped(token)}</span>`;
    cursor = match.index + token.length;
  }
  element.innerHTML = html + escaped(text.slice(cursor));
}
let noticeTimer;
export function notice(message) {
  $('notice').textContent = message;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => { $('notice').textContent = ''; }, 4500);
}
export function setupCopy() {
  document.querySelectorAll('[data-copy], [data-copy-target]').forEach((button) => {
    button.addEventListener('click', async () => {
      const source = button.dataset.copyTarget && $(button.dataset.copyTarget);
      // Text areas hold what the reader typed in `value`, not `textContent`.
      const content = button.dataset.copy ?? source.value ?? source.textContent;
      try {
        await navigator.clipboard.writeText(content);
        notice('Copied to clipboard.');
      } catch {
        const target = source || button;
        if (target.select) { target.select(); notice('Copy is unavailable here. The text is selected for manual copying.'); return; }
        const selection = window.getSelection();
        const range = document.createRange();
        range.selectNodeContents(target);
        selection.removeAllRanges();
        selection.addRange(range);
        notice('Copy is unavailable here. The text is selected for manual copying.');
      }
    });
  });
}
export class Engine {
  constructor() {
    this.id = 0;
    this.pending = new Map();
    this.worker = new Worker(new URL('./engine-worker.js', import.meta.url), { type: 'module' });
    this.worker.onmessage = ({ data }) => {
      const request = this.pending.get(data.id);
      if (!request) return;
      this.pending.delete(data.id);
      if (data.error) request.reject(new Error(data.error));
      else request.resolve(data.result);
    };
    this.worker.onerror = () => {
      this.pending.forEach(({ reject }) => reject(new Error('The audio engine could not start. Reload the page or try a current browser.')));
      this.pending.clear();
    };
  }
  request(type, payload = {}, transfer = []) {
    return new Promise((resolve, reject) => {
      const id = ++this.id;
      this.pending.set(id, { resolve, reject });
      this.worker.postMessage({ id, type, ...payload }, transfer);
    });
  }
}
/** Draws a waveform in the color of a CSS variable, such as `--ink` or `--accent`. */
export function drawWave(canvas, samples, colorVar) {
  const paint = () => {
    const color = cssColor(colorVar);
    const rect = canvas.getBoundingClientRect();
    if (!rect.width) return;
    const scale = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.round(rect.width * scale);
    canvas.height = Math.round(rect.height * scale);
    const ctx = canvas.getContext('2d');
    ctx.scale(scale, scale);
    ctx.clearRect(0, 0, rect.width, rect.height);
    ctx.strokeStyle = cssColor('--signal-grid');
    ctx.lineWidth = 0.6;
    ctx.beginPath();
    ctx.moveTo(0, rect.height / 2);
    ctx.lineTo(rect.width, rect.height / 2);
    ctx.stroke();
    if (!samples?.length) return;
    ctx.strokeStyle = color;
    ctx.lineWidth = Math.max(1, rect.width / 512 * 0.55);
    ctx.beginPath();
    for (let i = 0; i < 512; i++) {
      const x = (i + 0.5) / 512 * rect.width;
      const min = Math.max(-1, Math.min(1, samples[i * 2]));
      const max = Math.max(-1, Math.min(1, samples[i * 2 + 1]));
      ctx.moveTo(x, rect.height / 2 - max * rect.height * 0.43);
      ctx.lineTo(x, rect.height / 2 - min * rect.height * 0.43);
    }
    ctx.stroke();
  };
  paint();
  if (canvas._waveObserver) canvas._waveObserver.disconnect();
  if (canvas._wavePaint) window.removeEventListener('themechange', canvas._wavePaint);
  canvas._waveObserver = new ResizeObserver(paint);
  canvas._waveObserver.observe(canvas);
  canvas._wavePaint = paint;
  window.addEventListener('themechange', paint);
}
const playIcon = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m9 5 10 7-10 7Z"/></svg>';
const pauseIcon = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 5v14M16 5v14"/></svg>';
const clock = (seconds) => `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, '0')}`;
const players = new Set();
export class Player {
  constructor(button, time, head, label) {
    this.button = button; this.time = time; this.head = head; this.label = label;
    this.audio = new Audio();
    this.audio.preload = 'auto';
    players.add(this);
    button.addEventListener('click', () => (this.audio.paused ? this.play() : this.pause()));
    this.audio.addEventListener('ended', () => { this.pause(); this.audio.currentTime = 0; this.render(); });
    this.audio.addEventListener('error', () => notice('This browser could not play the WAV preview. Download it to listen locally.'));
  }
  /** Plays from the current position, or from the start with `restart`. */
  async play(restart = false) {
    players.forEach((p) => { if (p !== this) p.pause(); });
    try {
      if (restart) this.audio.currentTime = 0;
      await this.audio.play();
      this.button.innerHTML = pauseIcon;
      this.button.setAttribute('aria-label', `Pause ${this.label}`);
      this.head.classList.add('playing');
      this.animate();
    } catch { notice('Playback failed. Try again or download the audio.'); }
  }
  set(bytes, duration) {
    this.pause();
    if (this.url) URL.revokeObjectURL(this.url);
    this.url = URL.createObjectURL(new Blob([bytes], { type: 'audio/wav' }));
    this.audio.src = this.url;
    this.duration = duration;
    this.button.disabled = false;
    this.render();
  }
  render() {
    const position = this.audio.currentTime || 0;
    this.time.textContent = `${clock(position)} / ${clock(this.duration || 0)}`;
    this.head.style.left = `${Math.min(100, position / (this.duration || 1) * 100)}%`;
  }
  animate() {
    this.render();
    if (!this.audio.paused) this.frame = requestAnimationFrame(() => this.animate());
  }
  pause() {
    this.audio.pause();
    cancelAnimationFrame(this.frame);
    this.button.innerHTML = playIcon;
    this.button.setAttribute('aria-label', `Play ${this.label}`);
    this.head.classList.remove('playing');
  }
}
export function statsText(stats) {
  return `${(stats.rate / 1000).toLocaleString('en-US')} kHz · ${stats.channels === 1 ? 'mono' : 'stereo'} · ${stats.duration.toFixed(2)} s`;
}
export const defaults = { gain: -3, fadeIn: 0.3, fadeOut: 0.6, filter: 0, frequency: 2000, mono: false, reverse: false, rate: 0, start: 0, end: 4, normalize: false };
