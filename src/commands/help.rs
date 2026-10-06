//! `:help [<topic>]` — the command list, or details on one command or
//! proof rule — plus the topic registry behind it. The registry is also
//! what drives the TUI's Tab completion and the live suggestion strip
//! under the prompt, so "what can I type here?" is answered in exactly
//! one place. Rule syntax follows `proof_format_overview.md` at the repo
//! root, which stays the authority on full grammar and semantics.

use crate::output::{Output, outln};

pub struct Topic {
    /// Bare name — no leading `:`, even for commands: "check", "rup".
    pub name: &'static str,
    /// One-line syntax, as typed: ":check [<conclusion>]", "rup <constraint> ;".
    pub usage: &'static str,
    /// One-line description, shown in lists and prompt suggestions.
    pub summary: &'static str,
    /// Extra lines for `:help <topic>`.
    pub details: &'static [&'static str],
}

pub const COMMANDS: &[Topic] = &[
    Topic {
        name: "check",
        usage: ":check [<conclusion>]",
        summary: "test whether the session verifies, without closing it",
        details: &[
            "With no argument, auto-tries UNSAT, SAT, and — once an objective's",
            "best value is already known — BOUNDS v v for that value, showing",
            "whichever the checker accepts, or NOT YET CONCLUDED. With an argument",
            "(UNSAT, SAT, BOUNDS <lo> <hi>, ...), tests exactly that conclusion.",
            "Always a dry run on a throwaway checker — the live session is never",
            "touched.",
        ],
    },
    Topic {
        name: "show",
        usage: ":show [<filters>]",
        summary: "list database constraints, optionally filtered",
        details: &[
            "Filters combine with AND: a variable name (x3) or glob",
            "(i[vertex0]* — any constraint mentioning a variable whose name",
            "matches), an ID (5) or ID-range (5-12), a label (@sum) or label",
            "glob (@i[vertex0]*), and `core` or `derived`. `*` stands for any",
            "run of characters and works anywhere in a glob, not just the",
            "end. No filters shows every constraint still present; deleted",
            "ones are always skipped.",
        ],
    },
    Topic {
        name: "explain",
        usage: ":explain [<n>]",
        summary: "show how a proof line was derived, or why it was rejected",
        details: &[
            "For a pol line: the full step-by-step reverse-polish table (each",
            "operator applied, and the running constraint after it). For a",
            "red line: the substitution witness plus every proofgoal and how",
            "it was auto-proven. For a rup line: the baseline ConstraintId",
            "line plus the minimized set of already-derived constraints (and/",
            "or the rule's own negation, ~) the checker actually needed to",
            "reach the conflict — whatever hints were typed, or none. For",
            "every other rule: just the baseline ConstraintId line.",
            "If n is the line :verify rejected: the checker's reason, plus,",
            "for a rup line, a diagnosis — typed hint IDs missing from the",
            "database, and whether the constraint checks with its hint list",
            "stripped (and if so, which hints it then needed).",
            "With no n: the rejected line if there is one, otherwise the last",
            "checked line. Never touches the session — replays through a",
            "fresh, throwaway checker, same as :check.",
        ],
    },
    Topic {
        name: "list",
        usage: ":list",
        summary: "print the proof so far, numbered like checker traces",
        details: &[
            "Shows the synthesized preamble (tagged [preamble]) then every",
            "accepted line, numbered so `line N` in traces and errors matches —",
            "your first rule is line 3. Unchecked lines are tagged [unchecked]",
            "([unchecked, failed: ...] for the one known_bad names, if any);",
            "any line with a breakpoint set (see :debug) gets [breakpoint] too.",
        ],
    },
    Topic {
        name: "objective",
        usage: ":objective",
        summary: "show the objective and best known bounds",
        details: &[
            "Prints the current objective (reflecting any obju updates), the best",
            "value logged by any sol-family rule, and the best value with",
            "checked-deletion guarantees (what conclusion BOUNDS relies on).",
        ],
    },
    Topic {
        name: "undo",
        usage: ":undo [<n>]",
        summary: "undo the last n actions (default 1) — restores state from n actions ago",
        details: &[
            "Not line-based — action-based. An action is whatever counted as one",
            "coherent thing you did: a proof line, a :delete, a whole :source'd",
            "file, an entire :edit/:formula session, one TUI browse-mode commit,",
            "and so on. :undo 3 jumps back 3 actions, not 3 lines. Remembers up",
            "to the last 200 actions; further back than that reports nothing",
            "left to undo. :load/:instance/:reset are hard boundaries — nothing",
            "before one of those is reachable afterward.",
            "For an active :formula session, :formula cancel undoes just the",
            "most recent commit and stays in the mode; :undo, from outside the",
            "mode, undoes the whole session (however many constraints were",
            "retyped) as one step — different granularities, both useful.",
            "For precise, line-numbered removal instead of counting back",
            "actions, use :delete <n>-<m>.",
        ],
    },
    Topic {
        name: "delete",
        usage: ":delete <n> | :delete <n>-<m>",
        summary: "remove proof line(s) immediately, checked or not",
        details: &[
            "Immediate, no retyping and no interactive step, unlike :edit — the",
            "lines are just gone. If a removed line was checked, everything after",
            "it becomes unchecked again (it can no longer be assumed to still",
            "hold), but nothing else is touched or lost — :verify to reverify",
            "what's left. No special undo — same recovery as any other",
            "committed change.",
        ],
    },
    Topic {
        name: "formula",
        usage: ":formula [<n>] | :formula <n>-<m> | :formula cancel",
        summary: "switch into formula-editing mode — retype constraint(s) in place",
        details: &[
            "Enters formula-editing mode (prompt becomes opb>). :formula n shows",
            "constraint n's current text; retype it and press Enter to commit —",
            "the proof reverifies immediately and the mode exits automatically.",
            ":formula n-m instead walks through n, n+1, ... m in turn, each",
            "retype committing and reverifying as you go, until :done or the",
            "end of the range; a blank line keeps the current one unchanged and",
            "moves on. Bare :formula (no argument) starts on the last formula",
            "constraint.",
            "Every commit keeps the same constraint count and order, so every",
            "constraint ID (formula and proof-derived alike) stays numerically",
            "unchanged — an = replacement is rejected, since it parses as two",
            "constraints and would change the count. Reverifying checks the",
            "whole buffer, not just a downstream tail, since a formula edit can",
            "affect any line, not only ones after it; whatever still checks out",
            "stays committed, the first line that no longer holds stays in the",
            "buffer, unchecked, right where it is (never dropped) — fix it",
            "forward with :edit/:delete/:insert and :verify again, or by",
            "retrying the edit: the fullest buffer still worth reverifying is",
            "remembered across a run of edits, not just whatever's live right",
            "now, so a later, corrected edit can still recover proof lines an",
            "earlier, broken one left unchecked.",
            ":formula cancel undoes the single most recent commit, restoring",
            "both the formula and the buffer together — usable mid-mode",
            "(to retry a bad retype without leaving) or afterward at the",
            "ordinary prompt, but only immediately after that commit; anything",
            "else typed in between invalidates it. Bare :undo, typed after the",
            "whole session ends (:done), is different — it undoes the entire",
            "session (however many constraints were retyped) as one step, not",
            "just the last commit; see :help undo. Not the same as the f <n>;",
            "proof-format rule (which just checks the formula's constraint",
            "count).",
        ],
    },
    Topic {
        name: "debug",
        usage: ":debug",
        summary: "step/breakpoint mode: walk checked_len forward and back a line",
        details: &[
            "Enters debug mode (prompt becomes debug>). :step [n] re-checks the",
            "next n unchecked lines, same as :verify but one at a time; :back [n]",
            "retracts n lines instead — nothing is re-verified going backward,",
            "only forgotten. :continue runs forward to the next breakpoint or a",
            "rejection; :until <n> does the same but to a one-off line instead of",
            "a standing breakpoint. :break lists breakpoints, :break <n> toggles",
            "one at line n, :break clear drops them all — in the TUI, pressing b",
            "on the cursor line in Vim mode does the same toggle. :restart",
            "retracts all the way back to the start, keeping every line (the",
            "non-destructive sibling of :reset).",
            ":show, :list, :objective, :check, and :explain all still work",
            "without leaving the mode. :done (or Esc, in the TUI) leaves. Every",
            "command name here abbreviates to its shortest unambiguous prefix",
            "(:co for :continue), same as at the ordinary prompt; a genuinely",
            "ambiguous one (:c, matching both :check and :continue) resolves to",
            "whichever comes first above rather than erroring. In the TUI",
            "specifically, Shift+Down/Shift+Up step forward/backward without",
            "typing the command out — a bare Enter on an empty debug> line is a",
            "no-op, same as everywhere else in the app.",
            "Every :step/:back/:continue/:until/:restart is its own undo-able",
            "step, same as anything else that moves checked_len; :break isn't —",
            "a breakpoint is a marker, not proof state, so :undo never touches",
            "it.",
        ],
    },
    Topic {
        name: "insert",
        usage: ":insert <n>",
        summary: "[tentative] add new, unchecked proof line(s) before line n",
        details: &[
            "Tentative, for the same reason :edit is — see its own :help entry.",
            "Opens an open-ended edit — like :deassert, but nothing is pulled out",
            "to retype: type as many new lines as you want, then :done to leave.",
            "Nothing is checked until :verify. :insert <one past the last line>",
            "adds at the very end (same as just typing normally, except unchecked).",
            ":cancel abandons and restores things exactly as they were.",
        ],
    },
    Topic {
        name: "reset",
        usage: ":reset",
        summary: "drop all proof lines, keep the formula",
        details: &[
            "Clears the buffer (checked or not) and rebuilds the checker from",
            "the formula alone. Unlike :load, the formula file is not re-read.",
        ],
    },
    Topic {
        name: "load",
        usage: ":load <formula.opb>",
        summary: "load a formula (resets the session)",
        details: &[
            "OPB only for now. A bad path or parse error leaves the current",
            "session untouched.",
        ],
    },
    Topic {
        name: "instance",
        usage: ":instance <file.opb|file.pbp|stem>",
        summary: "load a formula and its matching proof together",
        details: &[
            "Name either half of a formula/proof pair sharing a stem",
            "(foo.opb + foo.pbp), or the bare stem itself (just \"foo\") to",
            "have both extensions appended, and both files get loaded:",
            ":load for the formula, then :source for the proof, exactly as",
            "if run separately. Both files are confirmed to exist before",
            "either is touched, so a typo can't leave the formula loaded",
            "with no proof to follow; a bad formula or a bad/partial proof",
            "is reported the same way a bare :load/:source reports one.",
        ],
    },
    Topic {
        name: "source",
        usage: ":source <file.pbp>",
        summary: "load a proof from a file into the buffer, unchecked",
        details: &[
            "Clears the buffer first (formula kept, same as :reset), then the",
            "file's derivation lines land in it as-is — nothing is checked on",
            "load. :verify checks them on request, same as any other unchecked",
            "buffer content. The file's preamble and closing lines are framing,",
            "not applied — its conclusion is dropped rather than auto-checked,",
            "since that only means anything once the buffer actually verifies.",
        ],
    },
    Topic {
        name: "verify",
        usage: ":verify",
        summary: "check the buffer's unchecked tail, stopping at the first problem",
        details: &[
            "Checks buffer lines one at a time from wherever checking last",
            "stopped, committing each as it passes. The first rejection stops",
            "there — nothing is discarded, including everything still",
            "unchecked after it. Fix the failing line (:edit, or retype it",
            "directly) and :verify again to resume from there.",
        ],
    },
    Topic {
        name: "save",
        usage: ":save <file.pbp> [<conclusion>]",
        summary: "write the entire buffer to a .pbp file, checked or not",
        details: &[
            "Always writes, regardless of check status — an unverified",
            "conclusion, an unchecked tail, or a known-bad line are all",
            "reported as warnings rather than blocking the write. With no",
            "explicit conclusion, auto-detects the same way bare :check does.",
        ],
    },
    Topic {
        name: "edit",
        usage: ":edit [<n>] | :edit <n>-<m>",
        summary: "[tentative] retype one or more buffer lines in place",
        details: &[
            "Tentative: now that any buffer line can be retyped in place",
            "directly (see :verify), a dedicated :edit command may not stay",
            "necessary — kept for now while that beds in.",
            "With no argument, targets the last buffer line; otherwise the",
            "given line or range. Retyping never checks the new text and",
            "never touches any other line — if the line was already checked,",
            "it (and everything after it) simply becomes unchecked again,",
            "still sitting in the buffer, until :verify says otherwise.",
            "Plain frontend: queues the target line(s) to retype in turn (:skip",
            "to drop one, deleting it); :done leaves the mode, leaving",
            "anything still queued exactly as it is; :cancel restores the",
            "whole buffer exactly as it was before the edit began. :show,",
            ":list, :objective, :check, and :explain also work mid-edit",
            "(nothing else does) — they're read-only, so they can't disturb",
            "what's queued.",
            "TUI: enters an interactive browse mode instead (cursor starts on",
            "the target line) — up/down move the cursor, typing edits that",
            "line, Enter applies it immediately, Esc cancels the in-progress",
            "edit or leaves browse mode, :done also leaves. The same six",
            "read-only commands work here too, without leaving browse mode;",
            "nothing else does.",
        ],
    },
    Topic {
        name: "deassert",
        usage: ":deassert [<n>]",
        summary: "[tentative] replace an `a` (unchecked assertion) with a real derivation",
        details: &[
            "Tentative, for the same reason :edit is — see its own :help entry.",
            "With no argument, locates the earliest `a`-rule in the proof; with a",
            "line number, targets that one specifically (erroring if it isn't",
            "actually an `a`-rule). Opens it the same way as an explicit :edit",
            "<n>-<n> range: retype it with one or more real derivation steps (rup,",
            "pol, red, ...) — never auto-finishes after just one, unlike bare",
            ":edit <n> — then :done once you're satisfied. Nothing is checked",
            "until :verify. :skip/:cancel work the same as during :edit. In",
            "the TUI, double-clicking an assertion line does this too. Also",
            "prints a hint: every currently-checked constraint mentioning any",
            "variable the assertion itself mentioned, most-relevant first — a",
            "starting point for what to reference, not a requirement.",
        ],
    },
    Topic {
        name: "help",
        usage: ":help [<topic>]",
        summary: "this list, or details on a command or proof rule",
        details: &[
            "Topics are command names (with or without the leading `:`) and",
            "proof-rule keywords, e.g. `:help rup` or `:help save`.",
        ],
    },
    Topic {
        name: "theme",
        usage: ":theme [<name>]",
        summary: "TUI only: switch the color palette",
        details: &[
            "No argument reports the current theme and the available ones.",
            "dark (default): tuned for a dark terminal background. light: white/",
            "light-grey background, near-black text. hi-contrast: true black/",
            "white background/text, fully saturated highlights, and a brighter",
            "dim, favoring maximum distinction over subtlety. colorblind: dark",
            "background like `dark`, but core/derived/assertion move off the",
            "green-vs-red pairing (hardest to tell apart under red-green color",
            "blindness) onto the Okabe-Ito palette instead. App-wide, not just",
            "accents: background/foreground everywhere, not just highlighted",
            "bits. Takes effect on the next redraw; has no effect at all in the",
            "plain frontend, which has no color to switch.",
        ],
    },
    Topic {
        name: "quit",
        usage: ":quit",
        summary: "exit the REPL",
        details: &["Ctrl-D on an empty line does the same."],
    },
];

