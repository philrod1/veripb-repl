# veripb-repl

An interactive REPL/TUI for exploring and building VeriPB proof-format
version 3 (`.pbp`) proofs one line at a time, checked live against a real
`veripb` binary — see [QUICKSTART.md](QUICKSTART.md) for a five-minute
walkthrough and [USER_GUIDE.md](USER_GUIDE.md) for the full command
reference.

`veripb-repl` never reimplements any checking logic itself: every rule you
type is handed to an installed `veripb` binary as a subprocess, and the
REPL only manages when and how that binary is invoked. That means two
things need building before it's useful: `veripb-repl` itself, and a
compatible `veripb` binary.

## Prerequisites

- Rust **1.92** or newer (this crate's `rust-version`) — `rustc --version`
  to check, [rustup.rs](https://rustup.rs) to install/update.
- A `veripb` binary built from the `feature/database-dump` branch (below)
  — required. An older/stock `veripb` will error on `:show`/the TUI's
  Database pane, and silently lose `:check`'s automatic `BOUNDS`
  detection and `:objective`'s bound reporting.
- For the default full-screen TUI: a terminal with truecolor (24-bit RGB)
  support. `--plain` (a bare line-based prompt) doesn't need it, and is
  selected automatically when input/output isn't a real terminal.

## Building veripb-repl

From this repository's root:

```sh
cargo build --release
```

The binary lands at `target/release/veripb-repl` (`target\release\veripb-repl.exe`
on Windows). `cargo run --` works the same way for a quick local run
without a separate build step — see [QUICKSTART.md](QUICKSTART.md).

## Building a compatible `veripb`

`veripb-repl` needs `--dump-database`/`--dump-objective` export flags
that aren't in any stock `veripb` release yet — only on the
`feature/database-dump` branch of the core VeriPB repository.

```sh
git clone <VERIPB_REPO_URL>
cd veripb
git checkout feature/database-dump
cargo build --release
```

This produces its own binary, independent of `veripb-repl`'s:

| Platform      | Path                            |
|----------------|---------------------------------|
| Linux / macOS  | `target/release/veripb`         |
| Windows        | `target\release\veripb.exe`     |

Build natively on whatever machine you intend to run `veripb-repl` on —
a binary built for one OS/architecture won't run on another, and there's
no cross-compilation step needed here; plain `cargo build --release`
already targets the machine it runs on. Confirm the build actually has
what's needed before moving on:

```sh
target/release/veripb --help
```

should list both `--dump-database <PATH>` and `--dump-objective <PATH>`
among its options.

## Pointing veripb-repl at your `veripb`

`veripb-repl` resolves the binary to run in one of two ways:

### Option A — `VERIPB_REPL_VERIPB_BIN` (recommended)

Set it to an **absolute** path to the binary built above. A relative
path is resolved against the working directory of the process spawning
`veripb`, not the directory you were in when you set the variable — it
can silently point somewhere unexpected, so avoid it here.

**bash / zsh:**

```sh
export VERIPB_REPL_VERIPB_BIN=/absolute/path/to/veripb/target/release/veripb
```

Add that line to `~/.bashrc`/`~/.zshrc` to make it permanent.

**fish:**

```fish
set -Ux VERIPB_REPL_VERIPB_BIN /absolute/path/to/veripb/target/release/veripb
```

(`-U` persists it across sessions; drop it to set it for the current
shell only.)

**Windows PowerShell:**

```powershell
$env:VERIPB_REPL_VERIPB_BIN = "C:\absolute\path\to\veripb\target\release\veripb.exe"
```

To persist it across sessions:

```powershell
setx VERIPB_REPL_VERIPB_BIN "C:\absolute\path\to\veripb\target\release\veripb.exe"
```

(`setx` only takes effect in terminal windows opened *after* running it —
not the one you ran it in.)

**Windows cmd.exe:**

```bat
set VERIPB_REPL_VERIPB_BIN=C:\absolute\path\to\veripb\target\release\veripb.exe
```

(session-only; use `setx`, above, or the System Properties → Environment
Variables dialog to persist it.)

### Option B — put it on `PATH`

With `VERIPB_REPL_VERIPB_BIN` unset, `veripb-repl` looks for a bare
`veripb` (`veripb.exe` on Windows) on `PATH`. Copy or symlink the binary
you built into a directory already on `PATH`, or add its containing
directory to `PATH`, instead of setting the environment variable.

## Running it

```sh
cargo run --release -- <formula.opb>
```

See [QUICKSTART.md](QUICKSTART.md) for a worked example and
[USER_GUIDE.md](USER_GUIDE.md) for everything else.

## Windows notes

- Nothing `veripb-repl` itself depends on needs a C toolchain — the
  ordinary MSVC or GNU Rust toolchain builds it with plain `cargo build`.
- The TUI needs a truecolor-capable terminal — Windows Terminal, not
  legacy `cmd.exe`/`conhost`. Pass `--plain` if colors don't render
  correctly.
- Wherever this repo's docs show a Unix-style path
  (`target/release/veripb`), read it as `target\release\veripb.exe` on
  Windows.

## Troubleshooting

- **`cannot execute binary file: Exec format error`** (or similar) — the
  `veripb` binary was built for a different OS/architecture than the
  machine running it. Build it natively on that machine instead of
  copying a binary over from elsewhere.
- **(macOS) the binary gets silently `Killed` when run from a copied
  location** — macOS enforces code-signing on every executable, and a
  plain `cp` can leave a copy with an invalid signature. Either
  re-sign it (`codesign --force -s - <path>`) or point
  `VERIPB_REPL_VERIPB_BIN` straight at wherever `cargo build` originally
  produced it, with no copy involved.
- **`veripb wrote no --dump-database output` / `--dump-objective
  output`** — the resolved `veripb` doesn't have that flag. Confirm
  `VERIPB_REPL_VERIPB_BIN`/`PATH` actually points at a
  `feature/database-dump` build, and that `veripb --help` lists both
  flags.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE),
at your option.
