# VeriPB REPL — user guide

> **New here?** [QUICKSTART.md](QUICKSTART.md) takes you from a fresh
> checkout to a checked proof step. This document is the full reference:
> every command, key, and output, and what is not implemented yet.

> **Status: early proof-of-concept.** Features marked **Not implemented**
> error, do nothing, or don't exist as commands yet.

## What this is

`veripb-repl` is an interactive terminal tool for entering VeriPB
proof-format version 3 (`.pbp`) rules one line at a time against a loaded
pseudo-Boolean formula. Each line is checked as soon as you enter it, and
the result is shown: the derived constraint, or the checker's error.

Every check runs a `veripb` binary as a subprocess
(`$VERIPB_REPL_VERIPB_BIN`, else `veripb` on `PATH`); no checker state
persists between calls. It requires an up-to-date `veripb` that supports
`--dump-database` and `--dump-objective` — see [README.md](README.md) for
building it and pointing the REPL at it.

## Quick start

See [QUICKSTART.md](QUICKSTART.md). In short: `cargo run -- <formula.opb>`
from the repo root. The formula path is optional; `:load <formula.opb>`
does the same from inside the REPL.

Type proof-format v3 rules at the `pbp>` prompt, one per line, exactly as
in a `.pbp` file (including the trailing `;`). `:quit` or EOF (Ctrl-D)
exits. Only `:instance`, `:load`, `:help`, `:theme`, and `:quit` work
without a formula; every other command reports "No formula loaded".

### The TUI (default) and `--plain`

By default the REPL opens a full-terminal interface: three columns —
Formula, Database (the live constraint database, as `:show` with no
filters), and Proof (as `:list`) — above a full-width Output pane with the
prompt. The Proof column starts with the two synthesized preamble lines,
dimmed, so its line numbers match the `line N:` numbers in checker traces
and errors; your first rule is line 3. The columns refresh after every
accepted line; command output, checker traces, and errors go to the
Output pane.

Constraint labels (`@name`) are shown ahead of the constraints they name
in the Formula and Database columns. The Formula column numbers each line
by its constraint ID, matching the Database column; a line that loads as
two constraints (an equivalence `<==>` or an equality `=`) shows an ID
range, e.g. `1-2:`.

**Colouring.** Variable names and `@labels` are highlighted in the three
columns (one colour each); keywords, operators, and numbers are not.
Formula and Database constraints are coloured token by token. Proof lines
are coloured by looking up each whitespace-separated word: a word naming a
formula variable (optionally `~`-prefixed) is a variable, a word starting
with `@` is a label. A variable with punctuation attached directly to it
is not highlighted.

**Row backgrounds.**

- Proof column: an `a`-rule (unchecked assertion) line has a dark red
  background — exactly the lines `:deassert` accepts. The Vim-mode cursor
  row takes precedence on the same line.
- Database column: dark green for a core constraint, dark amber for a
  derived one (instead of a `[core]`/`[derived]` tag). Together with the
  red assertion rows: red = unchecked, amber = derived, green = core.
- While `:deassert` is active, Database rows sharing a variable with the
  assertion are highlighted in the cursor-row accent colour.
