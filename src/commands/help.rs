//! `:help [<topic>]` and the command/rule topic registry. The registry also
//! drives TUI Tab completion and the suggestion strip. Rule syntax must follow
//! `proof_format_overview.md`, the authoritative grammar.

use crate::checker::parse::rule_keyword;
use crate::output::{Output, outln};

pub struct Topic {
    /// Bare name — no leading `:`, even for commands: "check", "rup".
    pub name: &'static str,
    /// One-line syntax, as typed: `:check [<conclusion>]`, `rup <constraint> ;`.
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
        summary: "test whether the checked lines prove a conclusion",
        details: &[
            "With a conclusion (UNSAT, SAT, BOUNDS <lo> <hi>, ...), tests that",
            "one and prints the checker's verdict or error.",
            "With no argument, tries UNSAT, then SAT, then BOUNDS v v if the",
            "objective has a best value with checked-deletion guarantees (see",
            ":objective). Prints the first that verifies, or NOT YET CONCLUDED.",
            "Only checked lines are used. Nothing is added to the proof.",
        ],
    },
    Topic {
        name: "show",
        usage: ":show [<filters>]",
        summary: "list database constraints, optionally filtered",
        details: &[
            "Lists the database after the checked lines, one constraint per",
            "line: ConstraintId <id>: <constraint> [core] or [derived].",
            "Deleted constraints are not shown. Filters, all of which must match:",
            "  x3             mentions variable x3",
            "  i[vertex0]*    mentions a variable matching the glob; * matches",
            "                 any run of characters, anywhere in the pattern",
            "  5  or  5-12    constraint ID, or ID range (inclusive)",
            "  core, derived  only core, or only derived, constraints",
            "  @name          the constraint labelled @name",
            "  @glob*         constraints with a label matching the glob",
            "An unknown variable name or label is an error. Labels come from",
            "the formula and from checked proof lines; labels set by `e` are",
            "not tracked.",
        ],
    },
    Topic {
        name: "explain",
        usage: ":explain [<n>]",
        summary: "show how a proof line was derived, or why it was rejected",
        details: &[
            "n must be a checked line, or the line :verify last rejected.",
            "With no n: the rejected line if there is one, otherwise the last",
            "checked line.",
            "For a checked line, prints the checker's trace for it:",
            "  pol          each operation and the constraint after it",
            "  red          the witness, and how each proof goal was proven",
            "  other rules  the ID of the constraint the line added",
            "A rup line also lists the constraints the checker needed to reach",
            "the conflict, whatever hints were typed; ~ means the negated",
            "constraint itself.",
            "For the rejected line, prints the checker's reason. For a rup line",
            "it also lists typed hint IDs not in the database, and says whether",
            "the line checks without hints (and, if so, which it needs).",
            "Changes nothing.",
        ],
    },
    Topic {
        name: "list",
        usage: ":list",
        summary: "print the proof so far, numbered like checker output",
        details: &[
            "Prints the two preamble lines the REPL adds (tagged [preamble]),",
            "then every proof line. Numbers match `line N` in checker output,",
            "so your first proof line is line 3. Other tags:",
            "  [unchecked]                  not checked yet",
            "  [unchecked, failed: <why>]   the line :verify rejected",
            "  [breakpoint]                 has a breakpoint (see :debug)",
        ],
    },
    Topic {
        name: "objective",
        usage: ":objective",
        summary: "show the objective and best known values",
        details: &[
            "Prints the current objective (after any obju updates), the best",
            "objective value logged so far, and the best value with",
            "checked-deletion guarantees (the one conclusion BOUNDS needs).",
            "Uses the checked lines only. If the formula has no min:/max:",
            "line, says there is no objective.",
        ],
    },
    Topic {
        name: "preserved",
        usage: ":preserved",
        summary: "show the current preserved variable set",
        details: &[
            "Prints the preserved set after the checked lines, including any",
            "preserved_add/preserved_rm steps. If a step changed it, also",
            "prints that step's line number and the set the formula declared.",
            "If the formula has no preserved: line, says so.",
            "Also works during :edit and :debug.",
        ],
    },
    Topic {
        name: "undo",
        usage: ":undo [<n>]",
        summary: "undo the last n actions (default 1)",
        details: &[
            "Restores the state from before the last n actions. An action is",
            "one thing you did, not one line: a proof line, a :delete, a",
            ":source, a :verify, a whole :edit, :insert, :deassert or",
            ":formula session in the plain frontend, one :debug step, or one",
            "TUI edit commit. :undo 3 goes back 3 actions.",
            "Keeps the last 200 actions. :load, :instance and :reset clear",
            "the history. Breakpoints are not affected.",
            ":formula cancel undoes only the latest formula commit (see",
            ":help formula). To remove specific lines, use :delete <n>-<m>.",
        ],
    },
    Topic {
        name: "delete",
        usage: ":delete <n> | :delete <n>-<m>",
        summary: "remove proof line(s) immediately, checked or not",
        details: &[
            "Removes line n, or lines n to m. Checked lines after the removed",
            "ones become unchecked; :verify checks them again.",
            ":undo restores the removed lines.",
        ],
    },
    Topic {
        name: "formula",
        usage: ":formula [<n>] | :formula <n>-<m> | :formula cancel",
        summary: "edit formula constraint(s) in place and recheck the proof",
        details: &[
            "Edits the formula line holding constraint ID n (lines n to m for",
            "n-m), or by default the last line. A line loading as two",
            "constraints (<==>, =) is edited as a whole.",
            "Plain frontend (prompt opb>): shows the constraint; type its",
            "replacement and press Enter, or a blank line to keep it. With",
            "n-m, steps through each constraint in turn; the mode ends after",
            "the last one, or at :done. :formula n ends after one constraint.",
            "In both frontends, only :done and :formula cancel work as",
            "commands in the mode.",
            "TUI: opens the Formula pane with the constraint's text at the",
            "prompt. Up/Down choose a constraint; edit the text and press",
            "Enter to commit. Esc discards an unfinished edit, or leaves if",
            "there is none; :done also leaves. Double-clicking a Formula line",
            "opens it too.",
            "Each replacement is checked as a formula first; if invalid, it is",
            "rejected and nothing changes. The trailing ; is optional. A",
            "replacement must load as the same number of constraints as the",
            "line, so every constraint ID stays the same. To change the count,",
            "edit the .opb file and :load it. Retype labels to keep them.",
            "After each commit the whole proof is rechecked from the start,",
            "stopping at the first line that now fails. That line and all",
            "later lines stay in the proof, unchecked: fix them and :verify.",
            ":formula cancel undoes the latest commit (formula and proof),",
            "in or out of the mode, but only if nothing else ran since.",
            ":undo after a plain-frontend session undoes the whole session;",
            "in the TUI, each commit is a separate :undo step.",
            "Not the same as the `f <n> ;` proof rule.",
        ],
    },
    Topic {
        name: "debug",
        usage: ":debug",
        summary: "step through the proof line by line, with breakpoints",
        details: &[
            "Opens debug mode (prompt debug>). Needs at least one proof line.",
            "Commands in the mode:",
            "  :step [n]     check the next n unchecked lines (default 1)",
            "  :back [n]     mark the last n checked lines unchecked",
            "                (default 1); nothing is rechecked",
            "  :continue     check lines up to a breakpoint, a rejection,",
            "                or the end",
            "  :until <n>    like :continue, but also stop before line n",
            "  :break        list breakpoints; :break <n> toggles one on",
            "                line n; :break clear removes them all",
            "  :restart      mark every line unchecked, keeping all lines",
            "  :done         leave (Esc in the TUI)",
            ":continue and :until stop before a breakpoint line, leaving it",
            "unchecked. :show, :list, :objective, :preserved, :check and",
            ":explain also work. Proof lines cannot be typed in this mode.",
            "Commands can be shortened to any prefix. If several match, the",
            "first in the list above wins: :c is :continue, :b is :back.",
            "TUI: Shift+Down steps forward and Shift+Up steps back. In Vim",
            "mode, b toggles a breakpoint on the cursor line; clicking the",
            "breakpoint marker column in the Proof pane does the same.",
            "Each :step, :back, :continue, :until and :restart can be undone",
            "with :undo. Breakpoint changes cannot.",
        ],
    },
    Topic {
        name: "insert",
        usage: ":insert <n>",
        summary: "add new, unchecked proof line(s) before line n",
        details: &[
            "Use n = last line + 1 to add at the end.",
            "Plain frontend (prompt edit>): each line you type is inserted",
            "before line n, in order, until :done. :cancel restores the proof",
            "as it was. :show, :list, :objective, :preserved, :check and",
            ":explain also work.",
            "TUI: adds a blank line before line n and opens Vim mode in",
            "Insert on it. Esc commits it. See :help edit for Vim keys.",
            "New lines are not checked; run :verify.",
        ],
    },
    Topic {
        name: "reset",
        usage: ":reset",
        summary: "drop all proof lines, keep the formula",
        details: &[
            "Removes every proof line, checked or not, and clears the undo",
            "history and all breakpoints. Cannot be undone. The formula stays",
            "as it is, including :formula edits; the file is not re-read",
            "(use :load for that).",
        ],
    },
    Topic {
        name: "load",
        usage: ":load <formula.opb>",
        summary: "load a formula file, replacing the session",
        details: &[
            "Reads a plain-text OPB file; compressed files are not supported.",
            "Replaces the formula and clears all proof lines, the undo",
            "history and breakpoints. The formula is checked with veripb",
            "first: if the file can't be read or veripb rejects it, the error",
            "(with line and column) is shown and nothing changes.",
        ],
    },
    Topic {
        name: "instance",
        usage: ":instance <file.opb|file.pbp|stem>",
        summary: "load a formula and its matching proof together",
        details: &[
            "Give foo.opb, foo.pbp or just foo. Loads foo.opb as :load does,",
            "then foo.pbp as :source does. Both files must exist, or nothing",
            "is loaded. Proof lines are added unchecked; run :verify.",
        ],
    },
    Topic {
        name: "source",
        usage: ":source <file.pbp>",
        summary: "load a proof from a file into the buffer, unchecked",
        details: &[
            "Replaces all proof lines with the file's; the formula is kept.",
            "Existing lines are cleared first, as with :reset (undo history",
            "and breakpoints too); :undo right after restores them.",
            "Lines are not checked on load; run :verify.",
            "Skipped: the version header, an `f <n> ;` line matching the",
            "formula, and the output, conclusion and end lines. The file's",
            "conclusion is not checked; use :check after :verify. An f line",
            "with a different count is kept, and fails at :verify.",
        ],
    },
    Topic {
        name: "verify",
        usage: ":verify",
        summary: "check the unchecked lines, stopping at the first failure",
        details: &[
            "Checks unchecked lines in order. Each line that passes becomes",
            "checked. At the first rejection it stops and prints the reason;",
            "that line and all later lines stay in the proof, unchecked. Fix",
            "the line (:edit <n>) and :verify again. :explain shows more.",
            "While unchecked lines exist, proof lines you type are added at",
            "the end, unchecked. Otherwise each typed line is checked at once",
            "and discarded if rejected.",
        ],
    },
    Topic {
        name: "save",
        usage: ":save <file.pbp> [<conclusion>]",
        summary: "write all proof lines to a .pbp file, checked or not",
        details: &[
            "Writes the preamble, every proof line, and a closing section",
            "with the conclusion. Without a conclusion, picks one the way",
            ":check does, from the checked lines; if none verifies, writes",
            "`conclusion NONE`.",
            "Always writes the file. Unchecked lines, a rejected line, or a",
            "conclusion that does not verify only produce warnings.",
        ],
    },
    Topic {
        name: "edit",
        usage: ":edit [<n>] | :edit <n>-<m>",
        summary: "retype one or more proof lines in place",
        details: &[
            "Targets line n, lines n to m, or by default the last line. New",
            "text is not checked. If the edited line was checked, it and all",
            "later lines become unchecked until :verify. Other lines are not",
            "changed.",
            "Plain frontend (prompt edit>): shows each target line in turn.",
            "Type its replacement, or a blank line to keep it; :skip deletes",
            "it. :edit n ends after that line. With a range, lines typed after",
            "the last target are inserted after it, until :done. :done leaves",
            "any remaining targets unchanged. :cancel restores the proof as",
            "it was. :show, :list, :objective, :preserved, :check and",
            ":explain also work.",
            "TUI: opens Vim mode with the cursor on the target line (a range",
            "uses its first line). Normal mode: hjkl/arrows move, i/a/o/O",
            "insert, x deletes a character, dd a line, b toggles a",
            "breakpoint, : goes to the prompt, Esc leaves. Insert mode: Esc",
            "commits the line and returns to Normal, Enter splits the line at",
            "the cursor. Each change is saved, unchecked, as you make it.",
            "Double-clicking a proof line also opens Vim mode.",
        ],
    },
    Topic {
        name: "deassert",
        usage: ":deassert [<n>]",
        summary: "replace an `a` (unchecked assertion) with a real derivation",
        details: &[
            "Targets the earliest `a` line, or line n (which must be one).",
            "Also prints up to 8 database constraints that share variables",
            "with the assertion, most shared variables first.",
            "Plain frontend: works like :edit n-n. Retype the line as one or",
            "more derivation steps (rup, pol, red, ...), then :done. :skip and",
            ":cancel work as in :edit. The prompt shows the assertion.",
            "TUI: opens Vim mode in Insert on the line. Enter starts another",
            "line; Esc commits. Double-clicking an `a` line does the same.",
            "New lines are not checked; run :verify.",
        ],
    },
    Topic {
        name: "help",
        usage: ":help [<topic>]",
        summary: "this list, or details on a command or proof rule",
        details: &[
            "Topics are command names (with or without the leading `:`) and",
            "proof-rule keywords, e.g. `:help rup` or `:help save`.",
            "Commands can be shortened to any unique prefix (:v for :verify).",
        ],
    },
    Topic {
        name: "theme",
        usage: ":theme [<name>]",
        summary: "TUI only: switch the color palette",
        details: &[
            "With no name, shows the current theme and the available ones.",
            "  dark         default; for dark terminal backgrounds",
            "  light        light background, near-black text",
            "  hi-contrast  black and white, fully saturated highlights",
            "  colorblind   like dark, but core/derived/assertion colors use",
            "               the Okabe-Ito palette instead of green vs red",
            "Recolors the whole TUI immediately. No effect with --plain.",
        ],
    },
    Topic {
        name: "quit",
        usage: ":quit",
        summary: "exit the REPL",
        details: &["Ctrl-D on an empty line also exits."],
    },
];

