# veripb-repl

An interactive REPL/TUI for writing and checking VeriPB proof-format
version 3 (`.pbp`) proofs one line at a time. Every line is checked by a
`veripb` binary, run as a subprocess.

- [QUICKSTART.md](QUICKSTART.md) — a five-minute walkthrough.
- [USER_GUIDE.md](USER_GUIDE.md) — the full command reference.
- [CONTRIBUTING.md](CONTRIBUTING.md) — tests and code layout.

## Prerequisites

- Rust **1.92** or newer (`rustc --version`; install or update via
  [rustup.rs](https://rustup.rs)).
- An up-to-date `veripb` that supports the `--dump-database` and
  `--dump-objective` flags (see below). Without them, `:show`, the TUI's
  Database pane, `:objective` and `:check`'s automatic `BOUNDS` detection
  don't work.
- For the full-screen TUI: a terminal with truecolor (24-bit RGB)
  support. `--plain` gives a line-based prompt that doesn't need it.

## Building veripb-repl

```sh
cargo build --release
```

The binary is `target/release/veripb-repl` (`target\release\veripb-repl.exe`
on Windows).

## Building veripb

```sh
git clone <VERIPB_REPO_URL>
cd veripb
cargo build --release
```

The binary is `target/release/veripb` (`target\release\veripb.exe` on
Windows). Build it on the machine you'll run it on. Check that it has the
required flags:

```sh
target/release/veripb --help
```

The output must list `--dump-database <PATH>` and `--dump-objective <PATH>`.

## Pointing veripb-repl at veripb

`veripb-repl` uses `$VERIPB_REPL_VERIPB_BIN` if it's set, otherwise
`veripb` on `PATH`.

### Option A — `VERIPB_REPL_VERIPB_BIN` (recommended)

Set it to the **absolute** path of the binary.

**bash / zsh** (add to `~/.bashrc`/`~/.zshrc` to make it permanent):

```sh
export VERIPB_REPL_VERIPB_BIN=/absolute/path/to/veripb/target/release/veripb
```

**fish** (`-U` keeps it across sessions):

```fish
set -Ux VERIPB_REPL_VERIPB_BIN /absolute/path/to/veripb/target/release/veripb
```

**Windows PowerShell** (current session, then permanently; `setx` applies
to newly opened terminals only):

```powershell
$env:VERIPB_REPL_VERIPB_BIN = "C:\absolute\path\to\veripb\target\release\veripb.exe"
setx VERIPB_REPL_VERIPB_BIN "C:\absolute\path\to\veripb\target\release\veripb.exe"
```

**Windows cmd.exe** (current session only; use `setx` to persist):

```bat
set VERIPB_REPL_VERIPB_BIN=C:\absolute\path\to\veripb\target\release\veripb.exe
```

### Option B — `PATH`

Leave `VERIPB_REPL_VERIPB_BIN` unset and put the `veripb` binary (or its
directory) on `PATH`.

## Running

```sh
cargo run --release -- <formula.opb>
```

See [QUICKSTART.md](QUICKSTART.md) for an example and
[USER_GUIDE.md](USER_GUIDE.md) for everything else.

## Windows notes

- Builds with the ordinary MSVC or GNU Rust toolchain; no C toolchain is
  needed.
- The TUI needs a truecolor terminal such as Windows Terminal, not legacy
  `cmd.exe`/`conhost`. Use `--plain` if colours don't render correctly.
- Read Unix-style paths in these docs (`target/release/veripb`) as
  `target\release\veripb.exe`.

## Troubleshooting

- **`cannot execute binary file: Exec format error`** — `veripb` was
  built for a different OS or architecture. Build it on the machine that
  runs it.
- **(macOS) the binary is `Killed` when run from a copied location** — the
  copy's code signature is invalid. Re-sign it
  (`codesign --force -s - <path>`) or point `VERIPB_REPL_VERIPB_BIN` at
  the original build output.
- **`veripb wrote no --dump-database output`** (or `--dump-objective`) —
  the `veripb` in use doesn't support that flag. Check which binary
  `VERIPB_REPL_VERIPB_BIN`/`PATH` points at and that its `--help` lists
  both flags.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE),
at your option.
