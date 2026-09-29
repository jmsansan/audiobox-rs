import { examples } from './examples.js';
import { $, Engine, Player, defaults, drawWave, highlight, setupCopy } from './common.js';
setupCopy();

function selectExample(button) {
  document.querySelectorAll('[data-example]').forEach((tab) => {
    const selected = tab === button;
    tab.setAttribute('aria-selected', selected);
    tab.tabIndex = selected ? 0 : -1;
  });
  const example = examples[button.dataset.example];
  $('example-file').textContent = example.file;
  $('example-description').textContent = example.description;
  $('example-panel').setAttribute('aria-labelledby', button.id);
  highlight($('example-code'), example.code);
}
const tabs = [...document.querySelectorAll('[data-example]')];
tabs.forEach((button, i) => {
  button.addEventListener('click', () => selectExample(button));
  button.addEventListener('keydown', (event) => {
    let next;
    if (['ArrowDown', 'ArrowRight'].includes(event.key)) next = (i + 1) % tabs.length;
    if (['ArrowUp', 'ArrowLeft'].includes(event.key)) next = (i - 1 + tabs.length) % tabs.length;
    if (event.key === 'Home') next = 0;
    if (event.key === 'End') next = tabs.length - 1;
    if (next !== undefined) { event.preventDefault(); selectExample(tabs[next]); tabs[next].focus(); }
  });
});
highlight($('example-code'));
const engine = new Engine();
const player = new Player($('hero-play'), $('hero-time'), $('hero-playhead'), 'demo audio');
let original, faded;
function selectAudio(isFaded) {
  const selected = isFaded ? faded : original;
  player.set(selected.bytes, selected.stats.duration);
  drawWave($('hero-wave'), selected.wave, isFaded ? '#6841cb' : '#202a50');
  $('demo-original').setAttribute('aria-pressed', !isFaded);
  $('demo-faded').setAttribute('aria-pressed', isFaded);
  $('hero-operation').textContent = isFaded ? 'audio.fade(0.8, 1.2, FadeCurve::EqualPower)?' : 'audio.clone()';
}
try {
  original = await engine.request('load', { kind: 1 });
  faded = await engine.request('process', { config: { ...defaults, gain: 0, fadeIn: 0.8, fadeOut: 1.2 } });
  selectAudio(true);
  $('demo-original').disabled = $('demo-faded').disabled = false;
  $('demo-original').addEventListener('click', () => selectAudio(false));
  $('demo-faded').addEventListener('click', () => selectAudio(true));
  $('hero-status').textContent = 'Processed in your browser by audiobox + WebAssembly.';
} catch (error) {
  $('hero-status').textContent = `${error.message} You can still explore the examples below.`;
  $('hero-status').classList.add('error');
}
$('hero-demo').setAttribute('aria-busy', 'false');

const narrowTabs = matchMedia("(max-width: 720px)");
function tabDirection() { document.querySelector(".example-tabs").setAttribute("aria-orientation", narrowTabs.matches ? "horizontal" : "vertical"); }
narrowTabs.addEventListener("change", tabDirection);
tabDirection();
