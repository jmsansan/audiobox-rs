const boot = (async () => {
  if (typeof WebAssembly === 'undefined') throw new Error('Your browser does not support WebAssembly. Try a current browser.');
  const response = await fetch(new URL('./sandbox.wasm', import.meta.url));
  if (!response.ok) throw new Error('The Rust audio engine could not be downloaded. Reload the page to retry.');
  const { instance } = await WebAssembly.instantiate(await response.arrayBuffer(), {});
  return instance.exports;
})();
function check(wasm, ok) {
  if (ok) return;
  const message = new TextDecoder().decode(new Uint8Array(wasm.memory.buffer, wasm.app_error_ptr(), wasm.app_error_len()));
  throw new Error(message || 'Audio processing failed. Try another file or reset the controls.');
}
function result(wasm, original = false) {
  const source = original ? 1 : 0;
  const stats = {};
  ['rate', 'frames', 'channels', 'duration', 'peak', 'rms'].forEach((key, i) => { stats[key] = wasm.app_stat(source, i); });
  const ptr = wasm.app_wave(source);
  const wave = new Float32Array(wasm.memory.buffer, ptr, 1024).slice();
  check(wasm, wasm.app_encode(source, 0));
  const bytes = new Uint8Array(wasm.memory.buffer, wasm.app_output_ptr(), wasm.app_output_len()).slice().buffer;
  return { stats, wave, bytes };
}
self.onmessage = async ({ data }) => {
  try {
    const wasm = await boot;
    let value;
    if (data.type === 'load') {
      if (data.bytes) {
        const input = new Uint8Array(data.bytes);
        if (!input.length) throw new Error('The file is empty. Choose an audio file with samples.');
        if (input.length > 16 * 1024 * 1024) throw new Error('The file exceeds the 16 MB sandbox limit. Choose a smaller clip.');
        const ptr = wasm.app_input(input.length);
        if (!ptr) throw new Error('The input could not be allocated. Choose a smaller file.');
        new Uint8Array(wasm.memory.buffer, ptr, input.length).set(input);
      }
      check(wasm, wasm.app_load(data.bytes ? 0 : data.kind));
      value = result(wasm, true);
      if (!value.stats.frames) throw new Error('The audio has no samples. Choose a different file.');
    } else if (data.type === 'process') {
      const c = data.config;
      check(wasm, wasm.app_process(c.gain, c.fadeIn, c.fadeOut, c.filter, c.frequency, +c.mono, +c.reverse, c.rate, c.start, c.end, +c.normalize));
      value = result(wasm);
    } else if (data.type === 'export') {
      check(wasm, wasm.app_encode(0, data.format));
      value = { bytes: new Uint8Array(wasm.memory.buffer, wasm.app_output_ptr(), wasm.app_output_len()).slice().buffer };
    } else throw new Error('Unknown audio operation.');
    const transfer = [value.bytes];
    if (value.wave) transfer.push(value.wave.buffer);
    self.postMessage({ id: data.id, result: value }, transfer);
  } catch (error) {
    self.postMessage({ id: data.id, error: error.message || 'The engine could not process this audio. Reload to retry.' });
  }
};
