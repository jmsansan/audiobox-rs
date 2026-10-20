import { $, Engine, Player, drawWave, highlight, notice, setupCopy } from './common.js';
import {
  cheats, defaultEffect, effectRust, effectText, fields, parseEffect, presetLabels, scoreRust, songs,
} from './playground-code.js';

setupCopy();
const engine = new Engine();
const sfxPlayer = new Player($('sfx-play'), $('sfx-time'), $('sfx-playhead'), 'effect');
const songPlayer = new Player($('song-play'), $('song-time'), $('song-playhead'), 'song');
let effect = defaultEffect();
let preset = null; // { kind, seed } while the effect is exactly a preset.
let sfxBytes = null;
let songBytes = null;
let rendering = Promise.resolve();

function status(id, message, error = false) {
  $(id).textContent = message;
  $(id).classList.toggle('error', error);
}
function download(bytes, name) {
  const url = URL.createObjectURL(new Blob([bytes], { type: 'audio/wav' }));
  const link = document.createElement('a');
  link.href = url; link.download = name;
  document.body.append(link); link.click(); link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60000);
}
function show(prefix, player, result) {
  drawWave($(`${prefix}-wave`), result.wave, '--accent');
  const peak = result.stats.peak;
  $(`${prefix}-meta`).textContent = `44.1 kHz · ${result.stats.duration.toFixed(2)} s`;
  $(`${prefix}-peak`).textContent = `Peak ${Number.isFinite(peak) ? peak.toFixed(1) : '−∞'} dBFS`;
  $(`${prefix}-peak`).classList.toggle('clipped', peak > 0);
  player.set(result.bytes, result.stats.duration);
}

// Sound effects -------------------------------------------------------------

const log = (f) => f.scale === 'log';
function toSlider(f, value) {
  if (f.off && value === 0) return f.off === 'max' ? 1000 : 0;
  if (log(f)) return Math.round(Math.log(Math.min(Math.max(value, f.min), f.max) / f.min) / Math.log(f.max / f.min) * 1000);
  return Math.min(Math.max(value, f.min), f.max);
}
function fromSlider(f, position) {
  if (log(f)) {
    if ((f.off === 'max' && position >= 1000) || (f.off === 'min' && position <= 0)) return 0;
    return Number((f.min * (f.max / f.min) ** (position / 1000)).toFixed(1));
  }
  const value = Number((+position).toFixed(3));
  if (f.floor && value > 0 && value < f.floor) return f.floor;
  return value;
}
function label(f, value) {
  if (f.off && value === 0) return 'Off';
  if (f.zero && value === 0) return f.zero;
  const digits = log(f) ? 0 : f.step >= 1 ? 0 : f.step >= 0.05 ? 2 : 3;
  const number = value.toFixed(digits).replace('-', '−');
  return f.unit ? `${number} ${f.unit}` : number;
}
function buildSliders() {
  let group = null;
  let container;
  for (const f of fields) {
    if (f.group !== group) {
      group = f.group;
      container = document.createElement('div');
      container.className = 'control-group';
      container.innerHTML = `<h3>${group}</h3>`;
      $('sliders').append(container);
    }
    const control = document.createElement('div');
    control.className = 'control';
    control.id = `${f.key}-control`;
    const id = `fx-${f.key}`;
    const range = log(f) ? 'min="0" max="1000" step="1"' : `min="${f.min}" max="${f.max}" step="${f.step}"`;
    control.innerHTML = `<label for="${id}">${f.label}<output id="${id}-value"></output></label><input id="${id}" type="range" ${range}>`;
    container.append(control);
    const input = control.querySelector('input');
    input.addEventListener('input', () => {
      effect[f.key] = fromSlider(f, +input.value);
      $(`${id}-value`).textContent = label(f, effect[f.key]);
      edited();
    });
    input.addEventListener('change', () => renderEffect(true));
  }
}
function syncControls() {
  $('wave').value = effect.wave;
  $('duty').value = String(effect.duty);
  $('noise').value = effect.noise;
  $('duty-control').hidden = effect.wave !== 'pulse';
  $('noise-control').hidden = effect.wave !== 'noise';
  for (const f of fields) {
    $(`fx-${f.key}`).value = toSlider(f, effect[f.key]);
    $(`fx-${f.key}-value`).textContent = label(f, effect[f.key]);
    if (f.pulse) $(`${f.key}-control`).hidden = effect.wave !== 'pulse';
  }
  document.querySelectorAll('.preset').forEach((button) => {
    button.setAttribute('aria-pressed', preset !== null && +button.dataset.kind === preset.kind);
  });
  $('seed-hint').textContent = preset ? `${presetLabels[preset.kind]} #${preset.seed}` : 'Edited: the text below is the effect';
  $('sfx-text').value = effectText(effect);
  highlight($('sfx-rust'), effectRust(effect, preset));
}
function edited() {
  preset = null;
  if (effect.wave !== 'pulse') effect['duty-sweep'] = 0;
  $('sfx-text').value = effectText(effect);
  $('seed-hint').textContent = 'Edited: the text below is the effect';
  document.querySelectorAll('.preset').forEach((button) => button.setAttribute('aria-pressed', 'false'));
  highlight($('sfx-rust'), effectRust(effect, null));
}
async function renderEffect(play) {
  const text = effectText(effect);
  rendering = rendering.then(async () => {
    try {
      const result = await engine.request('sfx-render', { text, naive: false });
      show('sfx', sfxPlayer, result);
      sfxBytes = result.bytes.slice(0);
      $('sfx-download').disabled = false;
      status('sfx-status', 'Rendered in Rust. Adjust a control or try another seed.');
      if (play && $('autoplay').checked) sfxPlayer.play(true);
    } catch (error) {
      status('sfx-status', error.message, true);
    }
  });
  return rendering;
}
async function loadPreset(kind, seed, play = true) {
  try {
    const { text } = await engine.request('sfx-preset', { kind, seed });
    effect = parseEffect(text);
    preset = { kind, seed };
    $('seed').value = seed;
    syncControls();
    await renderEffect(play);
  } catch (error) { status('sfx-status', error.message, true); }
}
const randomSeed = () => Math.floor(Math.random() * 100000);
function setupEffects() {
  presetLabels.forEach((name, kind) => {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'button secondary small preset';
    button.dataset.kind = kind;
    button.textContent = name;
    button.setAttribute('aria-pressed', 'false');
    button.addEventListener('click', () => loadPreset(kind, randomSeed()));
    $('presets').append(button);
  });
  buildSliders();
  $('new-seed').addEventListener('click', () => loadPreset(preset?.kind ?? 0, randomSeed()));
  $('seed').addEventListener('change', () => {
    const seed = Math.max(0, Math.floor(+$('seed').value || 0));
    loadPreset(preset?.kind ?? 0, seed);
  });
  for (const id of ['wave', 'duty', 'noise']) {
    $(id).addEventListener('change', () => {
      effect[id] = $(id).value;
      edited();
      syncControls();
      renderEffect(true);
    });
  }
  $('sfx-controls').addEventListener('submit', (event) => event.preventDefault());
  $('sfx-reset').addEventListener('click', () => { effect = defaultEffect(); preset = null; syncControls(); renderEffect(true); });
  const applyText = () => {
    // Render the text as typed, so Rust reports any mistake in it; the controls
    // take its values only once Rust accepts it.
    const text = $('sfx-text').value;
    rendering = rendering.then(async () => {
      try {
        const result = await engine.request('sfx-render', { text, naive: false });
        effect = parseEffect(text);
        preset = null;
        syncControls();
        $('sfx-text').value = text;
        show('sfx', sfxPlayer, result);
        sfxBytes = result.bytes.slice(0);
        status('sfx-status', 'Applied your text.');
        if ($('autoplay').checked) sfxPlayer.play(true);
      } catch (error) { status('sfx-status', error.message, true); }
    });
  };
  $('sfx-apply').addEventListener('click', applyText);
  $('sfx-text').addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); applyText(); }
  });
  $('sfx-download').addEventListener('click', () => {
    if (!sfxBytes) return;
    download(sfxBytes, preset ? `${presetLabels[preset.kind].toLowerCase()}-${preset.seed}.wav` : 'effect.wav');
    notice('Your effect is ready to download.');
  });
}