- Unchecked buffer lines (e.g. a `:source`'d file before `:verify`) are
  dimmed like the preamble. The line `:verify` last rejected gets a
  stronger red background than an assertion row.

**Themes.** `:theme [<name>]` switches palettes:

| Theme | Description |
|---|---|
| `dark` (default) | dark background, light text |
| `light` | white/light-grey background, near-black text |
| `hi-contrast` | black/white background and text, fully saturated highlights, brighter dim |
| `colorblind` | dark background; vermillion (assertion), yellow (derived), blue-green (core), from the [Okabe-Ito palette](https://jfly.uni-koeln.de/color/), distinguishable under protanopia, deuteranopia, and tritanopia |

`dark`, `light`, and `hi-contrast` use the same red/amber/green roles. The
rejected-line background is a stronger version of the theme's assertion
colour in every palette. Every character the TUI draws uses the theme's
explicit RGB colours, not the terminal's defaults. Bare `:theme` reports
the current and available themes. `:theme` is TUI only; the plain
frontend says so.

**Output pane.** Checker trace headers (`line 3: ...`) and `:explain`
headers (`Line 3 needed:`, `Line 4 is rejected:`) are bold;
`ConstraintId N:` prefixes are dimmed; variables and `@labels` in
constraint and rule text are coloured as in the panes; a `~` hint is in
the accent colour; failures (`Error:` lines, a rejection's reason, missing
hint IDs, "Without hints it still fails") are red and "Without hints it
DOES check" is green (vermillion and blue-green under `colorblind`).
Everything else is plain.

**Focus and scrolling.** Shift+Tab cycles focus between the four panes;
the focused pane's title (Output's is in the separator bar) is shown in
reverse video. PgUp/PgDn scroll the focused pane by a page. With a top
pane focused, ↑/↓ scroll it by a line and ←/→ scroll it horizontally;
line numbers (right-aligned, e.g. `  9: `, ` 10: `) stay pinned at the
left. With Output focused (the default), the arrow keys belong to the
prompt. Typing always goes to the prompt, whatever has focus. With the
Proof pane focused, Enter on an empty prompt opens the Vim-style editor
at the topmost visible line (see `:edit`).

Scrollbars appear inside a pane only when its content overflows: a `┃`
thumb on a dimmed `│` track in the last column (vertical), a `━` thumb on
a dimmed `─` track in the last row (horizontal). The Formula pane anchors
at the top; Database, Proof, and Output follow their newest content until
scrolled. Running any command resets every pane to its default view.

**Zoom.** Formula, Database, and Proof each have two header buttons:
`[▭]` widens the pane to fill the top row (the Output area keeps its
size); `[⛶]` also shrinks the Output area to its minimum. On Output,
`[▭]` shrinks the top row to three lines and `[⛶]` hides it. One pane is zoomed at a
time. Click the button again, or press Esc, to un-zoom. Esc goes first to
Vim mode, a `:formula` browse, or `:debug` if one is active; otherwise it
un-zooms before it clears a completion highlight. Entering Vim mode or a
`:formula` browse on a different pane while zoomed moves the zoom to that
pane at the same level.

**Mouse.**

- The wheel scrolls the pane under the pointer (without changing focus).
  Shift+wheel, Alt+wheel, or a trackpad's sideways scroll scrolls it
  horizontally — the only way to scroll Output horizontally.
- A left click focuses a pane.
- Double-clicking a Proof line enters Vim mode (`Normal`) on it, or, for
  an `a`-rule line, starts `:deassert` on it (in `Insert`). In `Normal`
  mode it just moves the cursor. Double-clicking a Formula line starts a
  `:formula` browse on it.
- Clicking a Proof line's gutter marker column toggles a breakpoint (see
  `:debug`).
- To select text for copying, use your terminal's mouse-capture bypass
  (usually Shift, or Option/Alt on macOS).

Every command produces the same output lines in both frontends.

**Prompt line editing.** ←/→, Home/End (or Ctrl-A/Ctrl-E), ↑/↓ history,
Ctrl-U/Ctrl-K kill to start/end of line, Ctrl-C clears the line, Ctrl-D on
an empty line exits. Ctrl+←/→ (or Alt+←/→) move to the start of the
previous/next word. Vim mode's `Insert` sub-mode uses the same editor,
except that ↑/↓ move between proof lines (see `:edit`); `Normal` has its
own Ctrl/Alt+←/→ word motion.

**Suggestion strip and Tab completion.** Once you start typing, a dimmed
strip under the prompt lists candidates for the first word: `:`-commands
when the line starts with `:` (a lone `:` lists all of them), rule
keywords otherwise. After a complete rule keyword, its arguments complete
against live constraint IDs and variable names (bare and negated, `~x3`)
from the session. This is not grammar-aware (e.g. `red` is offered
constraint IDs in its substitution). Where constraint IDs are offered,
their `@labels` are offered too. Nothing is offered for arguments before a
formula is loaded.

- `:instance` is pre-selected among `:`-command candidates. With no
  session, `:instance`, `:load`, `:help`, `:theme`, `:quit` are listed
  first, in that order.
- The strip is always six rows tall when visible.
- ↑/↓ move a highlight through the candidates (wrapping). Enter or Tab
  inserts the highlighted candidate; Esc drops the highlight. Enter
  submits the line only when nothing is highlighted.
- With no highlight, Tab completes a single match outright, else to the
  longest common prefix. A completed command or rule name gets a trailing
  space. A common prefix ending in a bare `.` (e.g. `foo.opb` and
  `foo.pbp`) stops before the dot.
- ↑ on an empty line starts a history walk; while walking, the strip is
  hidden and ↑/↓ step through history. The walk ends when you edit the
  recalled line, submit it, step ↓ past the newest entry, or press Tab
  (which also completes the line).
- After a full completion, the strip shows that command's usage line.
- The strip, Tab, and `:help` use the same registry.

In the argument of `:load`, `:source`, `:save`, and `:instance`, Tab and
the strip complete file paths: relative, absolute, or `~/`-prefixed (the
commands expand `~` in both frontends). Directories complete with a
trailing `/`. Files are filtered by command: `.opb` for `:load`, `.pbp`
for `:source` and `:save`, both for `:instance`. Compressed formulas are
not supported. Matching is case-insensitive; dot-files appear once you
type the leading dot; Tab on an empty argument lists the current
directory; paths containing spaces are not completed (the commands accept
them). `:help` and `:theme` arguments complete against their topic and
theme names.

