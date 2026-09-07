# VeriPB REPL — user guide

> **New here?** [QUICKSTART.md](QUICKSTART.md) gets you from a fresh
> checkout to a checked proof step in about five minutes. This document
> is the full reference — every command, what's implemented vs. planned,
> and why things work the way they do.

> **Status: early proof-of-concept.** This document describes the intended
> tool for the summer-school tutorial and tracks, feature by feature, what
> actually works today. Everything marked
> **Not implemented** will error, do nothing, or simply doesn't exist as a
> command yet — if in doubt, try it and see, then check back here.
>
> This file is a living document. As functionality lands, update the status
> column rather than leaving it stale — it's the source of truth for both
> end users and for tracking our own progress.

## What this is

`veripb-repl` is an interactive terminal tool for entering VeriPB proof-format
version 3 (`.pbp`) rules one line at a time against a loaded pseudo-Boolean
formula, and immediately seeing what happened — the derived constraint, an
error with a propagation trail, whatever the rule produced. It's built for
the constraint-programming summer school tutorial: an audience of CS/OR PhD
students and programmers who are comfortable coding but mostly new to proof
logging, trying out `pol`, `rup`, `red`, and friends interactively instead of
editing a `.pbp` file and re-running `veripb` after every line.

It sits on top of the existing `veripb-checker`/`veripb-parser`/`veripb-formula`
libraries — it does not reimplement any checking logic. Every rule you type
is checked by the real VeriPB checker; the REPL only manages *when* and *how*
that checker is invoked.

### Code layout

- `src/session.rs` — the engine: `Session` (formula, the live proof
  buffer plus how much of its front is checked, current checker) and the
  replay machinery (`verify_next`/`drive_forward`, `dry_run_conclusion`)
  shared by every command that needs to check something. Also the fd-level
  capture that collects the checker's own stdout/stderr trace output during
  each replay, so frontends decide where it lands instead of the checker
  printing straight to the terminal.
- `src/output.rs` — the `Output` sink every command emits user-visible
  text through: the plain CLI prints lines immediately, the TUI appends
  them to its scrollback pane.
- `src/commands/` — one file per `:`-command with real logic (`show.rs`,
  `check.rs`, ...). Only `:quit` stays as an inline match arm in the
  frontends' dispatch rather than earning a file.
- `src/plain.rs` — the plain line-based frontend: prompt and stdin loop
  over the shared `commands::dispatch`.
- `src/tui/` — the crossterm multi-panel frontend: `mod.rs` (app state,
  terminal guard, event loop), `layout.rs` (pure rect math), `draw.rs`
  (full-frame rendering), `input.rs` (hand-rolled line editor + history),
  `complete.rs` (Tab completion/suggestion-strip candidates), `theme.rs`
  (the color palettes `:theme` switches between). Drives the exact same
  `commands::dispatch` as the plain frontend.
- `src/main.rs` — arg parsing and frontend selection (`--plain`, or
  automatic when stdin/stdout isn't a terminal).

## Quick start

Build and run instructions and a first worked example live in
[QUICKSTART.md](QUICKSTART.md) — the short version: `cargo run -p
veripb-repl -- <formula.opb>` from the repo root, formula path optional
(`:load <formula.opb>` once you're in, equivalent to passing it on the
command line).

Type proof-format v3 rules at the `pbp>` prompt, one per line, exactly as
they'd appear in a `.pbp` file (including the trailing `;`). Type `:quit`
to exit, or send EOF (Ctrl-D). Every command other than `:load` and
`:quit` needs a formula loaded first — they'll tell you so rather than
crashing if you try them too early.

### The TUI (default) and `--plain`

By default the REPL opens a full-terminal interface: three columns — the
loaded formula, the live constraint database (same content as `:show` with
no filters), and the proof (same as `:list`) — above a full-width output
pane carrying the familiar `pbp>` prompt. The proof column starts with the
two synthesized preamble lines, dimmed, so its line numbers agree with the
`line N:` numbers in checker traces and error messages — your first rule
is line 3. Constraint labels (`@name`, from the OPB file or from
`@label`-prefixed rules) are shown ahead of the constraints they name in
the formula and database columns, matching `:show`. The columns refresh
after every accepted line; command output, checker traces, and errors all
land in the bottom pane's scrollback.

Variable names and `@labels` are highlighted throughout the three columns
— one accent color for variables, another for labels — deliberately
restrained to just those two: no coloring of keywords, operators, or
numbers, which would compete with reading the constraint math rather than
help it. Formula and database constraints are colored exactly, token by
token; proof-panel lines (raw as-typed rule text, with no single fixed
grammar across `rup`/`pol`/`red`/...) are colored by looking up whether
each word names a real variable, which can under-highlight (e.g. a
variable name with punctuation glued directly onto it) but never
mislabels a keyword or constraint-ID hint as a variable.

Every `a`-rule (unchecked assertion) line in the Proof column gets a dark
red background, whole row, flagging it as scaffolding still owed a real
derivation — the same recognition `:deassert` itself uses, so a line is
red exactly when `:deassert <that line>` would work on it. The Proof
pane's Vim-mode cursor row (see below) wins if it's on the same line;
otherwise variable/label coloring still applies on top, same as anywhere
else. The Database column's rows get a background too: dark green for a
core constraint, dark amber for a derived one — in place of a
`[core]`/`[derived]` word, not alongside it. Assertion/derived/core read
as a traffic light — red for unchecked, amber for derived but not yet
core, green for given — a three-way distinction carried by hue, not just
by which of two rows happens to be present. While `:deassert` is active,
every Database row sharing a variable with the assertion being replaced
gets the same accent color the Vim-mode cursor row uses — see `:deassert`
below. Any buffer line past what's currently checked — a `:source`'d
file's contents before the first `:verify`, or anything `:verify` hasn't
reached yet — is dimmed the same way the synthesized preamble is; the
one line `:verify` most recently rejected (if any) gets its own
background instead — a stronger, more saturated red than the assertion
row's, since the two can never actually coincide (an `a`-rule is never
itself rejected) but this one specifically means "broken," not just "not
yet checked" — see `:verify` below.