pub const RULES: &[Topic] = &[
    Topic {
        name: "rup",
        usage: "rup <constraint> [: <ID> ...] ;",
        summary: "add a constraint verified by reverse unit propagation",
        details: &[
            "Checked by negating the constraint and unit-propagating the database",
            "to contradiction. Optional hint IDs restrict (and order) which",
            "constraints propagate; `~` names the negated constraint itself.",
        ],
    },
    Topic {
        name: "pol",
        usage: "pol <reverse-polish sequence> ;",
        summary: "derive a constraint with cutting-planes operations",
        details: &[
            "Operands are constraint IDs, literals, and integers; operations",
            "include + (add), * (scalar multiply), d (divide), s (saturate),",
            "w (weaken). Example: pol 42 3 * 43 + s 2 d;",
        ],
    },
    Topic {
        name: "red",
        usage: "red <constraint> : <substitution> [: subproof ... qed;]",
        summary: "add a constraint by redundance-based strengthening",
        details: &[
            "The substitution (e.g. x1 -> 1 x2 -> ~x3) is the witness. Proof",
            "goals not closed by autoproving need an explicit subproof with",
            "`proofgoal <goalID> ... qed;` blocks.",
        ],
    },
    Topic {
        name: "pbc",
        usage: "pbc <constraint> : subproof ... qed;",
        summary: "add a constraint by explicit proof by contradiction",
        details: &[
            "The subproof derives contradiction from the database plus the",
            "constraint's negation.",
        ],
    },
    Topic {
        name: "e",
        usage: "e <constraint> : [<ID>] ;",
        summary: "check a constraint equals the one with the given ID",
        details: &[
            "Sanity check only — nothing is added. Negative IDs count back from",
            "the newest constraint (-1 = most recent).",
        ],
    },
    Topic {
        name: "i",
        usage: "i <constraint> : [<ID>] ;",
        summary: "check a constraint is syntactically implied",
        details: &["Sanity check only — nothing is added."],
    },
    Topic {
        name: "ia",
        usage: "ia <constraint> : [<ID>] ;",
        summary: "add a constraint that is syntactically implied",
        details: &["Like i, but the constraint is added to the database."],
    },
    Topic {
        name: "del",
        usage: "del id <ID> ... ; | del spec <constraint> ; | del range <lo> <hi> ;",
        summary: "delete constraints (by ID, matching spec, or ID range)",
        details: &[
            "Deleting from the core set may trigger checked deletion; see the",
            "format overview for the guarantees involved.",
        ],
    },
    Topic {
        name: "delc",
        usage: "delc <ID> ... ;",
        summary: "delete core constraints by ID",
        details: &[],
    },
    Topic {
        name: "deld",
        usage: "deld <ID> ... ;",
        summary: "delete derived constraints by ID",
        details: &[],
    },
    Topic {
        name: "core",
        usage: "core id <ID> ... ; | core range <lo> <hi> ;",
        summary: "move constraints to the core set",
        details: &[],
    },
    Topic {
        name: "obju",
        usage: "obju new <objective> ; | obju diff <difference> ;",
        summary: "update the objective (new value, or by difference)",
        details: &[
            "Equality of old and new objectives must be provable — by",
            "autoproving from the core set, or with an explicit subproof",
            "(proofgoals #1 and #2).",
        ],
    },
    Topic {
        name: "sol",
        usage: "sol <literal> ... ;",
        summary: "log a solution",
        details: &[],
    },
    Topic {
        name: "soli",
        usage: "soli <literal> ... [: <objective value>] ;",
        summary: "log a solution and add an objective-improving constraint",
        details: &[],
    },
    Topic {
        name: "solx",
        usage: "solx <literal> ... ;",
        summary: "log a solution and add a solution-excluding constraint",
        details: &[],
    },
    Topic {
        name: "f",
        usage: "f <n> ;",
        summary: "check the formula has exactly n constraints",
        details: &["The REPL's synthesized preamble already performs this check."],
    },
    Topic {
        name: "a",
        usage: "a <constraint> ; | a <constraint> : <IDs> : <name> : <text>;",
        summary: "add a constraint with NO verification at all — scaffolding only",
        details: &[
            "An unchecked assertion: the constraint is trusted outright, no proof",
            "required. A proof containing one isn't actually valid — it's meant as",
            "a temporary stand-in while you build the rest of a derivation, to",
            "replace later with a real one. `:deassert` finds and replaces the",
            "earliest remaining `a`-rule for exactly that purpose.",
        ],
    },
    Topic {
        name: "proofgoal",
        usage: "proofgoal <goalID> ... qed;",
        summary: "inside a subproof: derive one named proof obligation",
        details: &[],
    },
    Topic {
        name: "qed",
        usage: "qed;",
        summary: "close a subproof or proofgoal block",
        details: &[],
    },
    Topic {
        name: "output",
        usage: "output NONE ;",
        summary: "start the closing section (the REPL only supports NONE)",
        details: &[
            "Part of the closing sequence — typing it for real starts closing",
            "the session; prefer :check / :save, which synthesize it.",
        ],
    },
    Topic {
        name: "conclusion",
        usage: "conclusion NONE|SAT|UNSAT|BOUNDS <lo> <hi>|... ;",
        summary: "state (and have checked) the proof's conclusion",
        details: &[
            "SAT takes an optional witness, UNSAT an optional contradiction ID,",
            "BOUNDS its two bounds. NONE claims nothing and always verifies.",
            "Typing it for real closes the session; prefer :check / :save.",
        ],
    },
    Topic {
        name: "end",
        usage: "end pseudo-Boolean proof ;",
        summary: "the final line of a complete proof",
        details: &[
            "After this the proof is closed and nothing more can be typed;",
            "prefer :check / :save, which synthesize it.",
        ],
    },
];