// Chiptune scores -------------------------------------------------------------

function selectError(message) {
  const match = /line (\d+), column (\d+)/.exec(message);
  if (!match) return;
  const lines = $('score-text').value.split('\n');
  const line = Math.min(+match[1], lines.length) - 1;
  let offset = lines.slice(0, line).reduce((n, l) => n + l.length + 1, 0);
  offset += Math.min(+match[2] - 1, lines[line].length);
  $('score-text').focus();
  $('score-text').setSelectionRange(offset, Math.min(offset + 1, $('score-text').value.length));
}
async function renderSong(play) {
  $('score-play').disabled = true;
  status('score-status', 'Rendering in Rust…');
  try {
    const result = await engine.request('synth-render', {
      text: $('score-text').value, naive: $('score-aliasing').value === '1', loops: +$('loops').value,
    });
    show('song', songPlayer, result);
    songBytes = result.bytes.slice(0);
    $('score-download').disabled = false;
    status('score-status', `Rendered ${result.stats.duration.toFixed(1)} s in Rust.`);
    if (play) songPlayer.play(true);
  } catch (error) {
    status('score-status', error.message, true);
    selectError(error.message);
  }
  $('score-play').disabled = false;
}
function setupScores() {
  for (const [key, song] of Object.entries(songs)) $('song').add(new Option(song.label, key));
  $('score-text').value = songs.overworld.text;
  $('cheats').innerHTML = cheats.map(([command, meaning]) => `<code>${command}</code><span>${meaning}</span>`).join('');
  highlight($('score-rust'), scoreRust());
  $('song').addEventListener('change', () => { $('score-text').value = songs[$('song').value].text; renderSong(true); });
  for (const id of ['loops', 'score-aliasing']) $(id).addEventListener('change', () => renderSong(false));
  $('score-controls').addEventListener('submit', (event) => event.preventDefault());
  $('score-play').addEventListener('click', () => renderSong(true));
  $('score-text').addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); renderSong(true); }
  });
  $('score-download').addEventListener('click', () => {
    if (!songBytes) return;
    download(songBytes, 'song.wav');
    notice('Your song is ready to download.');
  });
}

setupEffects();
setupScores();
await loadPreset(5, 7, false);
$('effects').setAttribute('aria-busy', 'false');
await renderSong(false);
$('chiptune').setAttribute('aria-busy', 'false');
