// Usage: node batch.mjs <veripb.wasm> <root> <outdir> < list of "opb pbp" relative paths
import { readFileSync, openSync, closeSync } from 'node:fs';
import { WASI } from 'node:wasi';
const [wasmPath, root, outDir, base] = process.argv.slice(2);
const mod = await WebAssembly.compile(readFileSync(wasmPath));
const lines = readFileSync(0, 'utf8').trim().split('\n');
for (const [i, line] of lines.entries()) {
  const [opb, pbp, outopb] = line.split(' ');
  const out = openSync(`${outDir}/${+base + i}.out`, 'w');
  const wasi = new WASI({ version: 'preview1', args: ['veripb', opb, pbp, ...(outopb ? [outopb] : [])], preopens: { '.': root },
                          stdout: out, stderr: out, returnOnExit: true });
  const t = performance.now();
  let code;
  try { code = wasi.start(await WebAssembly.instantiate(mod, wasi.getImportObject())); }
  catch (e) { code = `trap:${e.message}`; }
  closeSync(out);
  console.log(`${+base + i}\t${code}\t${(performance.now() - t).toFixed(1)}\t${pbp}`);
}