`:theme [<name>]` switches between palettes — `dark` (the default, a
dark background with light text, tuned for a dark terminal), `light`
(its opposite number: white/light-grey background, near-black text),
`hi-contrast` (true black/white background/text, fully saturated
highlight backgrounds, and a brighter dim, favoring maximum distinction
over subtlety), and `colorblind`. `dark`/`light`/`hi-contrast` all keep
the same color per *role* as each other (assertion stays red, derived
stays amber, core stays green — the same traffic-light reading above),
so switching between them changes how things look, not what they mean —
but red-vs-green is exactly the pairing hardest to tell apart under
red-green color blindness (by far the most common kind), so
`colorblind` breaks from those three hues deliberately: a dark
background like `dark`'s, but vermillion for assertion, yellow for
derived, and blue-green for core, following the
[Okabe-Ito palette](https://jfly.uni-koeln.de/color/) — a research-backed
set specifically validated to stay pairwise distinguishable under
protanopia, deuteranopia, and tritanopia at once, rather than a guess at
"safe" colors — the same three-way meaning, just carried by hues that
don't collide. The Proof pane's rejected-line background isn't part of
this three-way traffic light — it's always a stronger version of
whichever hue that theme already uses for assertion, in every palette
including `colorblind`, since it can't coincide with an actual assertion
row in practice and so never needs to stay distinguishable from one.

This is genuinely app-wide, not just the accents: every character the
TUI prints — pane content, borders, scrollbar chrome, the scrollback,
the prompt, all of it — carries an explicit background and foreground
from the active theme, rather than leaving "plain" text to whatever the
terminal itself happens to default to. That's also why it's all
explicit RGB rather than the terminal's own named colors, which render
unpredictably across different terminal setups. Bare `:theme` reports
the current theme and the available ones rather than erroring; TUI
only — the plain frontend has no color to switch, and just says so.

All four panes scroll: Shift+Tab cycles focus — the focused pane's title
(the bottom pane is titled `Output`, in the separator bar) shows in
reverse video — and PgUp/PgDn move the focused pane by a page, PgUp
always toward earlier content. With the Proof pane focused, Enter on an
empty prompt drops into its Vim-style editor at the top of what's
currently on screen (see `:edit` below) — the keyboard's way in, with no
command to type and no mouse needed. Scrollbars appear exactly where needed,
inside the pane they describe: a `┃` thumb on a dimmed `│` track in the
pane's last column when content overflows vertically, and a `━` thumb on
a dimmed `─` track along its last row when its widest line overruns the
width. Thumb size tracks the visible fraction, position tracks where you
are — the bars are the only overflow signal, so a bar-less pane means
you're seeing everything. With one of the three top panes focused,
↑/↓ scroll it line by line and ←/→ scroll it horizontally — the line
numbers, right-aligned to a fixed width (`  9: `, ` 10: `), stay pinned
at the left edge while the content slides beneath the scrollbars. With Output focused (the default), all four
arrows belong to the prompt — cursor movement, the suggestion strip, and
history, as described above. Focus never affects where typing goes:
always the prompt. The formula pane anchors at
the top; database, proof, and output follow their newest content until
scrolled. Running any command snaps every pane back to its default view.

Any of the four panes can be maximized. Formula, Database, and Proof each
carry two header buttons, `[▭]` (widen) and `[⛶]` (full): click either to
toggle that pane into taking over the whole top row in place of the other
two columns. `[▭]` just widens it — the bottom Output/prompt area keeps
its usual size; `[⛶]` additionally shrinks the bottom area down to its
small-terminal floor so the zoomed pane gets nearly the entire screen.
Output is the mirror image: already full-width with no column-mate to
displace, its header carries only `[⛶]`, which instead shrinks the top
row down to *its* floor — Formula/Database/Proof stay visible as a
three-column strip, just a few lines tall — so Output gets nearly the
entire screen. Only one pane is ever zoomed at a time. Clicking the same
button again — or pressing Esc with nothing else claiming it (Vim mode's
`Normal` sub-mode, a `:formula` browse, or a completion highlight all
take priority) — un-zooms back to the normal three-column view. Entering
Vim-mode editing or a `:formula` browse on a *different* pane while one
is already zoomed (Output included) retargets the zoom there instead of
leaving it hidden or squeezed behind whatever was maximized, at the same
level it was already at.

The mouse works too: the wheel scrolls whichever pane it's hovering over
(without moving focus), Shift+wheel or Alt+wheel — or a trackpad's
sideways scroll — scrolls it horizontally, Output included: its own
scrollbar appears the same way the top panes' do, the moment its widest
line overruns the pane, and the wheel is the only way to move it — ←/→
stay claimed by the prompt whenever Output has focus (the default), same
as they already do for its vertical scrolling. A left click focuses a pane,
and double-clicking a line in the Proof column jumps straight into
Vim-mode editing (`Normal` sub-mode) on that line — or, if that line is
currently an `a` (unchecked assertion) rule, into `:deassert` on it
instead (landing straight in `Insert`) — or, already in Vim mode's
`Normal` sub-mode, just moves the cursor there. Double-clicking a line in
the Formula column does the same for `:formula`'s browse mode. Two modifiers
because terminals
disagree about which they forward:
some reserve Shift+mouse (or Option on macOS) as their own
capture-bypass and swallow it. Between the two modifiers, sideways
trackpad scrolling, and ←/→ with the pane focused, at least one route
works everywhere. One terminal
caveat comes with that: while the REPL has the mouse captured, selecting
text to copy needs your terminal's usual bypass — typically holding
Shift, or Option/Alt on macOS terminals.
Every command behaves identically to the plain frontend — same code, same
output lines, different destination.

Line editing at the TUI prompt: ←/→/Home/End (or Ctrl-A/Ctrl-E), ↑/↓
history, Ctrl-U/Ctrl-K kill to start/end of line, Ctrl-C clears the line,
and Ctrl-D on an empty line (or `:quit`) exits. Ctrl+← / Ctrl+→ move a
whole word at a time — to the start of the word behind the cursor, or the
start of the next one ahead of it, so the two are exact opposites and
repeated presses walk word by word rather than stalling on the gap
between two. Alt+← / Alt+→ do the same, since terminals disagree about
which of the two modifiers they forward rather than swallow (the same
reason the mouse wheel accepts either — see above). Vim mode's `Insert`
sub-mode inherits all of this, prompt and pane sharing one editor; `Normal`
sub-mode gets the same Ctrl/Alt+←/→ word motion too, from its own separate
binding (see the Vim-mode key table below).

Under the prompt, a dimmed suggestion strip appears once you start typing,
filtering as you type the first word: `:`-commands when the line starts
with `:` (a lone `:` shows all of them), rule keywords otherwise. Once a
rule keyword is fully typed, its *arguments* complete too — against live
constraint IDs, live `@labels`, and variable names (bare and negated,
`~x3`) pulled from the current session, not a fixed registry, so what's
offered changes as the proof grows. It's not grammar-aware — a rule like
`red` gets offered constraint IDs alongside variable names in its
substitution even though only variables ever belong there — but for a
rule like `pol`, where an operand can legitimately be either a database
reference or a fresh literal to push, both really do apply. Nothing
offered before a formula's loaded, since there's nothing yet to
reference. Among `:`-command candidates, `:instance` is always
pre-selected — highlighted before you've pressed an arrow — since
loading a formula/proof pair to work on is common enough, loaded session
or not, that a bare Tab should be able to land on it immediately; an
actual arrow press overrides this the same as it would override any
other highlight. Before a session exists, the candidate list itself is
also reordered: only `:instance`, `:load`, `:help`, `:theme`, and `:quit`
do anything useful with nothing loaded yet, so those five sort first (in
that order), with everything else following in its usual registry order.
The strip is a fixed six rows whenever visible, so the prompt only ever sits
at two heights — strip shown or strip hidden — instead of drifting with
the match count. ↑/↓ move a highlight through the candidates (wrapping,
and sliding the window when the list is longer than the strip); Enter or
Tab inserts the highlighted candidate into the line, and Esc drops the
highlight. Enter only ever *submits* when nothing is highlighted — so
accepting a suggestion and running the line are always two distinct
keystrokes. With no highlight, Tab completes outright when one match
remains, else to the longest common prefix. A completed command or rule
name gets a trailing space (ready for whatever comes next), and a common
prefix that would otherwise land on a bare trailing `.` — e.g. `:instance`
offering both a `.opb` and a `.pbp` for the same stem — stops one
character short instead, since the dot alone is never a valid path (for
`:instance` specifically, stopping at the bare stem is a complete,
loadable completion in its own right). History and the strip never
fight over the arrows: ↑ from an empty line starts a history walk, and
for as long as you're walking, the strip stays hidden and ↑/↓ keep
stepping through history — even when a recalled line would have matches.
The walk ends when you edit the recalled line (it's a fresh line of its
own then, and the strip takes the arrows back), submit it, step ↓ past
the newest entry, or press Tab — which ends the walk and completes the
recalled line in the same press. After a full completion the strip keeps showing that
command's usage line, as the cue for what arguments come next. The strip
and Tab share one registry with `:help`, so the three can never disagree
about what exists.

In the argument of `:load`, `:source`, and `:save`, Tab and the strip
complete file paths instead — relative, absolute, or `~/`-prefixed (`~` is
kept in your spelling; the commands themselves expand it to your home
directory when they run, in both frontends). Directories always show and
complete with a trailing `/` so the next Tab drills inside — but regular
files are filtered to what that command actually deals in: `.opb` (and
its compressed forms, `.opb.gz`/`.gzip`/`.zst`/`.zstd`/`.xz` — genuinely
loadable, since `:load` transparently decompresses by extension) for
`:load`; plain `.pbp` for `:source` and `:save` (neither reads nor writes
a compressed stream). Matching is case-insensitive. Dot-files stay hidden
until you type the leading dot, shell-style; Tab on an empty argument
lists the current directory (filtered the same way); and paths containing
spaces aren't completable (though the commands themselves accept them).
`:help`'s argument completes too, against the same topic registry the
command answers from.

Pass `--plain` for the original line-based frontend. It's also selected
automatically whenever stdin or stdout isn't a real terminal, so piping a
script of commands in (or capturing output) keeps working with no flag.

## Walkthrough

Using `tests/instances/correct/version3/redundance_rup.opb` /
`redundance_rup.pbp` as a worked example:

```
$ cargo run -p veripb-repl -- tests/instances/correct/version3/redundance_rup.opb
Loaded 3 constraints from tests/instances/correct/version3/redundance_rup.opb
pbp> red 1 x1 1 x2 >= 1 ;
  ConstraintId 4: 1 x1 +1 x2 >= 1
pbp> e 1 x1 1 x2 >= 1 : -1;
pbp> output NONE;
pbp> conclusion NONE;
pbp> end pseudo-Boolean proof;
s VERIFIED NO CONCLUSION
pbp> :quit
```

A few things worth understanding about *why* it behaves this way, not just
that it does:

- **Every line is independently re-checked from scratch.** There's no
  long-lived checker carrying state between lines. Instead, the REPL keeps a
  buffer of every proof line, along with a count of how many of them from
  the front are currently checked; each new line is checked by rebuilding a
  synthesized proof file (a version-3 preamble, that checked prefix, then
  your new line) and running the *real* checker over it from the beginning.
  If that succeeds, the line joins the checked prefix. If it fails, nothing
  is kept — the line isn't even added — so your session is exactly as it was
  before you typed it, and a bad line is always fully recoverable by just
  trying again. That instant check happens only when nothing is already
  sitting unchecked ahead of it; if something is, the new line just joins
  the end of that unchecked tail rather than being checked out of order
  (see `:verify` below).
- **`ConstraintId N: ...` output is the checker's own trace output**, not
  something the REPL renders itself — it's the same output you'd get running
  `veripb --trace-lines` on a file. Rules that don't derive a tracked
  constraint (`e`, `output`, `conclusion`, ...) don't print an ID line; that's
  expected, not a sign anything went wrong.
- **`output`, `conclusion`, and `end pseudo-Boolean proof;` are real checked
  rules**, not just punctuation — typing `conclusion UNSAT;` when your
  derivation doesn't actually justify it will fail exactly like a bad `rup`
  would. If you complete the full sequence, the checker prints its own
  `s VERIFIED ...` line, same as it would for a file-based `veripb` run.
- **`conclusion NONE` is a real no-op, not "nothing proven yet."** Checked
  it against the source (`ConclusionRule::compute` in
  `veripb-checker/src/rules/conclusion.rs:47` — the `None` arm is empty): it
  performs no validation at all and verifies trivially even with zero lines
  derived. `s VERIFIED NO CONCLUSION` above doesn't mean the redundance/`e`
  steps proved anything about the formula — it means no claim was made. Real
  conclusions (`UNSAT`, `SAT`, `BOUNDS ...`) *are* genuinely checked; see the
  next walkthrough.
- **Once you type `end pseudo-Boolean proof;`, the proof is closed.** Valid
  v3 syntax requires nothing after it, so anything you type next will be
  rejected — not a bug, just the natural consequence of the replay model.
  Don't type the closing lines for real unless you're deliberately finishing
  the session; there's no way back in afterwards yet (see `:undo`/`:save`
  below). Use `:check` (next section) to see the verification result
  without closing anything.

