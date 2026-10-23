# WebAssembly build experiment

Builds veripb and the veripb-repl core for WebAssembly:
- `wasm32-unknown-unknown`: the browser target
- `wasm32-wasip1`: the veripb CLI, run under a WASI runtime

## Requirements

- `rustup target add wasm32-unknown-unknown wasm32-wasip1`
- Node.js 22 or later, for `node:wasi`
- veripb source with `--dump-database` and `--dump-objective` support, in
  `../veripb-src`

## Contents

| Path | Purpose |
|---|---|
| `veripb-wasm.patch` | veripb changes needed for wasm targets (below) |
| `veripb-wasm/` | wrapper crate: links the patched veripb and enables `getrandom`'s `wasm_js` backend |
| `make-repl-core.sh` | creates `repl-core/`, a copy of veripb-repl without `src/tui` and `crossterm` |
| `wasi-run/run.mjs` | runs one veripb `.wasm` invocation under `node:wasi` |
| `wasi-run/batch.mjs` | runs a list of `opb pbp [outopb]` lines (read from stdin) through one compiled module |
| `wasi-run/runtest.mjs` | runs a Rust test binary built for `wasm32-wasip1` |
| `bigint-probe/` | prints `malachite-bigint` and `i128` arithmetic results, for diffing native against wasm |

## veripb patch

Changes in `veripb-wasm.patch`:
- **`veripb-parser`:** `zstd` and `xz2` become optional, under a default
  feature `native-compression`. Without the feature, `.zst` and `.xz` inputs
  return an "unsupported" error. Gzip is unaffected.
- **`veripb-checker` and `veripb`:** depend on `veripb-parser` with
  `default-features = false`. `veripb` forwards `native-compression` as a
  default feature.
- **`mimalloc`:** only used on non-wasm targets.
- **Parser bug fix:** the pol-integer token `NonNegativeIntegerOrID` holds an
  `i64` instead of an `isize`. With `isize`, pol multipliers of 2^31 or more
  are truncated on 32-bit targets, and valid proofs are rejected.

To apply:

```
cp -r ../veripb-src veripb-src-wasm      # without target directories
(cd veripb-src-wasm && patch -p1 < ../veripb-wasm.patch)
```

## Build

Use a separate target directory (see the main repo's build notes):

```
export CARGO_TARGET_DIR=~/.cache/veripb-wasm-target

# veripb library, browser target
(cd veripb-wasm && cargo build --release --target wasm32-unknown-unknown)

# veripb CLI, WASI target
(cd veripb-src-wasm && cargo build --release --bin veripb --no-default-features --target wasm32-wasip1)

# veripb-repl core, browser target
./make-repl-core.sh
(cd repl-core && cargo check --lib --target wasm32-unknown-unknown)
```

## Run

```
W=$CARGO_TARGET_DIR/wasm32-wasip1/release/veripb.wasm

# One proof; <dir> is mounted at /work
node --no-warnings wasi-run/run.mjs $W <dir> -- /work/foo.opb /work/foo.pbp --dump-database /work/foo.db

# veripb's test suite; output goes to out/<n>.out, one tab-separated line per case on stdout
cd ../veripb-src/tests/instances
find . -name '*.pbp' | sort | while read p; do o="${p%.pbp}.opb"; q="${p%.pbp}.outopb"
  [ -f "$o" ] && echo "${o#./} ${p#./}$([ -f "$q" ] && echo " ${q#./}")"; done > list.txt
mkdir -p out && node --no-warnings <path>/wasi-run/batch.mjs $W . out 0 < list.txt
```

Keep each `batch.mjs` run to about 30 lines (use `split -l 30`). Larger
batches crash Node.

## Results (veripb 3.0.2, 385 instances in `tests/instances`)

- **Exit codes:** match native on every instance.
- **`--dump-database` and `--dump-objective` output:** byte-identical to
  native.
- **Error messages:** differ on three `incorrect/` instances. Each has
  several violations, and the two builds report a different one first.
- **veripb-repl core:** compiles for `wasm32-unknown-unknown` without the
  TUI. These parts depend on processes or files at runtime:
  - `checker/invoke.rs`: veripb subprocess and temp files
  - `session.rs`: formula temp file, `load` from a path
  - `commands/save.rs`, `commands/source.rs` and `varnames.rs`: file reads
    and writes
  - `commands/mod.rs`: `HOME` lookup
