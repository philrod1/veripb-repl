# VeriPB REPL — quick start

Build it, run it, and check a proof step. For the full command
reference, see [USER_GUIDE.md](USER_GUIDE.md).

## Prerequisites

- Rust **1.92** or newer (`rustc --version`; install or update via
  [rustup.rs](https://rustup.rs)).
- An up-to-date `veripb`, built and configured as described in
  [README.md](README.md).
- For the full-screen interface: a terminal with truecolor (24-bit RGB)
  support. `--plain` doesn't need it.

## Build & run

From the repository root:

```bash
cargo run -- <formula.opb>
```

The formula path is optional; without it, use `:load <formula.opb>` once
the REPL starts. The first run compiles everything and takes a minute or
two.

This opens the full-screen TUI: Formula, Database and Proof panes above
an Output pane with a `pbp>` prompt. For a line-based prompt instead:

```bash
cargo run -- --plain <formula.opb>
```

`--plain` is also used automatically when input or output isn't a
terminal, so you can pipe a script of commands in.

`:quit` (or Ctrl-D) exits.

## Try it

`examples/tiny_unsat.opb` is an unsatisfiable formula with three
constraints (`x1 ∨ x2`, `¬x1 ∨ x2`, `¬x2`):

```
$ cargo run -- examples/tiny_unsat.opb
Loaded 3 constraints from examples/tiny_unsat.opb
pbp> :check
NOT YET CONCLUDED
pbp> rup 1 x2 >= 1 ;
Running VeriPB version 3.0.2
line    3: rup 1 x2 >= 1 ;
  ConstraintId 4: 1 x2 >= 1
pbp> rup >= 1 ;
Running VeriPB version 3.0.2
line    4: rup >= 1 ;
  ConstraintId 5: >= 1
pbp> :check
Running VeriPB version 3.0.2
s VERIFIED UNSATISFIABLE
pbp> :quit
```

- `rup 1 x2 >= 1 ;` derives `x2`: assuming `x2 = 0`, unit propagation on
  the formula reaches a conflict.
- `rup >= 1 ;` derives the contradiction (an empty left-hand side, which
  can never reach 1).
- `:check` reports whether the proof so far justifies a conclusion,
  without ending the session.
- Each accepted line prints the checker's trace: its version, the line
  number it assigns (lines 1–2 are the preamble the REPL supplies), and
  the constraint added.
- A rejected line isn't added; fix it and type it again.

In the TUI, the Database pane shows constraints 4 and 5 as each line is
accepted.

## Where next

- [USER_GUIDE.md](USER_GUIDE.md) — every command, the TUI (panes,
  completion, mouse, themes), and longer walkthroughs, including finishing
  this proof with `conclusion UNSAT;`.
- `proof_format_overview.md` in the veripb repository — the full v3
  proof-format grammar (`pol`, `red`, `e`, `del`, subproofs, …).
