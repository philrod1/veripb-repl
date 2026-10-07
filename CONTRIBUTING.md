# Contributing to veripb-repl

See [README.md](README.md) for building `veripb-repl` and `veripb`.

## Tests

```sh
VERIPB_REPL_VERIPB_BIN=/absolute/path/to/veripb cargo test
```

Tests that run `veripb` are skipped when `VERIPB_REPL_VERIPB_BIN` is
unset. Parser tests (`tests/*_parse.rs`, `tests/trace_parse.rs`,
`tests/objective_dump.rs`, …) need no binary.

## Code layout

- `src/session.rs` — `Session`: formula text, the proof buffer and how
  much of it is checked, undo stack, breakpoints; replaying the proof
  through `veripb` and the cached database.
- `src/checker/` — running `veripb` and parsing its output: `invoke.rs`
  (the subprocess call, including `--dump-database`/`--dump-objective`),
  `parse.rs` (trace and dump parsing), `types.rs` (parsed results).
- `src/varnames.rs` — the formula's variable names, for highlighting and
  completion.
- `src/output.rs` — the `Output` sink all command output goes through.
- `src/commands/` — one file per `:`-command (`show.rs`, `check.rs`,
  `preserved.rs`, …); `help.rs` also holds the command and rule help
  registry used by `:help` and Tab completion. `:quit` is handled in
  `commands::dispatch`.
- `src/plain.rs` — the line-based frontend (`--plain`).
- `src/tui/` — the terminal UI: `mod.rs` (app state, event loop),
  `layout.rs`, `draw.rs` (rendering) with `draw/scrollback.rs`
  (Output-pane highlighting), `input.rs` (line editor and history),
  `complete.rs` (Tab completion and suggestion strip), `theme.rs`
  (palettes).
- `src/main.rs` — argument parsing and frontend selection.

## Conventions

- Every check runs `veripb` as a subprocess on the formula plus the
  checked proof prefix; no checker state is kept between calls.
- Comments and docs are functional: what the code does, its contract and
  invariants. No history or narrative.