**`--plain`.** Selects the line-based frontend: plain stdin reading, with
no line editing, history, colour, or Tab completion. It is selected
automatically when stdin or stdout isn't a terminal, so piped scripts
work without the flag. Its prompt shows the mode: `pbp>`, `edit>` (or
`edit (deasserting <constraint>)>`), `opb>`, or `debug>`.

## Walkthrough

Using `examples/redundance_rup.opb` / `redundance_rup.pbp` (plain
frontend shown, with the typed lines after each prompt):

```
$ cargo run -- examples/redundance_rup.opb
Loaded 3 constraints from examples/redundance_rup.opb
pbp> red 1 x1 1 x2 >= 1 ;
Running VeriPB version 3.0.2
line    3: red 1 x1 1 x2 >= 1 ;
* constraint is RUP *
  ConstraintId 4: 1 x1 1 x2 >= 1
pbp> e 1 x1 1 x2 >= 1 : -1;
Running VeriPB version 3.0.2
line    4: e 1 x1 1 x2 >= 1 : -1;
pbp> output NONE;
Running VeriPB version 3.0.2
line    5: output NONE;
pbp> conclusion NONE;
Running VeriPB version 3.0.2
line    6: conclusion NONE;
s VERIFIED NO CONCLUSION
pbp> end pseudo-Boolean proof;
Running VeriPB version 3.0.2
s VERIFIED NO CONCLUSION
line    7: end pseudo-Boolean proof;
pbp> :quit
```

- **Each line is checked by replaying the whole checked prefix.** The REPL
  keeps a buffer of proof lines and a count of how many at the front are
  checked. A new line is checked by running `veripb` on a synthesized
  proof (v3 preamble, the checked prefix, the new line). On success the
  line joins the checked prefix. On failure nothing changes and the line
  is not added. If unchecked lines are already in the buffer, a new line
  is appended to them unchecked instead (see `:verify`).
- **The output is `veripb`'s own trace** for the new line: its version
  banner, `line N: ...` (numbered from 3, after the preamble), and
  `ConstraintId N: ...` for a derived constraint. Rules that derive no
  constraint (`e`, `output`, `conclusion`, ...) print no `ConstraintId`
  line. Once a `conclusion` is in the checked prefix, each later replay
  prints its `s VERIFIED ...` line again (hence the second one above).
- **`output`, `conclusion`, and `end pseudo-Boolean proof;` are checked
  rules.** A `conclusion UNSAT;` that your derivation doesn't justify is
  rejected like a bad `rup`.
- **`conclusion NONE` performs no check** and verifies with zero lines
  derived. `s VERIFIED NO CONCLUSION` means no claim was made, not that
  anything was proved. `UNSAT`, `SAT`, and `BOUNDS ...` are checked.
- **After `end pseudo-Boolean proof;` the proof is closed**: every further
  line is rejected. Use `:check` to see a verification result without
  closing the proof, and `:undo` to remove the closing lines.

### Walkthrough: building towards a real conclusion

`examples/tiny_unsat.opb` has three constraints, `x1 ∨ x2`, `¬x1 ∨ x2`,
`¬x2`, which are jointly unsatisfiable:

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

The first `:check` prints no banner because every candidate conclusion
failed, and failed candidates print nothing (see `:check`).

`rup 1 x2 >= 1 ;` derives `x2`: assuming `x2 = 0`, unit propagation on
constraints 1 and 2 reaches a conflict. `rup >= 1 ;` derives the
constraint with an empty left-hand side, which is always false — the
standard v3 way to record a contradiction. `conclusion UNSAT` requires
such a contradicting constraint in the database; constraints 3 and 4
being jointly unsatisfiable is not enough.

## Proof rule syntax

Rules use v3 `.pbp` syntax; `proof_format_overview.md` in the veripb
repository is the full reference for `pol`, `rup`, `red`, `e`, `del`,
`ia`, and the rest. `:help <rule>` gives usage and details for each rule.
Some rejections print a hint (e.g. a `*x` shrunk variable in
`sol`/`soli`, which only `solx` allows); otherwise errors are the
checker's own.

## Command reference

Proof rules are typed bare. Commands start with `:` and can be
abbreviated to any unambiguous prefix (`:q` is `:quit`, `:ob` is
`:objective`); an ambiguous prefix (`:s`) is an error listing the
candidates.

