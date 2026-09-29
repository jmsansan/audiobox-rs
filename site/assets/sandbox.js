import { rustCode } from './rust-code.js';
import { $, Engine, Player, defaults, drawWave, highlight, setupCopy, notice, statsText } from './common.js';
setupCopy();
const engine = new Engine();
const inputPlayer = new Player($('input-play'), $('input-time'), $('input-playhead'), 'original audio');
const outputPlayer = new Player($('output-play'), $('output-time'), $('output-playhead'), 'processed audio');
const controls = [...$('controls').querySelectorAll('input,select,button'), $('source'), $('upload')];
let duration = 4, filename = 'input.wav', isFile = false, appliedConfig, outputStats, busy = true, dirty = false;
function setBusy(value) {
  busy = value;
  controls.forEach((control) => { control.disabled = value; });
  $('input-play').disabled = value || !inputPlayer.url;
  $('output-play').disabled = value || !outputPlayer.url;
  $('download').disabled = value || dirty || !outputStats;
  $('sandbox').setAttribute('aria-busy', value);
}
function status(message, error = false) {
  $('sandbox-status').textContent = message;
  $('sandbox-status').classList.toggle('error', error);
}
function config() {
  return { gain: +$('gain').value, fadeIn: +$('fade-in').value, fadeOut: +$('fade-out').value,
    filter: +$('filter').value, frequency: +$('frequency').value, mono: $('mono').checked,
    reverse: $('reverse').checked, normalize: $('normalize').checked, rate: +$('rate').value,
    start: +$('trim-start').value, end: +$('trim-end').value };
}
function values() {
  $('gain-value').textContent = `${$('gain').value.replace('-', '−')} dB`;
  $('fade-in-value').textContent = `${(+$('fade-in').value).toFixed(1)} s`;
  $('fade-out-value').textContent = `${(+$('fade-out').value).toFixed(1)} s`;
  $('frequency-value').textContent = `${(+$('frequency').value).toLocaleString('en-US')} Hz`;
  $('frequency-control').hidden = $('filter').value === '0';
}
function resetValues() {
  $('gain').value = defaults.gain;
  $('fade-in').value = defaults.fadeIn;
  $('fade-out').value = defaults.fadeOut;
  $('filter').value = defaults.filter;
  $('frequency').value = defaults.frequency;
  $('rate').value = defaults.rate;
  $('trim-start').value = 0;
  $('trim-end').value = String(duration);
  ['mono', 'reverse', 'normalize'].forEach((id) => { $(id).checked = false; });
  values();
}
function showAudio(result, source) {
  const prefix = source ? 'input' : 'output';
  drawWave($(prefix + '-wave'), result.wave, source ? '#202a50' : '#6841cb');
  $(prefix + '-meta').textContent = statsText(result.stats);
  $(prefix + '-peak').textContent = `Peak ${Number.isFinite(result.stats.peak) ? result.stats.peak.toFixed(1) : '−∞'} dBFS`;
  $(prefix + '-rms').textContent = `RMS ${Number.isFinite(result.stats.rms) ? result.stats.rms.toFixed(1) : '−∞'} dBFS`;
  $(prefix + '-peak').classList.toggle('clipped', result.stats.peak > 0);
  (source ? inputPlayer : outputPlayer).set(result.bytes, result.stats.duration);
  if (!source) outputStats = result.stats;
}
function code(c) { highlight($("sandbox-code"), rustCode(c, { filename, isFile, format: +$("export-format").value })); }

async function apply() {
  const c = config();
  if (!Number.isFinite(c.start) || !Number.isFinite(c.end) || c.end <= c.start || c.start < 0 || c.end > duration + 0.0001) {
    status(`Choose a start and end between 0 and ${duration.toFixed(2)} seconds, with the end after the start.`, true);
    return;
  }
  inputPlayer.pause(); outputPlayer.pause();
  setBusy(true); status('Processing your audio in Rust…');
  try {
    const result = await engine.request('process', { config: c });
    showAudio(result, false);
    appliedConfig = c; dirty = false; code(c);
    status(result.stats.peak > 0 ? 'Ready. Peak exceeds 0 dBFS; reduce gain or normalize before export.' : 'Ready to listen or export.');
  } catch (error) { status(error.message, true); }
  setBusy(false);
}
async function load(payload, caption, selectedFile = null) {
  setBusy(true); inputPlayer.pause(); outputPlayer.pause(); status('Decoding audio in Rust…');
  try {
    const result = await engine.request('load', payload, payload.bytes ? [payload.bytes] : []);
    isFile = selectedFile !== null;
    filename = selectedFile || "input.wav";
    duration = result.stats.duration;
    ['trim-start', 'trim-end'].forEach((id) => { $(id).max = duration; });
    $('source-caption').textContent = caption;
    resetValues(); showAudio(result, true);
    await apply();
  } catch (error) {
    status(`${error.message} Try a WAV, FLAC or MP3 clip within the sandbox limits.`, true);
    setBusy(false);
  }
}
$('controls').addEventListener('submit', (event) => { event.preventDefault(); if (!busy) apply(); });
$('controls').addEventListener('input', () => {
  values(); dirty = true; $('download').disabled = true;
  status('Changes pending. Apply changes to hear the result.');
});
$('reset').addEventListener('click', () => { resetValues(); apply(); });
$('source').addEventListener('change', () => {
  load({ kind: +$('source').value }, `${$('source').selectedOptions[0].textContent} · generated demo audio. You can also drop an audio file here.`);
});
async function upload(file) {
  if (!file || busy) return;
  if (file.size > 16 * 1024 * 1024) { status('This file exceeds 16 MB. Choose a smaller clip.', true); return; }
  try {
    const bytes = await file.arrayBuffer();
    await load({ bytes }, `${file.name} · decoded locally. Your file stays on this device.`, file.name);
  } catch { status('The file could not be read. Choose it again or try another file.', true); }
  $('upload').value = '';
}
$('upload').addEventListener('change', () => upload($('upload').files[0]));
const zone = $('drop-zone');
zone.addEventListener('dragover', (event) => { event.preventDefault(); if (!busy) zone.classList.add('dragging'); });
zone.addEventListener('dragleave', (event) => { if (!zone.contains(event.relatedTarget)) zone.classList.remove('dragging'); });
zone.addEventListener('drop', (event) => { event.preventDefault(); zone.classList.remove('dragging'); upload(event.dataTransfer.files[0]); });
$('export-format').addEventListener('change', () => { if (appliedConfig) code(appliedConfig); });
$('download').addEventListener('click', async () => {
  if (busy || dirty) return;
  const format = +$('export-format').value;
  if (format === 2 && ![32000, 44100, 48000].includes(outputStats.rate)) {
    status('MP3 export needs 32, 44.1 or 48 kHz. Change the sample rate and apply again.', true); return;
  }
  setBusy(true); status('Encoding the download in Rust…');
  try {
    const { bytes } = await engine.request('export', { format });
    const type = ['audio/wav', 'audio/flac', 'audio/mpeg'][format];
    const extension = ['wav', 'flac', 'mp3'][format];
    const url = URL.createObjectURL(new Blob([bytes], { type }));
    const link = document.createElement('a');
    link.href = url; link.download = `audiobox-processed.${extension}`;
    document.body.append(link); link.click(); link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 60000);
    status(`Exported ${extension.toUpperCase()} (${(bytes.byteLength / 1024).toFixed(1)} KB).`);
    notice('Your processed audio is ready to download.');
  } catch (error) { status(error.message, true); }
  setBusy(false);
});
values();
await load({ kind: 1 }, 'Warm chord · generated demo audio. You can also drop an audio file here.');
