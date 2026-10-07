# Demos

Step-by-step worked examples of using the REPL's TUI. Each demo uses
small example files from [`files/`](files/). Run the commands from the
repository root, with `veripb` configured as in [README.md](../README.md).

| Demo | Shows |
|---|---|
| [1. Debugging a rejected `rup` step](01-debugging-a-rup-step.md) | `:verify` stops at a bad line; `:explain` shows the hint list is missing a constraint; fix with `:edit` in Vim mode; `:verify`, `:check`, `:save`. |
| [2. Finding a wrong `pol` step](02-finding-a-wrong-pol-step.md) | Every line is accepted but the proof doesn't conclude; `:debug` with a breakpoint shows the database mid-proof; `:explain` shows the `pol` arithmetic; fix and re-check. |
| [3. Writing a proof top-down with assertions](03-top-down-with-assertions.md) | Sketch a proof with `a` assertions and check it concludes `UNDER ASSERTIONS`; `:deassert` each one, guided by highlighted related constraints, until it's `VERIFIED`. |