| Command | Meaning | Status |
|---|---|---|
| *(bare rule text)* | check a v3 proof rule and, if accepted, add it | **Implemented** |
| `:quit` | exit | **Implemented** |
| `:theme [<name>]` | TUI only: switch palette (`dark`/`light`/`hi-contrast`/`colorblind`) | **Implemented** |
| `:check [<conclusion>]` | test whether the checked prefix verifies with a conclusion, without closing the proof; bare: try `UNSAT`, `SAT`, and `BOUNDS v v` | **Implemented** |
| `:explain [<n>]` | show how line `n` was derived, or why it was rejected | **Implemented** |
| `:list` | print every buffer line, numbered, with `[unchecked]`/`[breakpoint]` tags | **Implemented** |
| `:load <file>` | load an OPB formula, starting a new session | **Implemented** (OPB only) |
| `:instance <file.opb\|file.pbp\|stem>` | `:load` a formula then `:source` its matching proof | **Implemented** |
| `:show [filters]` | list database constraints, filtered by variable or label (exact or `*`-glob), ID/range, or core/derived | **Implemented** |
| `:objective` | show the current objective and best known values | **Implemented** |
| `:preserved` | show the current preserved variable set | **Implemented** |
| `:goals` | inside a subproof, list remaining proof goals | **Not implemented** |
| `:undo [n]` | undo the last `n` actions (default 1) | **Implemented** |
| `:source <file.pbp>` | replace the buffer with a proof file's lines, unchecked | **Implemented** |
| `:verify` | check the buffer's unchecked lines, stopping at the first rejection | **Implemented** |
| `:save <file.pbp> [<conclusion>]` | write the whole buffer, checked or not, to a `.pbp` file | **Implemented** |
| `:trace on\|off` | verbose checker tracing for subsequent lines | **Not implemented** |
| `:help [<topic>]` | list commands, or show details on a command or rule | **Implemented** |
| `:reset` | drop all proof lines, keep the formula | **Implemented** |
| `:edit [<n>]` \| `:edit <n>-<m>` | retype buffer lines without checking them; TUI: Vim-style editor in the Proof pane | **Implemented** (tentative in plain) |
| `:deassert [<n>]` | replace an `a`-rule (the first, or line `n`) with derivation steps | **Implemented** (tentative in plain) |
| `:delete <n>` \| `:delete <n>-<m>` | remove proof lines immediately | **Implemented** |
| `:insert <n>` | add new unchecked lines before line `n` | **Implemented** (tentative in plain) |
| `:formula [<n>]` \| `:formula <n>-<m>` \| `:formula cancel` | retype formula constraints in place (`opb>` prompt), reverifying the buffer; `cancel` undoes the last commit | **Implemented** |
| `:debug` | step through the buffer with breakpoints (`debug>` prompt) | **Implemented** |

## Commands in detail

### Formulas and loading

OPB only: the formula file is read as plain text. CNF/WCNF and compressed
formulas are not supported.

- **`:load <file>`** — loads a formula and starts a new session (buffer,
  undo history, and breakpoints are discarded), the same as passing the
  path on the command line. Prints `Loaded N constraints from <file>`.
  The formula is checked with `veripb` first. If the file can't be read or
  `veripb` rejects it, the error is printed (with line and column in your
  file) and the current session is unchanged.

- **`:instance <file.opb|file.pbp|stem>`** — loads a formula and its proof
  with the same stem (`foo.opb` + `foo.pbp`): name either file, or the
  bare stem. Runs `:load` then `:source`, with their usual messages. Both
  files must exist; otherwise it reports which is missing and changes
  nothing. As with `:load`, a formula `veripb` rejects isn't loaded. Use `:load` and
  `:source` separately when the files don't share a stem.

### Inspecting state

