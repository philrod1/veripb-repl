import { readFileSync } from 'node:fs';
import { WASI } from 'node:wasi';
const [wasmPath, ...args] = process.argv.slice(2);
const wasi = new WASI({ version: 'preview1', args: ['t', ...args], preopens: { '.': '.' }, returnOnExit: true });
const inst = await WebAssembly.instantiate(await WebAssembly.compile(readFileSync(wasmPath)), wasi.getImportObject());
process.exitCode = wasi.start(inst);