pub const RULES: &[Topic] = &[
    Topic {
        name: "rup",
        usage: "rup <constraint> [: <ID> ...] ;",
        summary: "add a constraint verified by reverse unit propagation",
        details: &[
            "Checked by adding the constraint's negation and unit-propagating",
            "to a conflict. With hint IDs, only those constraints propagate,",
            "in the order given; `~` names the negated constraint itself.",
            ":explain <n> shows which constraints a rup line needed.",
        ],
    },
    Topic {
        name: "pol",
        usage: "pol <reverse-polish sequence> ;",
        summary: "derive a constraint with cutting-planes operations",
        details: &[
            "Operands: constraint IDs, literals (x or ~x, meaning x >= 0 or",
            "~x >= 0) and integers. Operations, written after their operands:",
            "  +  add                  *  multiply by an integer >= 0",
            "  d  divide (literal normal form)",
            "  c  divide (variable normal form)",
            "  s  saturate             w  weaken: remove a variable",
            "  -  subtract a constant from the right-hand side",
            "  m  MIR cut (variable normal form)",
            "  n  MIR cut (literal normal form)",
            "The sequence must leave exactly one constraint.",
            "Example: pol 42 3 * 43 + s 2 d ;",
        ],
    },
    Topic {
        name: "red",
        usage: "red <constraint> : <substitution> [: subproof ... qed;]",
        summary: "add a constraint by redundance-based strengthening",
        details: &[
            "The substitution is the witness, e.g. x1 -> 0 x2 -> ~x3 (`->` is",
            "optional). Proof goals that are not autoproven need a subproof",
            "with `proofgoal <goalID> ... qed;` blocks.",
            ":explain <n> shows each goal and how it was proven.",
        ],
    },
    Topic {
        name: "pbc",
        usage: "pbc <constraint> ; | pbc <constraint> : subproof ... qed;",
        summary: "add a constraint by explicit proof by contradiction",
        details: &[
            "The subproof derives a contradiction from the database plus the",
            "constraint's negation. Without a subproof, veripb tries to",
            "autoprove it.",
        ],
    },
    Topic {
        name: "e",
        usage: "e <constraint> : [<ID>] ;",
        summary: "check a constraint equals the one with the given ID",
        details: &[
            "Same terms and degree; term order does not matter. Without an",
            "ID, checks that the constraint is somewhere in the database.",
            "Adds nothing. Negative IDs count back from the newest constraint",
            "(-1 = most recent).",
        ],
    },
    Topic {
        name: "i",
        usage: "i <constraint> : [<ID>] ;",
        summary: "check a constraint is syntactically implied",
        details: &[
            "Checks that the constraint with that ID implies the given one",
            "using literal axioms and one saturation. Without an ID, any",
            "database constraint may imply it. Adds nothing.",
        ],
    },
    Topic {
        name: "ia",
        usage: "ia <constraint> : [<ID>] ;",
        summary: "add a constraint that is syntactically implied",
        details: &["Same check as i, then adds the constraint to the database."],
    },
    Topic {
        name: "del",
        usage: "del id <ID> ... ; | del spec <constraint> ; | del range <lo> <hi> ;",
        summary: "delete constraints (by ID, matching spec, or ID range)",
        details: &[
            "id: the listed IDs. spec: constraints equal to the given one.",
            "range: IDs from lo up to, but not including, hi.",
            "Deleting core constraints runs a checked deletion: each must be",
            "re-derivable from the remaining core by redundance. Add",
            "`: <substitution>` (and a subproof) if needed. If a check fails,",
            "checked-deletion guarantees are lost for the rest of the proof.",
        ],
    },
    Topic {
        name: "delc",
        usage: "delc <ID> ... ;",
        summary: "delete core constraints by ID",
        details: &["Same as del id, but fails if any ID is a derived constraint."],
    },
    Topic {
        name: "deld",
        usage: "deld <ID> ... ;",
        summary: "delete derived constraints by ID",
        details: &["Same as del id, but fails if any ID is a core constraint."],
    },
    Topic {
        name: "core",
        usage: "core id <ID> ... ; | core range <lo> <hi> ;",
        summary: "move constraints to the core set",
        details: &["range: IDs from lo up to, but not including, hi."],
    },
    Topic {
        name: "obju",
        usage: "obju new <objective> ; | obju diff <difference> ;",
        summary: "update the objective (new value, or by difference)",
        details: &[
            "new: replaces the objective. diff: adds f_new - f_old to it.",
            "Old and new objectives must be provably equal from the core set:",
            "autoproven, or by `: subproof ... qed;` where proofgoal #1 proves",
            "f_new >= f_old and #2 proves f_old >= f_new.",
        ],
    },
    Topic {
        name: "sol",
        usage: "sol <literal> ... [: <objective value>] ;",
        summary: "log a solution",
        details: &[
            "Checks that unit propagation from the given literals (~x for",
            "negated) leaves an assignment satisfying every core constraint.",
            "With `: <objective value>`, also checks the assignment achieves",
            "that value. Adds no constraint.",
            "*x (a shrunk variable) is only allowed in solx.",
        ],
    },
    Topic {
        name: "soli",
        usage: "soli <literal> ... [: <objective value>] ;",
        summary: "log a solution and add an objective-improving constraint",
        details: &[
            "Needs an objective (min:/max: in the formula). Same check as sol,",
            "plus every objective variable must be assigned after propagation.",
            "Adds f(x) <= f(solution) - 1 to the core set.",
            "*x (a shrunk variable) is only allowed in solx.",
        ],
    },
    Topic {
        name: "obji",
        usage: "obji <objective value> ;",
        summary: "add an objective-improving constraint without a solution",
        details: &["Adds f(x) <= <objective value> to the core set; no solution is checked."],
    },
    Topic {
        name: "solx",
        usage: "solx <literal|*variable> ... ;",
        summary: "log a solution and add a solution-excluding constraint",
        details: &[
            "Needs a preserved: set in the formula and no objective; not",
            "allowed after unchecked deletion. Same check as sol, plus every",
            "preserved variable must be assigned after propagation. Adds the",
            "clause excluding this assignment of the preserved variables.",
            "*x marks x as shrunk: the solution holds for any value of x. A",
            "shrunk variable must be preserved, must stay unassigned after",
            "propagation, and is left out of the excluding clause, so one",
            "solx excludes the whole cube. Example: solx *x1 x2 *x3 ;",
        ],
    },
    Topic {
        name: "preserved_add",
        usage: "preserved_add <variable> : <constraint> ;",
        summary: "add a variable to the preserved set",
        details: &[
            "Needs a preserved: set in the formula. The constraint C may only",
            "use preserved variables, and x <=> C must be proven: by",
            "autoproving, or in a subproof where proofgoal #1 is x => C and",
            "#2 is x <= C. Example: preserved_add x5 : 1 x1 >= 1 ;",
            ":preserved shows the current set.",
        ],
    },
    Topic {
        name: "preserved_rm",
        usage: "preserved_rm <variable> : <constraint> ;",
        summary: "remove a variable from the preserved set",
        details: &[
            "Needs a preserved: set in the formula. The constraint C may only",
            "use preserved variables other than x itself, and x <=> C must be",
            "proven: by autoproving, or in a subproof where proofgoal #1 is",
            "x => C and #2 is x <= C. Example: preserved_rm x5 : 1 x1 >= 1 ;",
            ":preserved shows the current set.",
        ],
    },
    Topic {
        name: "epreserved",
        usage: "epreserved <variable> ... ;",
        summary: "check that the preserved set is exactly these variables",
        details: &[
            "Fails unless the current preserved set equals the listed",
            "variables (order doesn't matter). Adds no constraint.",
            ":preserved shows the current set.",
        ],
    },
    Topic {
        name: "f",
        usage: "f <n> ;",
        summary: "check the formula has exactly n constraints",
        details: &[
            "An = constraint counts as two. The REPL's preamble already",
            "contains this line, so you do not need to type it.",
        ],
    },
    Topic {
        name: "a",
        usage: "a <constraint> ; | a <constraint> : <IDs> : <name> : <text>;",
        summary: "add a constraint with NO verification at all",
        details: &[
            "The constraint is added without any check. A proof containing an",
            "`a` line is not valid; use it as a placeholder while you build",
            "the rest of a derivation. The second form adds annotations",
            "(antecedent IDs, a name, free text) that the checker ignores.",
            ":deassert replaces an `a` line with a real derivation.",
        ],
    },
    Topic {
        name: "proofgoal",
        usage: "proofgoal <goalID> ... qed;",
        summary: "inside a subproof: derive one named proof obligation",
        details: &[
            "Each proofgoal block must derive a contradiction. <goalID> is a",
            "database constraint ID for goals coming from the database, or",
            "#1, #2, ... for the others (e.g. #1 is the constraint being",
            "derived, for red).",
        ],
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
        summary: "start the closing section of a proof",
        details: &[
            "Not needed at the REPL: :check and :save add `output NONE ;`,",
            "the conclusion and the end line themselves. If you type it, it",
            "is added to the proof like any other line, and :check and :save",
            "fail until you remove it (:delete or :undo). :source skips it.",
        ],
    },
    Topic {
        name: "conclusion",
        usage: "conclusion NONE|SAT|UNSAT|BOUNDS <lo> <hi>|... ;",
        summary: "state (and have checked) the proof's conclusion",
        details: &[
            "UNSAT [: <ID>]: a contradiction was derived (optionally its ID).",
            "SAT [: <literals>]: a solution, given here or logged earlier.",
            "BOUNDS <lo> <hi>: the optimum lies between lo and hi.",
            "ENUMERATION_COMPLETE <n> : <ID> / ENUMERATION_PARTIAL <n>:",
            "count of solutions logged with solx.",
            "NONE claims nothing and always verifies.",
            "Test one with :check, write one with :save. Typing it directly",
            "has the same problem as typing output; see :help output.",
        ],
    },
    Topic {
        name: "end",
        usage: "end pseudo-Boolean proof ;",
        summary: "the final line of a complete proof",
        details: &[
            "Nothing can follow it: later proof lines are rejected.",
            ":check and :save add it themselves; see :help output.",
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

/// Tab-completion rank used by `tui::complete::first_token` when no session
/// exists: the commands usable without one, `:instance` first; `usize::MAX`
/// for all others. Keep in sync with the session-free arms in
/// `dispatch_inner`.
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

/// A hint explaining a likely cause of the checker rejecting proof line
/// `line`, for mistakes whose checker error doesn't name the rule, or
/// `None`. Printed under the rejection by every command that reports one.
pub fn rejection_hint(line: &str) -> Option<&'static str> {
    match rule_keyword(line)? {
        "sol" | "soli" if line.split_whitespace().any(|tok| tok.starts_with('*')) => {
            Some("Hint: *<variable> (a shrunk variable) is only allowed in solx — see :help solx.")
        }
        _ => None,
    }
}

/// Prints [`rejection_hint`] for `line`, if there is one.
pub(crate) fn print_rejection_hint(out: &mut dyn Output, line: &str) {
    if let Some(hint) = rejection_hint(line) {
        outln!(out, "{hint}");
    }
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
            "Full grammar and semantics: proof_format_overview.md in the veripb repository."
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