- **`:show [filters]`** — lists the constraint database after the checked
  prefix, skipping deleted constraints, e.g.
  `ConstraintId 4: 1 x1 1 x2 >= 1 [derived]`. Prints `No constraints
  match.` when nothing matches. The database comes from replaying the
  checked prefix with `veripb --dump-database`, cached until the session
  changes; a rejected line never changes it. Filters are space-separated
  and combine with AND:
  - a variable name (`x10`) — constraints mentioning it; an unknown name
    is an error
  - a variable glob (`i[vertex0]*`) — constraints mentioning any variable
    matching the pattern (`*` matches any run of characters, anywhere,
    any number of times); no match gives an empty result, not an error
  - an ID (`5`) or inclusive range (`5-12`)
  - `core` or `derived`
  - a label (`@sum`) — the constraint it names; an undefined label is an
    error
  - a label glob (`@amo*`) — constraints with a label matching the
    pattern
  - examples: `:show x3 core`; `:show i[vertex0]*`; `:show @amo*`

  Each constraint is shown with its labels ahead of it. Labels come from
  the formula (each line's leading `@label`s, one per constraint) and from
  checked proof lines (`@name pol ...`); a later definition of a label
  replaces an earlier one, as in veripb. Labels set by an `e` rule
  (`@name e ...`, which names an existing constraint) are not shown.

- **`:list`** — prints the two preamble lines (tagged `[preamble]`;
  not editable or undoable) and every buffer line, numbered continuously
  so the numbers match the Proof pane and the checker's `line N:` output
  (your first rule is line 3). These are line numbers, not
  `ConstraintId`s. Unchecked lines are tagged `[unchecked]` (or
  `[unchecked, failed: ...]` for the rejected one), breakpoints
  `[breakpoint]`. Does not run `veripb`. The closing
  `output`/`conclusion`/`end` lines that `:check`/`:save` add are not
  listed.

- **`:objective`** — prints the current objective (including any `obju`
  updates), the best value logged by any solution rule so far, and the
  best value logged with checked-deletion guarantees (the one `conclusion
  BOUNDS` uses). Values come from replaying the checked prefix with
  `veripb --dump-objective`. For a formula with no objective, prints
  `No objective — this is a satisfaction problem.`

- **`:preserved`** — prints the preserved variable set after the checked
  prefix (the formula's `preserved:` set as changed by
  `preserved_add`/`preserved_rm`), sorted by name. If a proof line changed
  it, also prints which line last did and the formula's original set:

  ```
  Preserved set (3): x1 x3 x5
    last changed by line 7; the formula declared: x1 x3
  ```

  If the formula has no `preserved:` line, says so. The set is read from
  `veripb`'s trace of the last `preserved_add`/`preserved_rm` line; if
  none has run, `veripb` is not called. Does not show whether a witness
  has been used. Works mid-edit and in `:debug`.

- **`:explain [<n>]`** — explains proof line `n` without changing the
  session. With no `n`: the rejected line if there is one, else the last
  checked line. Any other `n` must be a checked line; the preamble, an
  unchecked line, or a line past the end is an error.

  For a checked line:
  - `pol`: the step-by-step table (each operator applied and the running
    constraint).
  - `red`: the substitution witness and every proof goal with how it was
    proved.
  - `rup`: the `ConstraintId N: ...` line, then `Line n needed:` and the
    minimized set of hints the conflict actually used — constraint IDs
    and/or `~` (the rule's own negation), whatever hints were typed. A
    lone `~` means the negation alone reached the conflict.
  - any other rule: the `ConstraintId N: ...` line.

  For the rejected line: the rejection reason (as shown by `:list` and
  `:save`). For a `rup` line, also: the hints you typed, any typed ID not
  in the database (deleted or never derived), and the result of
  re-checking without hints — either "Without hints it DOES check" with
  the hints actually needed (the hint list was the problem), or "Without
  hints it still fails" (the constraint is not RUP-implied).

### Checking a conclusion

- **`:check [<conclusion>]`** — appends `output NONE;`, `conclusion
  <conclusion>;`, and `end pseudo-Boolean proof;` to the *checked prefix*
  (unchecked lines are ignored), runs `veripb` on that, and prints the
  result: `s VERIFIED ...` on success, the checker's error on failure. The
  session is not changed. The argument takes the `conclusion` rule's
  syntax: `SAT`, `UNSAT`, `UNSAT : 5`, `BOUNDS 0 10`, `NONE`,
  `ENUMERATION_COMPLETE ...`, etc.

  With **no argument**, `:check` tries these in order and prints the
  first that verifies:
  - `UNSAT` — needs a contradicting constraint in the database;
  - `SAT` — needs no objective and a solution logged by `sol`/`solx`;
  - `BOUNDS v v` — only when there is an objective and its best value
    with checked-deletion guarantees (from `veripb --dump-objective`, as
    `:objective` shows) is known; `v` is that value. The lower bound
    still has to be derived.

  Failed candidates print nothing. If none verifies, prints
  `NOT YET CONCLUDED` (not a proof-format result). `BOUNDS lo hi` with
  `lo ≠ hi`, `ENUMERATION_COMPLETE`, and `ENUMERATION_PARTIAL` are never
  tried automatically; give them as an argument.

  Limitation: after you have closed the proof by typing the closing lines
  as rules, every candidate fails, so bare `:check` prints `NOT YET
  CONCLUDED`; `:check <conclusion>` shows the error ("Expected end of file
  (EOF) but found `output` ...").

### Changing the buffer

- **`:undo [n]`** — undoes the last `n` actions (default 1). Every command
  that changes the formula or buffer records the previous state; rejected
  lines, no-ops, and read-only commands record nothing. Undo restores the
  formula (including its objective and `preserved:` lines) and the buffer
  (its lines, checked length, and rejected line). One action is: an
  accepted proof line, a `:delete`, a whole `:source`, a whole `:verify`,
  a whole plain-frontend `:edit`/`:deassert`/`:insert`/`:formula`
  session, or one commit in the TUI's Vim mode (`x`, `dd`, an `Insert`
  Esc, an `Insert` Enter line split, or an `Insert` arrow move off a
  changed line). `:undo 3` goes back 3 actions.

  - At most 200 actions are kept.
  - `:load` and `:instance` (new session) and `:reset` clear the undo
    history. `:source` into a non-empty buffer also clears it; the
    `:source` itself can still be undone.
  - `:undo` and `:formula cancel` are not themselves undoable (no redo).
  - Breakpoints are not affected by `:undo`.
  - In a `:formula` session, `:formula cancel` undoes the last commit and
    stays in the mode; `:undo` after `:done` undoes the whole session.

- **`:source <file.pbp>`** — replaces the buffer with the file's non-blank
  derivation lines, **unchecked**; the formula is kept. If the buffer was
  not empty it is reset first (as `:reset`, including undo history and
  breakpoints) and `Cleared N existing proof line(s).` is printed. Prints
  `Loaded N line(s) from <file> into the buffer — nothing checked yet.
  :verify when ready.` Run `:verify` to check the lines.

  - A leading `pseudo-Boolean proof version ...` header is skipped.
  - A leading `f N;` matching the formula's constraint count is skipped;
    a non-matching one is kept, so `:verify` reports the mismatch.
  - Closing `output`/`conclusion`/`end pseudo-Boolean proof;` lines are
    dropped.
  - An unreadable file is an error and changes nothing.
  - Multi-line constructs (`red ... : subproof ... qed;`) load as
    ordinary lines.

- **`:verify`** — checks the unchecked lines one at a time from the first
  unchecked line, committing each as it passes. At the first rejection it
  stops: in the TUI the line gets the rejected-line background and the
  lines after it stay dimmed; in plain mode `:list` tags it
  `[unchecked, failed: ...]`. No line is discarded. Fix the line (`:edit`
  or the Vim editor) and run `:verify` again to continue from there.
  `:undo` reverts a whole `:verify` as one action.

  A line typed at `pbp>` is checked immediately only when no unchecked
  lines precede it; otherwise it is appended unchecked and `(added to the
  buffer, unchecked — N line(s) now pending; :verify when ready)` is
  printed.

- **`:save <file.pbp> [<conclusion>]`** — writes the preamble, every
  buffer line (checked or not), and `output NONE; conclusion
  <conclusion>; end pseudo-Boolean proof;`. Always writes. With an
  explicit conclusion (same syntax as `:check`), it is first checked
  against the checked prefix; failure is a warning, not an abort. With no
  conclusion, `:save` tries the same candidates as bare `:check` and falls
  back to `conclusion NONE` with a reminder that `NONE` proves nothing.
  Unchecked lines or a rejected line produce a warning. The session is not
  changed.

- **`:reset`** — clears the buffer (checked or not), the undo history,
  and breakpoints; the in-memory formula is kept. (`:load` re-reads the
  formula file instead.) Reports a no-op if the buffer is already empty.

- **`:delete <n>` / `:delete <n>-<m>`** — removes the lines immediately, in
  both frontends. If a removed line was checked, every later line becomes
  unchecked (but stays); `:verify` rechecks them. Recover with `:undo`.
  In the TUI's Vim mode, `dd` deletes the cursor line and `x` the
  character under the cursor; if `dd` empties the proof, Vim mode exits.

- **`:edit [<n>]` / `:edit <n>-<m>`** *(tentative in the plain frontend)* —
  retypes buffer lines in place; with no argument, the last buffer line.
  **Retyping never checks the new text and never changes other lines.**
  If the retyped line was checked, it and every later line become
  unchecked; run `:verify` to recheck. Lines inside `red ... : subproof
  ... qed;` blocks are edited the same way. Line numbers are as in
  `:list`; a preamble line or a line past the end is an error.

  - **Plain**: `:edit` queues the last line, `:edit n` line `n`,
    `:edit n-m` the range. Each queued line is printed as a reference;
    the line you type replaces it and the next queued line is shown. A
    bare Enter keeps the shown line. Once the queue is empty, further
    lines are inserted (extending the range). `:skip` deletes the queued
    line without replacement. `:done` ends the edit, leaving remaining
    queued lines unchanged. `:cancel` restores the buffer to its state
    when the edit began. While editing, only `:skip`, `:done`, `:cancel`,
    `:show`, `:list`, `:objective`, `:preserved`, `:check`, and `:explain`
    are accepted; any other `:`-command is refused ("Only :show, ... work
    while editing"), and every line not starting with `:` is replacement
    text. The read-only commands leave the queue unchanged. The prompt is
    `edit>` (for `:deassert`, `edit (deasserting <constraint>)>`).

  - **TUI**: `:edit` (cursor on the last line) or `:edit n`/`:edit n-m`
    (cursor on `n`; nothing is queued) opens the Vim-style editor in the
    Proof pane. So do double-clicking a Proof line, focusing the Proof
    pane and pressing Enter on an empty prompt (cursor on the topmost
    visible line), and `:deassert`/`:insert` (which start in `Insert`).
    Every change commits immediately and is a separate undo step.

    The editor starts in `Normal` mode (status line `-- NORMAL --`;
    focus moves to the Proof pane):

    | Key | Action |
    |---|---|
    | `h`/`j`/`k`/`l`, arrows | move the cursor; edits on a preamble line are refused |
    | Ctrl+←/→ (or Alt+←/→) | move a word at a time |
    | `i` / `a` | enter `Insert` at / after the cursor |
    | `o` / `O` | add a blank unchecked line below / above and enter `Insert` on it |
    | `x` | delete the character under the cursor |
    | `d` `d` | delete the cursor line |
    | `b` | toggle a breakpoint (`●` in the gutter; see `:debug`) |
    | `:` | leave the editor; the prompt opens with `:` typed |
    | `Esc` | leave the editor |

    `Insert` mode (status line `-- INSERT --`) uses the prompt's line
    editor (←/→, Home/End, Backspace/Delete, Ctrl/Alt+←/→), with the text
    shown in the Proof pane at the cursor line:
    - Esc commits the line (unchecked) and returns to `Normal` on it.
    - Enter splits the line at the cursor: the rest becomes a new next
      line and `Insert` continues at its start (one undo step).
    - ↑/↓ move to the previous/next proof line, keeping the column where
      possible; ← at the start / → at the end of a line move to the
      neighbouring line. Moving off a line commits it only if it changed.
      Prompt history is not available in `Insert`.

    The terminal cursor is shown in the Proof pane in both modes. The
    range form of `:edit n-m` only sets the starting line.

- **`:deassert [<n>]`** *(tentative in the plain frontend)* — opens an
  `a`-rule (unchecked assertion) for replacement by real derivation
  steps: the first one, or line `n`. Errors if `n` is not an `a`-rule, or
  (bare) if none remain ("no assertions left").

  On opening it prints the database constraints that mention any
  variable of the assertion, ranked by how many of its variables they
  share, then by recency:

  ```
  Constraints mentioning x1, x2:
    ConstraintId 7: 1 x1 1 x3 >= 1 [core]
    ConstraintId 3: 1 x2 1 x4 >= 1 [derived]
    ... (+12 more — :show x1 for everything mentioning just that one)
  ```

  Variables are found by looking up each whitespace-separated word of the
  assertion (after stripping `~`). Nothing is printed if it mentions no
  variables. The assertion's own database entry may appear in the list.

  - **Plain**: uses the `:edit` queue flow, starting with the assertion
    line; it always waits for `:done`, however many lines you type.
    `:skip`/`:cancel` work as in `:edit`. The prompt is
    `edit (deasserting <constraint>)>`.
  - **TUI**: opens the Vim editor in `Insert` at column 0 of the
    assertion line. While that `Insert` session lasts, the Output heading
    reads `Output (deasserting <constraint>)`, the Database heading
    `Database (N) (deasserting <constraint>)`, and Database rows sharing
    a variable with the assertion are highlighted (updated every frame).
    These clear when the line is committed. Use `o`/`O` in `Normal` to add
    more lines. Double-clicking an `a`-rule line in the Proof pane runs
    `:deassert` on it.

- **`:insert <n>`** *(tentative in the plain frontend)* — adds new
  unchecked lines before line `n`. `:insert <last line + 1>` appends at the
  end, unchecked.
  - **Plain**: the `:deassert` queue flow with an empty queue: type lines,
    `:done` to finish, `:cancel` to restore the buffer.
  - **TUI**: adds one blank unchecked line at `n` and enters `Insert` on it
    (like `O`). Esc commits and returns to `Normal`; Enter splits.

  **Tentative in the plain frontend**: the plain `:edit`, `:deassert`, and
  `:insert` commands (and `:skip`/`:cancel`/`:done`) may be removed;
  `:help edit` marks them so.

### Editing the formula

- **`:formula [<n>]` / `:formula <n>-<m>`** — retypes formula lines in
  place (prompt `opb>`): the line holding constraint ID `n`, the lines
  holding IDs `n` to `m`, or with no argument the last line. A line that
  loads as two constraints (`<==>`, `=`) is edited as a whole. A
  replacement must load as the same number of constraints as the line it
  replaces, so constraint IDs stay the same; to change the count, edit the
  OPB file and `:load` it. Retype any `@label`s you want to keep. A
  trailing `;` is added if missing.

  Each committed replacement reverifies the whole buffer against the new
  formula. Lines that still check stay checked; the first failing line
  and everything after it stay in the buffer, unchecked, for you to fix
  with `:edit`/`:delete`/`:insert` and `:verify`. Across consecutive
  `:formula` edits, reverification uses the longest buffer from the
  start of the run, so a later edit can recover lines an earlier edit
  broke; any other command ends the run.

  **`:formula cancel`** undoes the most recent commit (constraint and
  buffer together), in the mode or at the ordinary prompt, as long as
  nothing else has been done since. `:undo` after `:done` undoes the whole
  session.

  Unrelated to the `f <n>;` proof rule, which checks the constraint count.

  - **Plain**: `:formula n` queues one constraint, `:formula n-m` a range.
    Each is shown as a reference line; type the replacement, or a blank
    line to keep it. The mode ends after the last queued constraint or on
    `:done`. Only `:done` and `:formula cancel` are accepted as commands;
    any other `:`-command is refused ("Only :done and :formula cancel work
    while editing the formula"), and every line not starting with `:` is
    replacement text.
  - **TUI**: `:formula` (cursor on the last constraint), `:formula n` /
    `:formula n-m` (cursor on `n`), or double-clicking a Formula line
    starts a browse mode in the Formula pane. ↑/↓ move the cursor; the
    prompt shows the current constraint. Typing a replacement locks the
    cursor until you press Enter (commit, reverify, and continue browsing
    on the same constraint) or Esc (discard the edit). Esc when not
    editing leaves the mode. `:formula cancel` undoes the last commit;
    `:done` leaves. Any other `:`-command is refused with the same message
    as in plain mode.

### Stepping and breakpoints

- **`:debug`** — enters stepping mode (prompt `debug>`), which moves the
  boundary between checked and unchecked lines:
  - `:step [n]` — check the next `n` unchecked lines (default 1); stops at
    a rejection.
  - `:back [n]` — mark the last `n` checked lines unchecked (default 1);
    nothing is rechecked. Stops at the start.
  - `:continue` — check forward to the next breakpoint (past the current
    one) or a rejection.
  - `:until <n>` — check forward to line `n` or a rejection.
  - `:break` — list breakpoints; `:break <n>` toggles one; `:break clear`
    removes all.
  - `:restart` — mark every line unchecked, keeping them all.
  - `:show`, `:list`, `:objective`, `:preserved`, `:check`, `:explain` work
    as usual.
  - `:done` (or Esc in the TUI) leaves.

  `:step`/`:back`/`:continue`/`:until`/`:restart` are each one undo step.
  Breakpoint changes are not undoable, and `:undo` does not change
  breakpoints.

  Breakpoints follow their lines when lines are inserted or deleted
  before them and survive `:edit`/`:formula` edits. They are cleared by
  `:reset`, by `:source` into a non-empty buffer, and by `:load`/
  `:instance` (new session). `:list` tags them `[breakpoint]`.

  Commands abbreviate to any prefix; an ambiguous prefix picks the first
  match in `:help debug` order (`:c` is `:continue`, `:b` is `:back`).

  Works the same in the plain frontend. In the TUI:
  - Shift+Down steps forward one line, Shift+Up back one. Enter on an empty
    `debug>` line does nothing.
  - `b` in Vim `Normal` mode toggles a breakpoint on the cursor line.
  - Hovering over a Proof line's gutter marker column previews a dimmed
    `●`; clicking there toggles a breakpoint.

### Help

- **`:help [<topic>]`** — with no argument, lists every command with its
  usage and summary, then the proof rules it documents. With a command
  (leading `:` optional) or rule keyword, e.g. `:help rup`, `:help save`,
  prints its usage, summary, and details. Works with no formula loaded.
  For full rule grammar and semantics, see `proof_format_overview.md` in
  the veripb repository.

## Known gaps and limitations

- **Completion** covers command and rule names, file paths for
  `:load`/`:source`/`:save`/`:instance`, `:help` topics, `:theme` names,
  and rule arguments (constraint IDs and variables, not grammar-aware).
  Not completed: paths containing spaces, and rule arguments
  before the rule keyword is unambiguous.
- **Mode-specific completion (TUI only).** In a `:formula` browse, `:`
  completion offers only `:formula cancel` and `:done`; in `:debug`, only
  its own commands and the read-only ones, and bare rule text gets no
  suggestions. `Normal` mode doesn't use the prompt; `Insert` uses the
  normal completion. The plain frontend has no Tab completion at all.
- **Mouse**: no clicking suggestions, no clicking or dragging scrollbars,
  no click-to-position in the prompt, no pane resizing.
- **No progress indicator for `:verify`.** Each check is a blocking
  `veripb` subprocess call and the TUI doesn't redraw until it returns, so
  a large unchecked buffer can leave the screen unchanged for a while.
- **Prompts don't show subproof depth.** The plain prompt shows only the
  mode (`pbp>`, `edit>`, `opb>`, `debug>`); the TUI shows `pbp>`/`opb>`/
  `debug>` or the Vim status (`-- NORMAL --`/`-- INSERT --`).
- **Labels set by `e`** (`@name e <constraint> ;`) are not shown or
  usable in `:show`: the checker's trace doesn't report which constraint
  an `e` rule matched. Proof lines may still use them.
- **No subproof-specific support.** Multi-line constructs (e.g. a `red ...
  : subproof` body) are accepted line by line, but there is no depth
  tracking or goal listing.

## Not yet implemented

- The full propagation trail of a `rup` step (every intermediate
  assignment); `:explain` shows only the needed hints.
- `:goals` — list remaining proof goals inside a subproof.
- `:trace on|off` — verbose checker tracing.
- Subproof-depth prompts and other subproof-specific support.
- CNF/WCNF formulas (the formula is read as OPB text) and compressed
  formulas.
