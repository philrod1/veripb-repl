// Usage: node run.mjs <veripb.wasm> <dir> -- <veripb args, paths relative to /work>
import { readFile } from 'node:fs/promises';
import { WASI } from 'node:wasi';
const [wasmPath, dir, , ...args] = process.argv.slice(2);
const wasi = new WASI({ version: 'preview1', args: ['veripb', ...args], preopens: { '/work': dir }, returnOnExit: true });
const mod = await WebAssembly.compile(await readFile(wasmPath));
const inst = await WebAssembly.instantiate(mod, wasi.getImportObject());
const t = performance.now();
const code = wasi.start(inst);
console.error(`[exit ${code}, ${(performance.now() - t).toFixed(0)} ms]`);
process.exitCode = code;
