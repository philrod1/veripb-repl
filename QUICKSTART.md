# VeriPB REPL — quick start

Everything here in about five minutes: build it, run it, check one real
proof step. For the full command reference and feature status, see
[USER_GUIDE.md](USER_GUIDE.md).

## Prerequisites

- Rust **1.92** or newer (this workspace's `rust-version`) — `rustc
  --version` to check, [rustup.rs](https://rustup.rs) to install/update.
- A full checkout of this repo. `veripb-repl` is one crate in the larger
  `VeriPB` cargo workspace and depends on its sibling crates
  (`veripb-checker`, `veripb-parser`, `veripb-formula`) by relative path —
  a standalone copy of just the `veripb-repl/` directory won't build.
- For the default full-screen interface: a terminal emulator with
  truecolor (24-bit RGB) support. Effectively every terminal in current
  use qualifies; `--plain` (below) doesn't need it, since it prints no
  color at all.

## Build & run

From the repository root:

```bash
cargo run -p veripb-repl -- <formula.opb>
```

The formula path is optional — run with none to start empty and
`:load <formula.opb>` once you're in. First run will take a minute or two
to compile everything; after that, `cargo run` is fast.

That opens the full-screen TUI: three panes (formula / live constraint
database / your proof so far) over an output pane with a `pbp>` prompt.
Pass `--plain` for a bare line-based prompt instead (also selected
automatically when input/output isn't a real terminal — piping a script
in just works, no flag needed):

```bash
cargo run -p veripb-repl -- --plain <formula.opb>
```

Type `:quit` (or Ctrl-D) to exit either one.

## Try it

`examples/tiny_unsat.opb` is a tiny three-constraint formula
(`x1 ∨ x2`, `¬x1 ∨ x2`, `¬x2`) that's unsatisfiable, but not obviously so
from any single constraint — enough to actually derive something:

```
$ cargo run -p veripb-repl -- veripb-repl/examples/tiny_unsat.opb
Loaded 3 constraints from veripb-repl/examples/tiny_unsat.opb
pbp> :check
NOT YET CONCLUDED
pbp> rup 1 x2 >= 1 ;
  ConstraintId 4: 1 x2 >= 1
pbp> rup >= 1 ;
  ConstraintId 5: >= 1
pbp> :check
s VERIFIED UNSATISFIABLE
pbp> :quit
```

What happened: `rup 1 x2 >= 1 ;` derives `x2` by negating it (`x2 = 0`)
and unit-propagating against the formula's own constraints until they
conflict. `rup >= 1 ;` — an empty left-hand side, unconditionally false —
is the standard v3 way to materialize an outright contradiction once
enough is derived. `:check` asks the real checker whether everything
typed so far already justifies a conclusion, without closing the
session — safe to run as often as you like while you work.

Every line is independently re-checked from scratch against the real
VeriPB checker (not reimplemented — this REPL is a thin interactive
shell around it), so a rejected line never leaves you in a broken state:
just fix it and retype. If you're in the TUI, the Database pane
(middle column) shows constraints 4 and 5 land the moment each `rup`
above is accepted — worth watching as you type.

## Where next

- [USER_GUIDE.md](USER_GUIDE.md) — the full reference: every command,
  what's implemented vs. planned, the TUI's panes/completion/mouse
  support/themes, and a couple of longer worked walkthroughs (including
  finishing the one above with a real `conclusion UNSAT;`).
- [`proof_format_overview.md`](../proof_format_overview.md) at the repo
  root — the full v3 proof-format grammar (`pol`, `red`, `e`, `del`,
  subproofs, ...) for when you're ready to go beyond `rup`.