/// Commands whose name starts with `prefix` (empty prefix = all).
pub fn command_matches(prefix: &str) -> Vec<&'static Topic> {
    COMMANDS
        .iter()
        .filter(|t| t.name.starts_with(prefix))
        .collect()
}

/// Tab-completion priority for the five commands that do anything useful
/// before a session exists — mirrors the literal `cmd == "..."` arms
/// `dispatch_inner` handles ahead of its "no formula loaded" check.
/// `:instance` ranks first (loads a formula and proof together — the
/// single most useful next command with nothing loaded yet), `:load`
/// second (formula alone), then the rest in the order they'd sort
/// naturally anyway. Anything else gets `usize::MAX`, sorting after all
/// five — a stable sort keeps that tail in `COMMANDS`' own order. Used by
/// `tui::complete::first_token` to order the command list while there's no
/// session yet; once one exists, `first_token` falls back to bumping just
/// `:instance` to the front on its own (see that function's docs) rather
/// than consulting this list further.
pub(crate) fn no_session_tab_rank(name: &str) -> usize {
    ["instance", "load", "help", "theme", "quit"]
        .iter()
        .position(|&n| n == name)
        .unwrap_or(usize::MAX)
}

/// Rules whose name starts with `prefix` (empty prefix = all).
pub fn rule_matches(prefix: &str) -> Vec<&'static Topic> {
    RULES
        .iter()
        .filter(|t| t.name.starts_with(prefix))
        .collect()
}