### Walkthrough: building towards a real conclusion

`redundance_rup.opb` above is satisfiable, so there's no interesting `SAT`
or `UNSAT` claim to build towards, which is why `conclusion NONE` made
for an unremarkable ending above. `examples/tiny_unsat.opb` (three
constraints: `x1 ∨ x2`, `¬x1 ∨ x2`, `¬x2` — jointly unsatisfiable, but
not obviously so from any single constraint) gives you something to
actually derive:

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

`rup 1 x2 >= 1 ;` derives `x2` by negating it (`x2 = 0`) and unit-propagating
against constraints 1 and 2 until they conflict. `rup >= 1 ;` — an
empty-left-hand-side constraint, unconditionally false — is the standard v3
idiom (see `proof_format_overview.md`) for materializing a contradiction;
it's needed as an explicit step because `conclusion UNSAT`'s check
(`check_contradiction` in `veripb-checker/src/rules/conclusion.rs:243`)
looks for one literal contradicting constraint already in the database, not
for an implicit conflict between separate unit constraints — constraints 3
and 4 alone being jointly unsatisfiable isn't enough on its own.

## Proof rule syntax

Rules are typed exactly as v3 `.pbp` syntax — see
[`proof_format_overview.md`](../proof_format_overview.md) at the repo root
for the full grammar and semantics of `pol`, `rup`, `red`, `e`, `del`, `ia`,
and the rest. The REPL doesn't (yet) offer any syntax help beyond what the
checker's own error messages give you.

## Command reference

Proof rules are typed bare. REPL commands are `:`-prefixed, and may be
abbreviated to any unambiguous prefix — `:q` is `:quit`, `:ob` is
`:objective` — while an ambiguous prefix (`:s`) errors and lists its
candidates rather than guessing. Status reflects this codebase today, not
the eventual design.

