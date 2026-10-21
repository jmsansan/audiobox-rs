// Runs a WASI (preview 1) program, such as a test binary built for wasm32-wasip1,
// with Node's built-in WASI support. Used as Cargo's test runner for that target:
//   CARGO_TARGET_WASM32_WASIP1_RUNNER="node scripts/wasi-run.mjs" cargo test --target wasm32-wasip1
import { readFile } from 'node:fs/promises';
import { argv, env, exit } from 'node:process';
import { WASI } from 'node:wasi';

const [program, ...args] = argv.slice(2);
const wasi = new WASI({ version: 'preview1', args: [program, ...args], env, preopens: { '/': '/' }, returnOnExit: true });
const module = await WebAssembly.compile(await readFile(program));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
exit(wasi.start(instance));
