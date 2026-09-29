import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import { test } from 'node:test';
const bytes = await readFile(new URL('../../_site/assets/sandbox.wasm', import.meta.url));
const { instance } = await WebAssembly.instantiate(bytes, {});
const w = instance.exports;
function output() {
  return new Uint8Array(w.memory.buffer, w.app_output_ptr(), w.app_output_len()).slice();
}
function error() {
  return new TextDecoder().decode(new Uint8Array(w.memory.buffer, w.app_error_ptr(), w.app_error_len()));
}
function process({ gain = 0, fadeIn = 0, fadeOut = 0, filter = 0, frequency = 2000,
  mono = 0, reverse = 0, rate = 0, start = 0, end = 4, normalize = 0 } = {}) {
  return w.app_process(gain, fadeIn, fadeOut, filter, frequency, mono, reverse, rate, start, end, normalize);
}
test('Real Rust demo audio, processing and all three export formats', () => {
  for (const kind of [1, 2, 3]) {
    assert.equal(w.app_load(kind), 1, error());
    assert.equal(w.app_stat(1, 0), 44100);
    assert.equal(w.app_stat(1, 1), 176400);
    assert.equal(w.app_stat(1, 2), 2);
    assert.equal(process({ gain: -6, fadeIn: 0.3, fadeOut: 0.4, mono: 1, reverse: 1, rate: 48000 }), 1, error());
    assert.equal(w.app_stat(0, 0), 48000);
    assert.equal(w.app_stat(0, 1), 192000);
    assert.equal(w.app_stat(0, 2), 1);
    assert.ok(w.app_stat(0, 4) < w.app_stat(1, 4));
    assert.equal(w.app_stat(1, 2), 2, 'Source remains immutable');
    const wavePtr = w.app_wave(0);
    const wave = new Float32Array(w.memory.buffer, wavePtr, 1024).slice();
    assert.ok(wave.every(Number.isFinite));
    for (let i = 0; i < 512; i++) assert.ok(wave[i * 2] <= wave[i * 2 + 1]);
    for (const format of [0, 1, 2]) {
      assert.equal(w.app_encode(0, format), 1, error());
      const encoded = output();
      if (format === 0) assert.equal(new TextDecoder().decode(encoded.slice(0, 4)), 'RIFF');
      if (format === 1) assert.equal(new TextDecoder().decode(encoded.slice(0, 4)), 'fLaC');
      if (format === 2) assert.equal(encoded[0], 0xff);
      const ptr = w.app_input(encoded.length);
      new Uint8Array(w.memory.buffer, ptr, encoded.length).set(encoded);
      assert.equal(w.app_load(0), 1, error());
      assert.equal(w.app_stat(1, 0), 48000);
      assert.equal(w.app_stat(1, 2), 1);
      assert.equal(w.app_stat(1, 1), 192000);
    }
  }
});
test('Trim, filters and normalization run through the library', () => {
  assert.equal(w.app_load(1), 1);
  assert.equal(process({ start: 0.5, end: 2.5, filter: 1, frequency: 1000, normalize: 1 }), 1, error());
  assert.equal(w.app_stat(0, 3), 2);
  assert.ok(Math.abs(w.app_stat(0, 4) - -1) < 0.01);
  assert.equal(process({ filter: 2, frequency: 80 }), 1, error());
});
test('Invalid files and operations return recoverable errors', () => {
  assert.equal(w.app_input(16 * 1024 * 1024 + 1), 0);
  const ptr = w.app_input(4);
  new Uint8Array(w.memory.buffer, ptr, 4).set([0, 1, 2, 3]);
  assert.equal(w.app_load(0), 0);
  assert.ok(error().length > 0);
  assert.equal(w.app_load(1), 1);
  assert.equal(process({ start: 3, end: 1 }), 0);
  assert.match(error(), /empty|end|range/i);
  assert.equal(process({ rate: 16000, filter: 1, frequency: 8000 }), 0);
  assert.match(error(), /frequency|nyquist/i);
  assert.equal(process({ rate: 16000 }), 1);
  assert.equal(w.app_encode(0, 2), 0);
  assert.ok(error().length > 0);
  assert.equal(process(), 1, 'A bad operation does not poison the next request');
});