| Command | Meaning | Status |
|---|---|---|
| *(bare rule text)* | check and, if accepted, apply a v3 proof-format rule | **Implemented** |
| `:quit` | exit the REPL | **Implemented** |
| `:theme [<name>]` | TUI only: switch the color palette (`dark`/`light`/`hi-contrast`/`colorblind`) | **Implemented** — see below |
| `:check [<conclusion>]` | bare: auto-tries `UNSAT`, `SAT`, and (once known) `BOUNDS v v` — the conclusions needing no extra input — and shows whichever the real checker accepts, else a REPL-native "not yet" status. With an argument: non-destructively test whether the session would verify with that conclusion, without closing it | **Implemented** — see below |
| `:explain <n>` | show an expanded derivation for proof line `n` — `pol`'s full step-by-step table, `red`'s substitution witness and proofgoals, or (every other rule, for now) the same baseline `ConstraintId N: ...` line every rule prints when traced | **Implemented** — see below |
| `:list` | print every buffer line, checked or not (tagged `[unchecked]`, or `[breakpoint]` if one's set there), numbered by position | **Implemented** — see below |
| `:load <file>` | load a formula (resets session) | **Implemented** — OPB only, same as startup; see below |
| `:instance <file.opb\|file.pbp\|stem>` | load a formula and its matching proof together — `:load` then `:source`, from either half of the pair or their bare stem | **Implemented** — see below |
| `:show [filters]` | list database constraints; filter by variable, ID/range, or label (each exact or `*`-glob), or core/derived | **Implemented** — filters combine (AND); see below |
| `:why [all\|needed]` | explain the last successful `rup` step — or, if a line is currently rejected, explain that instead. Bare/`needed` shows the minimized set of already-derived constraints (and/or the rule's own negation) the checker actually needed; `all` — the fuller literal-by-literal propagation trail — needs upstream changes | **Implemented** — `needed` only; see below |
| `:objective` | show current objective and best known bounds | **Implemented** — see below |
| `:goals` | inside a subproof, list remaining proof goals | Not implemented |
| `:undo [n]` | undo the last *n* actions (default 1) — not lines, whatever counted as one thing done | **Implemented** — see below |
| `:source <file.pbp>` | load a proof from a file into the buffer, unchecked | **Implemented** — see below |
| `:verify` | check the buffer's unchecked tail, stopping (without losing anything) at the first problem | **Implemented** — see below |
| `:save <file.pbp> [<conclusion>]` | write the entire buffer to a `.pbp` file, checked or not | **Implemented** — see below |
| `:trace on\|off` | verbose checker tracing for subsequent lines | Not implemented — tracing is currently always scoped internally to just-accepted lines |
| `:help [<topic>]` | command list, or details on a command or proof rule | **Implemented** — see below |
| `:reset` | drop all proof lines, keep the formula | **Implemented** — see below |
| `:edit [<n>]` \| `:edit <n>-<m>` | *(tentative in plain)* plain: queue buffer lines to retype in place; TUI: Vim-style modal editing of the Proof pane itself, cursor starting on line `n` (or the last buffer line, bare) — also reachable with no command at all, by focusing the Proof pane and pressing Enter. Never checks the new text | **Implemented** — see below |
| `:deassert [<n>]` | *(tentative in plain)* replace an `a` (unchecked assertion) rule — the first one, or line `n` specifically — with one or more real derivation steps, unchecked until `:verify` | **Implemented** — see below |
| `:delete <n>` \| `:delete <n>-<m>` | remove proof line(s) immediately, checked or not | **Implemented** — see below |
| `:insert <n>` | *(tentative in plain)* add new, unchecked proof line(s) before line n | **Implemented** — see below |
| `:formula [<n>]` \| `:formula <n>-<m>` \| `:formula cancel` | switch into formula-editing mode (`opb>` prompt) and retype constraint(s) in place, reverifying the whole buffer against each commit (unchecked tail preserved, never discarded, on a partial failure); `cancel` undoes the most recent commit | **Implemented** — see below |
| `:debug` | switch into stepping/breakpoint mode (`debug>` prompt): `:step`/`:back` move `checked_len` one line at a time, `:continue`/`:until <n>` run to the next breakpoint (or a one-off line) or a rejection, `:break`/`:break <n>`/`:break clear` manage breakpoints, `:restart` retracts to the top without discarding anything. `:show`/`:list`/`:explain`/`:why`/`:objective`/`:check` all still work; `:done` (or Esc, in the TUI) leaves | **Implemented** — see below |

## What works today, concretely

- Loading a formula in **OPB format only** — pass its path as the sole,
  optional command-line argument, or load/replace one mid-session (or for
  the first time, if the REPL was started with no argument) with `:load
  <file>`. CNF/WCNF formula loading is not wired up in the REPL (the
  underlying library supports it; the REPL just doesn't call it yet).
- Typing v3 rules one at a time and having them checked against the real
  checker, with derived constraints, deletions, and propagation-failure
  trails printed exactly as the checker itself would print them.
- A rejected line leaves the session state untouched — you can immediately
  try a corrected line.
- A fully completed proof (through `output`/`conclusion`/`end pseudo-Boolean
  proof;`) prints the real `s VERIFIED ...` result line.
- `:show [filters]` — lists the current constraint database. With no
  filters, shows everything still present (deleted constraints are skipped
  automatically). Constraint labels are displayed ahead of the constraint
  they name — `ConstraintId 4: @sum 1 x1 +1 x2 >= 1 [derived]` — whether
  defined in the OPB file or by an `@label`-prefixed proof rule
  (relabeling follows the format's overwrite semantics: the label shows
  on whichever constraint it currently names). Filters are space-separated and combine with AND:
  - a variable name (e.g. `x10`) — only constraints mentioning that
    variable; an unrecognised name reports an error rather than silently
    matching nothing, since there's no "did you mean...?" suggestion yet
  - a variable glob (e.g. `i[vertex0]*`) — any constraint mentioning *any*
    variable whose name matches, `*` standing in for any run of
    characters, anywhere in the pattern (not just the end) and any number
    of times. Handy for a family of generated, indexed variable names
    where you don't want to spell out or already know every one. Unlike
    an exact name this never errors for "no match" — it's a loose filter
    by design, so no matches just means an empty (or otherwise
    filtered-down) result, same as an ID range that happens to be empty
  - a single ID (e.g. `5`) or an inclusive range (`5-12`)
  - a label (e.g. `@sum`) — just the constraint it currently names, an
    error if no such label exists — or, the same way, an `@`-prefixed
    glob (`@i[vertex0]*`) for any label matching, with the same "no match
    is just an empty result" behavior as a variable glob
  - `core` or `derived` — status filter
  - example: `:show x3 core` — core constraints mentioning `x3`
  - example: `:show i[vertex0]*` — every constraint mentioning a variable
    whose name starts with `i[vertex0]`

  Implementation note: this reads live off the checker instance retained
  after the most recently *accepted* line — a rejected line never updates
  what `:show` sees, consistent with the "state unchanged on rejection"
  guarantee described above.

- `:check [<conclusion>]` — builds `output NONE;` + `conclusion
  <conclusion>;` + `end pseudo-Boolean proof;` on top of the buffer's
  current *checked prefix* (any unchecked tail is deliberately ignored —
  a conclusion only means anything once the derivation leading to it has
  actually verified) in a throwaway buffer, checks it with a fresh
  checker, prints
  the result (`s VERIFIED ...` on success, the real checking error on
  failure — e.g. if you claim `UNSAT` but haven't actually derived a
  refutation), and discards that checker either way. The live session is
  never touched, so you can `:check` as often as you like mid-proof. The
  argument accepts the same syntax as the real `conclusion` rule — `SAT`,
  `UNSAT`, `UNSAT : 5`, `BOUNDS 0 10`, `NONE`, `ENUMERATION_COMPLETE ...`,
  etc. — validated by the real parser, not reimplemented by the REPL.

  With **no argument**, `:check` auto-detects: it tries `UNSAT`, then
  `SAT`, via the same non-destructive dry run as above, and shows whichever
  one the real checker actually accepts. `UNSAT` only needs a contradiction
  somewhere in the database (checked against `ConclusionRule::compute` in
  `veripb-checker/src/rules/conclusion.rs`); `SAT` (with no hint) only
  needs no objective present and a solution already logged via an earlier
  `sol`/`solx` rule — neither needs anything from you beyond what's
  already in the session.

  For an **optimization problem**, a third candidate joins the list once
  it's derivable for free: if the session has an objective and
  `Context::best_valid_objective_value` — the best solution value logged
  with checked-deletion guarantees, the same figure `:objective` reports
  as "checked-deletion guarantees" — is already known, `:check` also
  tries `BOUNDS v v` for that exact value. The value is read straight out
  of the session, never guessed, and the *lower*-bound half of a `BOUNDS`
  conclusion still has real work to do (deriving that the objective can't
  do better — see `veripb-checker/src/rules/conclusion.rs`'s `Bounds`
  arm), so this is exactly as safe as the `UNSAT`/`SAT` tries: a
  candidate worth attempting, checked for real, not an assumed success.
  This is what makes `:check` right after a `:source`'d optimization
  proof (or after deriving the matching bound by hand) show the true
  `s VERIFIED BOUNDS v <= obj <= v` instead of `NOT YET CONCLUDED`.

  `BOUNDS lo hi` with unequal bounds, `ENUMERATION_COMPLETE ...`, and
  `ENUMERATION_PARTIAL ...` all take explicit numbers only you can supply
  (which bound? how many solutions?), so they're never auto-tried — ask for
  those explicitly with an argument. A failed auto-detect probe prints
  nothing (confirmed: with tracing off, a failing `dry_run_conclusion`
  produces no stdout output), so trying every candidate and silently
  moving on is safe — you'll never see multiple "it's not this" errors
  dumped on you. If nothing auto-detects, `:check` prints a REPL-native
  `NOT YET CONCLUDED — ...` status instead of running a real `conclusion
  NONE` check, which would verify trivially either way and tell you
  nothing (see the walkthrough above for why that's actively misleading
  rather than just unhelpful). `NOT YET CONCLUDED` is visually distinct
  from `s VERIFIED`
  specifically so it can never be mistaken for genuine proof-format output
  — there's no `NOT READY` conclusion type in the real v3 grammar, and this
  REPL doesn't invent one.

  Caveat: if you *have* already closed the session for real (typed the tail
  lines as actual rules, not via `:check`), running `:check` afterwards will
  produce a confusing "expected EOF" error rather than a clean message — it
  doesn't yet detect "the proof is already closed" as a distinct state.

- `:explain <n>` — an expanded, non-destructive derivation of one already-
  accepted proof line: replays just that line through a fresh, throwaway
  checker with tracing turned up, the same pattern `:check` uses to avoid
  touching the live session. For a `pol` line: the full step-by-step
  reverse-polish table (each operator applied, and the running constraint
  after it). For a `red` line: the substitution witness plus every
  proofgoal and how it was auto-proven — genuinely verbose, since that's
  what `red` actually does. For every other rule (`rup` included, for
  now): the same baseline `ConstraintId N: ...` line every rule prints
  when traced — a real, honest result, not a placeholder; `:why` (see
  below) covers `rup` specifically, with its own kind of detail. `n` must
  name an already-accepted line — the synthesized preamble or anything
  past the end of the proof errors rather than guessing. Read-only, so —
  like `:show`/`:list`/`:objective`/`:check`/`:why` — it also works
  mid-`:edit`/mid-`:deassert`/mid-`:insert` in the plain frontend's
  queue-based flow, without disturbing what's queued. In the TUI's Vim
  mode, typing `:` leaves the editor and hands focus to the ordinary
  prompt with `:` pre-typed — so this (and anything else you'd type
  there) runs the same way it always does once you're back at the
  prompt, but the edit itself is over; the buffer keeps whatever it
  committed, `:edit`/`:deassert`/`:insert` again to resume elsewhere.

- `:why [all|needed]` — explains the *last* `rup` step in the buffer's
  checked prefix (never a specific line number — always whichever one was
  most recently checked), by showing the minimized set of hints the
  checker actually needed to reach its conflict: which already-derived
  constraints (and/or the rule's own negation) genuinely contributed a
  propagation, as opposed to whatever hint list, if any, was typed. Bare
  `:why` and `:why needed` are the same thing. This is real data the
  checker already computes internally for every accepted `rup` step,
  explicit-hint or bare alike —
  `RUPRule::compute` in `veripb-checker/src/rules/rup.rs` builds exactly
  this minimized list either way, filtering a typed hint down to only
  what actually propagated something, or, with no hint typed at all,
  building it fresh via the propagation engine's own trail-minimization —
  but it's normally only ever *used* to write an elaborated (explicit-hint,
  minimized) proof file when the checker's own `--elaborate` output is
  requested, never surfaced back to a human interactively. `:why` gets at
  it by replaying the buffer's prefix through `display_line` — the
  usual non-destructive throwaway-checker pattern, same as `:check`/
  `:explain` — but this time with the checker's elaborator pointed at a
  scratch temp file, then reads back the one `rup <constraint> : <hints>;`
  line it wrote and reports those hints. Entirely a `veripb-repl`-side
  trick: nothing in `veripb-checker`/`veripb-rules` changes for this, on
  purpose, until (if ever) the REPL becomes permanent enough to justify a
  proper in-memory elaboration hook instead of a scratch file. `:why all`
  — the fuller literal-by-literal propagation trail every intermediate
  assignment took, not just the minimized hint set — is a different, and
  genuinely unavailable, kind of data: it exists in the checker only on
  the *failure* path (`trace_failed_with_hints` in `rup.rs`, printed when
  a RUP check doesn't reach a conflict), with no equivalent hook for a
  successful step. Asking for `:why all` says so plainly rather than
  silently falling back to `needed` or doing nothing. Errors with "no
  `rup` step in the proof yet" if the checked prefix doesn't have one.
  Read-only, so it works the same way `:explain` does above — mid-edit in
  the plain frontend's queue, or after leaving the TUI's Vim mode via
  `:` — though since it always targets "whatever's currently the *last*
  `rup` line," that target can itself shift if editing removed or added
  one.

- `:objective` — prints the current objective (`context.objective`, which
  reflects any `obju` updates, not just the formula's original one), plus
  two separately-tracked bound fields from `Context`: `best_objective_value`
  (the best value logged by any `sol`/solution rule so far, checked or not)
  and `best_valid_objective_value` (the subset of that with
  checked-deletion guarantees — the stricter one `conclusion BOUNDS`
  actually relies on). If there's no objective at all (a pure satisfaction
  problem), says so and stops there.

- `:list` — prints the two synthesized preamble lines (tagged
  `[preamble]` — they're supplied by the session, not editable or
  undoable) followed by the PBP lines you've had accepted so far, one per
  line. Numbered as one continuous sequence, so the numbers here, in the
  TUI's proof panel, and in the checker's own `line N:` trace output all
  refer to the same lines — your first accepted rule is line 3, matching
  what a trace or error message will call it. (Not numbered by
  `ConstraintId` — those don't correspond 1:1, since e.g. `e` never adds
  a new constraint.) Doesn't touch the checker at all — quick way to see
  what you've built up. Deliberately still not the closing
  `output`/`conclusion`/`end` lines that `:check`/`:save` synthesize on
  demand — those aren't part of the session until you type them for real.

- `:undo [n]` — undoes the last `n` *actions* (default `1`), restoring the
  session to exactly the state from `n` actions ago. Not line-based:
  every mutating command pushes a snapshot of the state right before it
  ran onto an undo stack, only if that command actually changed something
  (a rejected line, a no-op, or a purely read-only command like `:show`
  pushes nothing); `:undo` pops it back off and restores `formula`,
  `variables`, and the buffer — its lines, how much of it was checked, and
  which line (if any) was known bad — together, then silently rebuilds
  `current_checker` from the restored checked prefix — same replay
  machinery every other command here relies on. An "action" is whatever
  counted as one coherent thing done: a single accepted proof line, a
  `:delete`, an entire `:source`'d file (reverts as one step, not line by
  line), a whole `:edit`/`:formula`
  session in the plain frontend's queue (see below — the TUI differs
  here), or one keystroke's worth of commit in the TUI's Vim mode (an
  `x`, a `dd`, or one `Insert` session's Esc/Enter). `:undo 3` jumps back
  3 actions, discarding the two in between — not "remove 3 lines," the
  old (pre-undo-stack) behavior.

  The stack remembers up to the last 200 actions; further back than that,
  `:undo` reports nothing left. `:load`, `:instance`, and `:reset` are
  hard boundaries — nothing before one of those is reachable afterward,
  matching how each already discards the previous state outright. `:undo`
  itself, and `:formula cancel`, never push their own entry (undoing an
  undo, or cancelling a cancel, isn't supported — no redo).

  For an active `:formula` session specifically: `:formula cancel` undoes
  just the most recent commit and stays in the mode (see `:formula`
  below); `:undo`, from outside the mode (after `:done`), undoes the
  *whole* session — however many constraints were retyped — as one step.
  Different granularities, both useful.

  For precise, line-numbered removal instead of counting back actions,
  `:delete <n>-<m>` remains the tool for that.

- `:source <file.pbp>` — replaces the current buffer with one loaded from
  `file`: the buffer is cleared first (formula kept, same as `:reset` —
  and announced, "Cleared N existing proof line(s)." — whenever there was
  actually something to clear), then `file`'s non-blank derivation lines
  land in it **unchecked**. Nothing is verified on load — `:verify` (see
  below) checks them on request, same as any other unchecked buffer
  content. Mirrors `:load`'s "this is now the X" model, just for the
  proof rather than the formula, rather than appending onto whatever was
  already there.

  This is deliberate, not a missing feature. Checking a whole file's
  worth of lines the instant it loads is exactly what made a single typo
  deep in a long file expensive before: the fix used to be "stop, report
  how far it got, and require a fresh `:source` to try again," discarding
  everything after the failure along with it. Now the file's content
  simply sits in the buffer, editable in place, until `:verify` says
  otherwise — see that entry below for the full story.

  The file's *framing* is handled specially, not fed through, because
  the session synthesizes its own on demand:

  - a leading `pseudo-Boolean proof version ...` header is skipped (the
    synthesized preamble already has one; a second would be rejected);
  - a leading `f N;` whose count matches the loaded formula is skipped for
    the same reason — but one that *doesn't* match is deliberately fed
    through, so a file written for a different formula fails loudly with
    the checker's own error instead of silently misapplying;
  - the closing `output`/`conclusion`/`end pseudo-Boolean proof;` lines
    are never applied and simply dropped — checking a conclusion only
    means anything once the derivation leading to it has actually
    verified, which loading alone doesn't attempt.

  A file that can't even be read (a missing path, bad encoding, ...) is
  caught before anything is cleared, so that case is still a true no-op.

  Multi-line constructs (a `red ... : subproof ... qed;` body, say) load
  as plain, unchecked lines exactly like everything else — there's
  nothing special about them until `:verify` actually replays the body,
  at which point each intermediate line is accepted as "checked fine,
  just wants more input" the same way typing one directly always has.

- `:verify` — checks the buffer's unchecked tail (everything from
  wherever checking last stopped to the end), one line at a time,
  committing each as it passes: `:explain`/`:show`/the Database pane see
  it immediately, exactly as if it had been typed directly. The first
  line that's rejected stops the check right there — in the TUI, that
  line gets its own background in the Proof pane (a stronger version of
  the assertion row's red, described above), with the rest of the still-
  unchecked buffer dimmed below it, same as the preamble; in plain mode,
  `:list` tags it `[unchecked, failed: ...]`. **Nothing after the failure
  is discarded** — every line still sitting in the buffer, checked or
  not, stays exactly as it is. Fix the failing line (`:edit`, or the
  TUI's Vim-mode editor) and run `:verify` again to resume checking from
  there; repeat for however many more rejections it hits, or run to the
  end.

  A bare typed line at the ordinary `pbp>` prompt still checks instantly,
  same as always — that's unrelated to `:verify`, which only ever
  concerns whatever's already sitting unchecked in the buffer (from
  `:source`, or from editing an already-checked line back into the
  unchecked state). If you type a new line while something's still
  unchecked ahead of it, it just joins the end of that unchecked run
  rather than being checked out of order — there's no way to know it's
  valid without first knowing everything before it is.

  No `:verify`-specific undo exists: `:undo`, from outside the mode,
  already reverts everything one `:verify` call committed as a single
  action, the same granularity it already gives `:edit`/`:formula`
  sessions.

- `:help [<topic>]` — with no argument, lists every `:`-command with its
  usage and a one-liner, then names the proof rules it can explain. With a
  topic — a command name (leading `:` optional) or a rule keyword, e.g.
  `:help rup` or `:help save` — prints that topic's usage, summary, and
  details. Works with no formula loaded. The same registry drives the
  TUI's Tab completion and suggestion strip, so help, completion, and
  suggestions always agree; rule summaries are just orientation — 
  [`proof_format_overview.md`](../proof_format_overview.md) remains the
  authority on grammar and semantics.

- `:save <file.pbp> [<conclusion>]` — writes **the entire buffer, checked
  or not**, out as a `.pbp` file: the synthesized preamble, every buffer
  line, and a closing `output NONE; conclusion <conclusion>; end
  pseudo-Boolean proof;` sequence — the same text `Session::listing`
  builds for `:check`, just no longer scoped to only the checked prefix.
  Always writes, regardless of check status — unlike a completed proof's
  closing sequence, the buffer is meant to be freely saved and reloaded
  mid-work, not just once it's fully verified. With an explicit
  `<conclusion>` (same syntax as `:check`'s argument — `SAT`, `UNSAT`,
  `BOUNDS 0 10`, `NONE`, ...), that conclusion is checked first with the
  same non-destructive `dry_run_conclusion` machinery `:check` uses
  (against the checked prefix only — see `:verify` above for why); if it
  doesn't verify, that's reported as a warning, not a reason to abort the
  write. With no argument, `:save` auto-detects exactly like bare `:check`
  does and falls back to `conclusion NONE` (printing a reminder that
  `NONE` is a real no-op, not a verified result) only if nothing
  auto-detects. If any lines are unchecked, or the buffer has a known-bad
  line, a warning names it before the file is written either way. The
  live session is never touched by any of this, so `:save` is safe to run
  mid-proof as often as you like, same as `:check`.

- `:reset` — clears the buffer entirely (checked or not) and rebuilds
  `current_checker` from the formula alone, as if no proof lines had ever
  been typed — without touching `formula`/`variables`/`labels`. Different
  from `:load` with the same path: `:load` re-parses the formula file
  from scratch (and would pick up any changes to it since the session
  started), `:reset` just discards the proof built on top of whatever
  formula is already in memory. A no-op (reported as such) if the buffer
  is already empty.

- `:load <file>` — loads a new formula and resets the whole session
  (buffer cleared, checker rebuilt), same as passing a path on the
  command line at startup — in fact it's the exact same `Session::load`
  call. A bad path or a formula that fails to parse leaves the *current*
  session untouched and prints the error, rather than crashing the REPL or
  leaving you half-reset; same "state unchanged on failure" guarantee as a
  rejected proof line. Still OPB-only, same limitation as startup loading.

- `:instance <file.opb|file.pbp|stem>` — loads a formula and its matching
  proof together: name either half of a pair sharing a stem (`foo.opb` +
  `foo.pbp`, the convention `tests/instances/` follows throughout this
  repo, and how a formula and its proof usually sit on disk), or just the
  bare stem itself (`foo`, no extension) to have both appended, and both
  get loaded — the formula via `:load`, the proof via `:source`, run one
  after the other exactly as if typed separately, same messages, and same
  "loaded unchecked, :verify when ready" outcome. Both files are confirmed
  to exist *before* either is touched — a missing formula, a missing
  `.pbp`, or both report clearly which, session untouched, rather than a
  typo silently leaving you with the formula loaded and no proof to
  follow it. A formula that fails to *parse* is still the same "state
  unchanged" failure `:load` alone gives — existing is checked up front,
  parsing isn't. Compressed formula suffixes (`.opb.gz` and friends,
  `:load` alone still handles those) aren't paired here: an already-
  compressed path just gets treated as a stem and won't resolve to
  anything real, same as any other typo. Doesn't replace `:load`/
  `:source` — reach for those directly when the files don't share a stem,
  or you only want one of the two.

- `:edit [<n>]` / `:edit <n>-<m>` *(tentative in the plain frontend — see
  below)* — retype one or more buffer lines in place; with no argument,
  targets the last buffer line, so a quick "fix what I just typed" needs
  no line number at all. **Retyping never checks the new text, and never
  touches any other line.** If the line being retyped was already
  checked, it — and everything after it, which can no longer be assumed
  to still hold now that something upstream changed — simply becomes
  unchecked again, still sitting right there in the buffer. Nothing is
  lost and nothing is reverified automatically; `:verify` (see above)
  does that on request, and reports exactly how far it gets. Nesting
  needs no special handling: editing a line inside a
  `red ... : subproof ... qed;` block works the same way as any other
  line.

  Line numbers use the same numbering as `:list` and the proof panel
  (preamble lines 1–2, your first rule at 3, and this never shifts —
  since nothing is ever truncated, a line's number is stable for as long
  as it exists); targeting a preamble line or anything past the end of
  the buffer is an error, buffer untouched.

  **The two frontends genuinely differ here** — the TUI has a persistent
  cursor in the Proof pane itself to build a real modal editor on; the
  plain frontend, reading one stdin line at a time, doesn't.

  - **Plain**: `:edit` with no argument queues the last buffer line;
    `:edit n` queues that one line; `:edit n-m` queues the whole range.
    Submitting a queued line's replacement commits it immediately and
    pops the next one into place (shown as a reference line — plain mode
    can't pre-fill a prompt); once the queue drains, further lines just
    get typed and inserted normally (how a range grows longer). A bare
    Enter — nothing typed — keeps the shown line exactly as it is rather
    than doing nothing, since the printed reference reads like a value
    already there; that only applies while a line is actually queued and
    shown, so it has no effect once the queue's drained (nothing to
    keep). And `:skip` drops the currently queued line without replacing
    it, i.e. deletes it outright (how a range shrinks) — a deliberate
    action, unrelated to `:verify`'s "never lose anything" guarantee for
    *rejections*. `:done` leaves the edit — anything still queued and
    unretyped is simply left exactly as it is, nothing discarded — and
    `:cancel` abandons the whole thing at any point before that,
    restoring the buffer exactly as it was when the edit began (no
    re-verification needed, since that prior state was already
    known-good). While an edit is active, only `:skip`, `:done`,
    `:cancel`, and the six read-only commands — `:show`, `:list`,
    `:objective`, `:check`, `:explain`, `:why` — are recognized; everything
    else you type (or don't) is proof-rule text. The read-only ones run in
    place and leave whatever's queued untouched, exactly as they would at
    the ordinary prompt (and, same as always, still refuse a line that
    isn't checked). The prompt reads `edit> ` instead of `pbp> ` for
    the duration — or, specifically for `:deassert` (see below), `edit
    (deasserting <constraint>)> `, so what's being replaced stays visible
    for the whole edit, not just the intro message printed once at the
    start. `:deassert`/`:insert` use this same queue-based flow
    (see below) and get the same bare-Enter behavior.

  - **TUI**: `:edit` (cursor starts on the last buffer line) or
    `:edit n` / `:edit n-m` (cursor starts on `n` — a range only picks
    where the cursor starts, nothing is queued) enters the Proof pane's
    Vim-style modal editor instead — as does double-clicking a line in
    the Proof column (see above), or `:deassert`/`:insert` (below), which
    land straight in `Insert`.

    There's a keyboard route in that needs neither the mouse nor a typed
    command: **focus the Proof pane (Shift+Tab cycles focus, and the
    focused pane's title shows in reverse video), then press Enter on an
    empty prompt.** That costs nothing that was doing anything — a blank
    Enter is otherwise a no-op, and with any text typed the line still
    submits as usual. It starts on the *topmost line the pane is
    currently showing* rather than on the last line the way bare `:edit`
    does, and the difference is deliberate: `:edit` is typed blind at the
    prompt, where "the line I just added" is the obvious target, while
    this is pressed while looking at the pane, very possibly having
    scrolled somewhere specific first — jumping to the end of a long
    proof would be exactly wrong there. `:edit n` remains the way to name
    a line outright. There is no queue here at all: every
    keystroke that changes the buffer commits immediately and is its own
    independently-undoable step (`:undo` reverts one at a time, not a
    whole multi-line edit as one action the way the plain frontend's does).

    Entering the editor moves focus to the Proof pane and hides the
    ordinary prompt behind a `-- NORMAL --` status line — the mode starts
    here, a read-only cursor with single keys as commands rather than
    text:

    | Key | Action |
    |---|---|
    | `h`/`j`/`k`/`l`, arrows | move the cursor (checked or unchecked lines alike; a preamble line is reachable but every edit there is refused, same "nothing to edit there" error `:edit`/`:delete` already give elsewhere) |
    | Ctrl+←/→ (or Alt+←/→) | jump a word at a time on the cursor line — the same word-boundary rule the prompt and `Insert` use, ported here since `Normal` has no line editor of its own to borrow it from |
    | `i` / `a` | enter `Insert` at the cursor column / one past it |
    | `o` / `O` | splice a new, unchecked blank line in below / above the cursor and enter `Insert` on it at column 0 |
    | `x` | delete the character under the cursor |
    | `d` `d` | delete the whole cursor line (a lone `d` does nothing until a second one follows) |
    | `b` | toggle a breakpoint on the cursor line — shows as a `●` marker in the gutter; see `:debug` below (same underlying `Session::breakpoints`, so it's visible from either) |
    | `:` | leave the editor and drop to the ordinary prompt with `:` already typed — the way to reach `:verify`, `:list`, or anything else mid-edit |
    | `Esc` | leave the editor outright, focus back to the ordinary prompt |

    `Insert` reuses the exact same character-level editing the ordinary
    prompt has — ←/→/Home/End/Backspace/Delete, and Ctrl+←/Ctrl+→ (or
    Alt+←/Alt+→) to move a word at a time, all work the same way —
    except what's typed renders live in the Proof pane itself, right at
    the cursor's line, not down at the bottom; the status line reads
    `-- INSERT --` while it's active. (`Normal`'s own Ctrl/Alt+←/→ is a
    separate implementation reaching the same word-boundary behavior,
    since `Normal` has no text buffer of its own to borrow the prompt's
    editor from — see the table above.) Esc *or* Enter both commit the
    line (via the same never-checks-anything path retyping always
    used) and return to `Normal` on the same line, cursor left where
    typing stopped — real Vim's "Enter splits the line in two" isn't
    implemented, so treat Enter as a second Esc here. The real terminal
    cursor sits directly in the Proof pane the whole time Vim mode is
    active, in both sub-modes, at the exact row and column `hjkl`/typing
    put it — not at the bottom prompt, which is exactly what the
    `-- NORMAL --`/`-- INSERT --` status line stands in for instead.
    `:edit n-m`'s *range* form is otherwise unused by the TUI, since
    navigating to each line in turn already covers it.

  **Tentative in the plain frontend.** Now that any buffer line can be
  retyped in place directly — which is really all `:edit` ever does under
  the hood — its dedicated command (and `:skip`/`:cancel`/`:done`
  vocabulary) may not stay necessary there once this beds in;
  `:deassert`/`:insert` share this status for the same reason. `:help
  edit` flags all three the same way. The TUI has already moved past this
  entirely — Vim-mode editing is its real answer now, not a placeholder.

- `:deassert [<n>]` — with no argument, finds the earliest `a`-rule (an
  [unchecked assertion](../proof_format_overview.md) — a constraint
  trusted with no proof at all, meant as temporary scaffolding while
  building a larger derivation) and opens it for editing; with a line
  number, targets that one specifically instead — handy once a proof has
  more than one assertion and the first isn't the one you meant. Errors
  if `n` isn't an `a`-rule (or doesn't exist), rather than silently
  falling back to the first one. See below for how each frontend actually
  opens it for editing.

  What's being replaced stays visible for the whole edit: in the plain
  frontend, the prompt itself becomes `edit (deasserting <constraint>)> `
  for as long as the queue-based edit lasts (not just the intro message,
  which has long since scrolled past by the time you're a few derivation
  steps in). The TUI's Output-pane heading likewise becomes `Output
  (deasserting <constraint>)`, but only for as long as the resulting
  `Insert` session lasts — see below, where the two frontends genuinely
  diverge.

  The TUI also keeps the *hint list* current while that heading is
  showing, not just the constraint text: every live constraint in the
  Database pane that shares a variable with the assertion gets
  highlighted (the same accent color the Vim-mode cursor row uses
  elsewhere, since the Database pane never has one of its own to
  collide with), recomputed every frame — so it stays accurate as you
  derive new intermediate constraints mid-edit, unlike a list frozen at
  whatever the database looked like when the edit began. The Database
  heading picks up the same `(deasserting <constraint>)` suffix as
  Output's, alongside its usual count, so a highlighted subset never
  reads as "this is the whole database" — `Database (12) (deasserting
  <constraint>)`.

  Errors ("no assertions left") if none remain (bare `:deassert` only).
  **The two frontends diverge here, same as `:edit`:**

  - **Plain** uses the queue-based flow described under `:edit` above:
    type as many real derivation steps as it takes, `:done` when
    satisfied, `:skip`/`:cancel` available throughout — replacing an
    assertion is inherently open-ended (it might take one derivation step
    or ten), so unlike a bare `:edit n`'s auto-finish, `:deassert` always
    behaves like an explicit range, waiting for `:done` no matter how
    many lines it takes.

  - **TUI** lands directly in the Vim-mode editor's `Insert` sub-mode, at
    column 0 of the assertion line — replacing it is the whole point, so
    there's no need for the extra `i` keystroke Vim mode would otherwise
    need. Esc/Enter commits that one line and returns to `Normal`,
    exactly like any other Vim-mode edit; the "(deasserting ...)"
    headings and Database highlighting last only for that one `Insert`
    session, clearing the moment it's committed. Need more than one line
    to replace the assertion with? `o`/`O` from `Normal` mode open
    further ones the usual way — they just won't carry the
    "(deasserting ...)" heading forward, since by then you're doing
    ordinary Vim-mode editing, not the initial one-line swap-in. The TUI
    also adds one shortcut on top: double-clicking a Proof-pane line
    that's currently an `a`-rule runs `:deassert` on that exact line, the
    same way double-clicking any other line enters Vim mode's `Normal`
    sub-mode on it (see above).

  It also prints a hint as soon as the edit opens: every constraint
  currently in the database that mentions any variable the assertion
  itself mentioned, e.g.

  ```
  Constraints mentioning x1, x2:
    ConstraintId 7: @foo 1 x1 +1 x3 >= 1 [core]
    ConstraintId 3: 1 x2 +1 x4 >= 1 [derived]
    ... (+12 more — :show x1 for everything mentioning just that one)
  ```

  Ranked by how many of the assertion's variables each one shares (most
  first), then by recency — usually what you were just working on, and so
  most likely relevant. Found the same lookup-based way the TUI's syntax
  highlighter finds variable references in raw rule text — every
  whitespace token that resolves to a real variable name, `~` stripped
  first — rather than actually parsing the assertion, since `a`'s
  constraint syntax has no more of a fixed grammar than any other rule's
  does. Purely a courtesy: nothing shown is required or verified, just a
  starting point, and it says nothing if the assertion mentions no
  variables at all. The assertion's own database entry can show up in its
  own list (it trivially "shares" every one of its variables with
  itself) — harmless, and easy to recognize as exactly what you're about
  to replace.

- `:delete <n>` / `:delete <n>-<m>` — removes those line(s) immediately,
  checked or not, in both frontends. No retyping, no interactive step at
  all: unlike `:edit`, there's nothing to wait on, so it's just gone the
  moment you press Enter. If a removed line was checked, everything after
  it (which can no longer be assumed to still hold) becomes unchecked
  again — but stays in the buffer; `:verify` reverifies whatever's left.
  Deliberately has no `:edit cancel`-style safety net of its own: it's
  meant to be a quick, direct action, not a staged one, so its recovery
  is the same as any other committed change (retype what's missing, or
  `:undo`). In the TUI's Vim-mode editor, it's also reachable without
  typing the command at all: `dd` in `Normal` mode deletes the cursor
  line the same way (`x` deletes just the character under the cursor).
  If a `dd` empties the whole proof, the editor leaves Vim mode on its
  own — there's nothing left to edit.

- `:insert <n>` *(tentative in the plain frontend — see `:edit` above)*
  — adds new, unchecked line(s) *before* whatever's currently line `n`,
  pushing it (and everything after) down without touching it — the one
  operation `:edit` doesn't cover, since `:edit` always starts from an
  *existing* line to replace. `:insert <one past the last line>` appends
  at the very end (same convention `Vec::insert` uses for its index) —
  equivalent to just typing normally, except unchecked; offered mainly so
  an off-by-one doesn't turn into a confusing error.

  - **Plain** uses `:deassert`'s open-ended queue flow with nothing
    pulled out to retype: the queue starts empty, so from the first
    keystroke on it behaves exactly like `:deassert` — type as many new
    lines as you want, `:done` when finished, `:cancel` to abandon and
    restore things exactly as they were.

  - **TUI** splices one new, unchecked blank line in at that position —
    `Session::insert_line`, the same primitive `o`/`O` use in Vim mode —
    and lands directly in `Insert` on it at column 0, the equivalent of
    pressing `O` there without needing an existing line to open it from.
    Esc/Enter commits it and returns to `Normal`; `o`/`O` from there add
    as many further lines as you want, same as anywhere else in the
    editor.

- `:formula [<n>]` / `:formula <n>-<m>` — switches into formula-editing
  mode (prompt becomes `opb>`) to retype one or more existing formula
  constraints' content in place, freshly parsed as OPB constraints; with
  no argument, targets the last formula constraint. Each commit
  reverifies the whole proof buffer against the edited formula
  immediately. Scoped to in-place, same-count edits only: constraint IDs
  are purely
  positional, so an edit that neither adds nor removes a constraint leaves
  every ID — formula and proof-derived alike — numerically unchanged, and
  every label still points at the right place. An `=` constraint is
  rejected, since it parses as two constraints (`>=` and `<=`) and would
  change the count; use `>=` or `<=` instead, or edit the OPB file
  directly and `:load` it if the constraint count needs to change.

  The replacement text doesn't need its own trailing `;` — it's implied
  when retyping one already-formatted constraint at a time, and is added
  automatically if missing (harmless if you type it anyway).

  A syntactically valid replacement always commits. Unlike `:edit`/
  `:delete`, which only ever invalidate a downstream *tail*, a formula
  edit can invalidate any proof line, not just ones positionally after
  something, so the entire buffer is reverified from scratch on every
  commit — one O(n) batched attempt first, falling back to a one-line-
  at-a-time pass only if that fails, to pinpoint exactly where (`:source`
  used this same two-tier trick before it stopped checking on load
  entirely). Whatever still checks out stays committed; the first line
  that no longer holds stays in the buffer, unchecked, right where it is
  — **never dropped** — with everything after it left for you to fix
  forward with `:edit`/`:delete`/`:insert` and `:verify` again.

  The fullest buffer still worth reverifying is remembered across a *run*
  of edits, not just whatever the buffer happens to be right now — so if
  one edit breaks something further down and a later edit fixes it, that
  later edit is tried against the original, longer proof again, rather
  than only ever reverifying the already-shortened one. This memory is
  reset the same way `:formula cancel`'s is (see below): by anything else
  you do in between.

  `:formula cancel` undoes the single most recent commit, restoring both
  the replaced constraint and the buffer it had reverified against,
  together, in one step. It only ever applies to the one commit that's
  still the most recent thing that happened — typing anything else
  first, including an ordinary proof line, makes it unavailable. It
  works both mid-mode (to retry a bad retype without leaving) and
  afterward at the ordinary prompt.

  Bare `:undo`, typed after the whole session ends (`:done`), is
  different: it undoes the *entire* session (however many constraints
  were retyped) as one step, not just the last commit — see `:undo`
  above.

  Not to be confused with the `f <n>;` proof-format rule, which merely
  checks the formula's constraint count and is unrelated to editing
  content.

  - **Plain**: `:formula n` queues that one constraint; `:formula n-m`
    queues the whole range. Submitting a replacement (or a blank line, to
    keep the current one unchanged) advances to the next constraint in
    the queue, shown as a reference line the same way `:edit`'s plain
    queue shows one — once the range is exhausted, or immediately after a
    single-constraint edit, the mode ends automatically. `:done` ends it
    early, leaving any remaining constraints in the range untouched.
    While the mode is active, only `:done` and `:formula cancel` are
    recognized as commands — everything else you type is the replacement
    constraint text.

  - **TUI**: `:formula` (cursor starts on the last constraint) or
    `:formula n` / `:formula n-m` (cursor starts on `n` — a range only
    picks where browsing begins, nothing is queued) enters an interactive
    browse mode over the Formula pane, as does double-clicking a
    constraint there. ↑/↓ move the cursor through the formula, the prompt
    continuously shows the current constraint's text, and typing a
    replacement "locks" the line: arrows stop moving the cursor (so a
    keystroke never silently discards what you're typing) until you
    either apply or back out. Enter commits the replacement and
    reverifies immediately, then drops
    back to browsing on the same constraint so the result is visible
    right away. Esc reverts an in-progress (locked) edit, or leaves the
    mode if not locked. `:formula cancel`, typed and entered, undoes the
    most recent commit without leaving the mode. `:done` leaves the mode
    outright.

- `:debug` — switches into stepping/breakpoint mode (prompt becomes
  `debug>`): a walk back and forth over `checked_len`, the same
  checked/unchecked buffer boundary the ordinary prompt and `:verify`
  already drive, with breakpoints to stop at along the way.

  - `:step [n]` (bare, default 1) re-checks the next `n` unchecked
    lines, one at a time — the same thing typing a line at the ordinary
    prompt or `:verify` does, just paced out. Stops early on a
    rejection, same as `:verify`.
  - `:back [n]` retracts `n` lines instead — nothing is re-verified
    going backward, since there's nothing left to check, only to
    forget; retracting past an already-good line doesn't re-run
    anything when you step forward over it again either, since nothing
    about it changed. Stops early at the very start (`checked_len ==
    0`) rather than erroring.
  - `:continue` runs forward to the next breakpoint or a rejection,
    whichever comes first — standing on a breakpoint doesn't stop you
    immediately; it runs *past* the one you're already at and stops at
    the next.
  - `:until <n>` does the same as `:continue`, but to a one-off line
    instead of a standing breakpoint — handy for "just run to here
    once" without leaving a breakpoint behind.
  - `:break` (bare) lists the current breakpoints; `:break <n>` toggles
    one at line `n`; `:break clear` drops them all.
  - `:restart` retracts all the way back to the top, keeping every
    line — the non-destructive sibling of `:reset`, which drops the
    buffer entirely.
  - `:show`, `:list`, `:objective`, `:check`, `:explain`, and `:why`
    all still work without leaving the mode — the same read-only
    commands the ordinary prompt has, reused as-is (see `:why`'s own
    entry above for what it explains when a line is currently
    rejected — exactly the "why is this line bad" question stepping
    through a proof tends to raise).
  - `:done` (or Esc, in the TUI) leaves the mode.

  Every `:step`/`:back`/`:continue`/`:until`/`:restart` is its own
  undoable step, same as anything else that moves `checked_len` —
  `:undo` walks back through them one at a time like any other action.
  `:break` isn't undoable: a breakpoint is editor furniture, not proof
  state, so toggling one never touches the undo stack (and `:undo`
  never touches a breakpoint either) — same reasoning as `:theme` or
  which pane has focus not being undoable.

  Breakpoints are buffer positions, not proof content: they shift
  correctly when lines are inserted or deleted ahead of them, survive
  `:edit`/`:formula` edits to the lines around them, and are cleared
  only by `:reset` (a fresh buffer has nothing to mark). `:list` tags
  a breakpointed line with `[breakpoint]`, alongside `[unchecked]` if
  it's also unchecked.

  **Possible, if tedious, in the plain frontend** — everything above
  works identically there, just typed out each time.

  **TUI-specific:** entering Vim mode's `Normal` sub-mode on the Proof
  pane and pressing `b` toggles a breakpoint on the cursor line
  directly, without going through `:debug` at all — it shows as a `●`
  marker in the gutter, and is the exact same `Session::breakpoints`
  `:debug`'s own `:break` manages, so either view stays in sync with
  the other. `:debug` mode itself isn't a custom UI the way `:edit`/
  `:formula` are — it's the same `debug>` prompt the plain frontend
  shows, just with a couple of extra keyboard shortcuts layered on:
  Shift+Down steps forward one line (`:step`'s shortcut) and Shift+Up
  steps back one (`:back`'s), regardless of what's typed at the prompt.
  A bare Enter on an empty `debug>` line is deliberately *not* one of
  these — it's a no-op, same as an empty prompt anywhere else in the
  app. Esc leaves the mode outright, same as `:done`.

  Every command name in `:debug` also abbreviates to its shortest
  unambiguous prefix, the same as at the ordinary prompt (`:co` for
  `:continue`) — except a genuinely ambiguous one doesn't error the way
  the ordinary prompt's does: it resolves to whichever candidate is
  listed first in `:help debug` (so bare `:c`, ambiguous between
  `:check` and `:continue`, becomes `:continue`), since a fast,
  possibly-wrong-but-recoverable guess beats an error in a tight
  stepping loop.

  The gutter also takes the mouse directly, no Vim mode needed: hover
  over a Proof line's marker column and a dimmed `●` previews there —
  purely a preview, nothing is set yet, and it vanishes the moment the
  mouse moves off. Click it while previewed (or just click straight
  down on the column, no hover required) and it becomes a real,
  colored breakpoint that stays put after the mouse leaves; click an
  already-active one and it clears. Same `Session::breakpoints` as `b`
  and `:break`, so all three stay in sync.

## Known gaps / rough edges in the current PoC

- **Completion covers first tokens, file paths, `:help` topics, `:theme`
  names, and rule arguments.** Tab completes command and rule names, file
  paths in the argument of `:load`/`:source`/`:save`/`:instance`
  (filtered to each command's own file type — `.opb` for `:load`, `.pbp`
  for `:source`/`:save`, both for `:instance`), topic names after
  `:help`, theme names after `:theme`, and — once a rule keyword is fully
  typed — its arguments, against live constraint IDs/labels/variable
  names (see above; coarse per-rule, not grammar-aware). Still
  uncompleted: paths containing spaces, and rule arguments before the
  keyword itself resolves to exactly one rule (no candidates while it's
  still ambiguous or unrecognized). The `--plain` frontend remains a bare
  `stdin` read with no editing, history, or completion, on purpose.
- **Mouse support is scroll/focus/edit-entry/zoom/breakpoint only.** The
  wheel scrolls, a click focuses, double-clicking a Proof line enters Vim
  mode's `Normal` sub-mode there (or `:deassert`'s `Insert`, for an
  `a`-rule), or a Formula line opens `:formula`, a header's `[▭]`/`[⛶]`
  buttons toggle that pane's zoom, and clicking a Proof line's gutter
  marker column toggles a breakpoint there (see above); clicking a
  suggestion doesn't insert it, scrollbars can't be clicked or dragged,
  and there's no click-to-position in the prompt or drag-to-resize of
  panes.
- **No real progress indicator for a slow `:verify`.** `:source` itself
  is fast now — loading a file no longer checks anything — so the
  potentially-slow step moved to `:verify`, which has no `Working…`-style
  acknowledgment of its own: for a large unchecked backlog, the screen
  can sit still for a visible moment with no feedback before the result
  (or the first rejection) appears. Same underlying reason `:source`'s
  own indicator exists at all — a single-threaded app with no safe way to
  interleave drawing with a blocking check, since the checker's internals
  aren't `Send`.
- **The prompt only distinguishes `:edit` mode, not subproof depth — and
  only in the plain frontend.** It switches to `edit>` while a queue-based
  edit is active there, but is always `pbp>` otherwise, even mid-subproof
  — the design calls for depth-aware prompts too (`pbp>` → `red>` →
  `red #1>`), not yet built. The TUI has its own, unrelated Vim-mode
  status (`-- NORMAL --`/`-- INSERT --`) in that same row instead, which
  has the identical gap: no subproof-depth awareness either.
- **No subproof-specific UX.** Multi-line constructs (e.g. a `red ... :
  subproof` body) do get accepted line-by-line — via `:source`, `:edit`,
  or by typing them one at a time — because each intermediate line
  replays as "checked fine, just wants more input." But there's no depth
  tracking, no goal listing, and neither frontend's prompt/status reflects
  being inside a subproof.
- **No completion for `:formula`'s own arguments, and none at all in the
  TUI's Vim mode.** The `:`-vocabulary itself completes correctly during
  the plain frontend's queue-based `:edit`/`:deassert`/`:insert` and
  during `:formula` mode in both frontends — the TUI's suggestion strip
  and Tab (both frontends' Tab, in fact — the strip is TUI-only, but the
  same registry backs both) offer exactly what that mode actually
  recognizes (`:cancel`/`:skip`/`:done` plus the six read-only commands
  in the plain queue; `:formula cancel`/`:done` in formula mode) instead
  of the full command list, most of which doesn't work there. The TUI's
  Vim mode has no such carve-out to begin with: `Normal` never feeds the
  prompt (single keys are commands, not text, so there's nothing to
  complete), and `Insert` uses the ordinary full registry, same as typing
  a fresh line anywhere else — there's no restricted vocabulary for it to
  get right or wrong. What's still missing, in both cases, is completion
  *inside* `:formula`'s/the plain queue's own keywords' arguments — moot
  today, since none of them take one, but relevant if that ever changes.

## Not yet implemented

- `:why all` — the full literal-by-literal propagation trail behind a
  `rup` step (every intermediate assignment, not just which constraints
  were needed). `:why`/`:why needed` (see above) already covers the
  minimized-hints half — that one didn't need upstream changes, just a
  scratch-file trick; `all` genuinely does, since the checker only builds
  that fuller trail on the *failure* path today, with no equivalent hook
  for a successful step.
- `:goals` — inside a subproof, list remaining proof goals.
- `:trace on|off` — verbose checker tracing for subsequent lines; tracing
  is currently always scoped internally to just-accepted lines.
- Depth-aware prompts and other subproof-specific UX (see "Known gaps"
  above).
- CNF/WCNF formula loading (the underlying library supports it; the REPL
  only calls into the OPB parser so far).