/// Exact-name lookup across commands then rules.
fn find(name: &str) -> Option<&'static Topic> {
    COMMANDS.iter().chain(RULES.iter()).find(|t| t.name == name)
}

pub fn run(args: &str, out: &mut dyn Output) {
    let topic = args.trim().trim_start_matches(':');

    if topic.is_empty() {
        let width = COMMANDS.iter().map(|t| t.usage.len()).max().unwrap_or(0);
        outln!(out, "REPL commands:");
        for t in COMMANDS {
            outln!(out, "  {:width$}  {}", t.usage, t.summary);
        }
        outln!(out);
        outln!(
            out,
            "Proof rules are typed bare, in v3 .pbp syntax — `:help <rule>` for:"
        );
        let names: Vec<&str> = RULES.iter().map(|t| t.name).collect();
        outln!(out, "  {}", names.join(", "));
        outln!(
            out,
            "Full grammar and semantics: proof_format_overview.md at the repo root."
        );
        return;
    }

    match find(topic) {
        Some(t) => {
            outln!(out, "{}", t.usage);
            outln!(out, "  {}", t.summary);
            for line in t.details {
                outln!(out, "  {line}");
            }
        }
        None => {
            outln!(
                out,
                "No help for '{topic}' — try :help with no argument for the list."
            );
        }
    }
}
