//! `:edit <n>` / `:edit <n>-<m>` — retype one or more existing buffer
//! lines in place, live. Also `:deassert` — find the first unchecked
//! `a`-rule assertion and open it the same way, for replacing scaffolding
//! with a real derivation — and `:delete`/`:insert`, the two operations
//! `:edit` doesn't itself cover: removing line(s) with nothing put back,
//! and adding new ones with nothing pulled out.
//!
//! **Nothing here ever truncates or checks anything.** Every mutation —
//! retyping a line, skipping one, inserting a new one — is an immediate,
//! always-successful edit to `session.buffer` (see `session.rs`'s module
//! docs for the full checked/unchecked model): retyping a line inside the
//! checked prefix retracts `checked_len` to that point, but the text of
//! every other line, checked or not, stays exactly where it is. Nothing
//! is ever reverified as a side effect of an edit — that's `:verify`'s
//! job, on request. This is a substantial simplification from the old
//! truncate-and-replay design: there's no way for an edit to "half
//! succeed," so there's nothing left to report partial success or
//! failure about.
//!
//! These three commands are kept for now mostly as-is (their surface
//! syntax, `:skip`/`:cancel`/`:done`) while the live, in-place-editable
//! buffer beds in — flagged as tentative in `:help`. The TUI has already
//! moved past this queue-based flow entirely: `:edit`/`:deassert`/
//! `:insert` there are all intercepted before dispatch and driven by its
//! own Vim-style modal editor instead (`tui/mod.rs`'s `VimState`), calling
//! straight into [`Session::set_line`]/`insert_line`/`delete_line` with no
//! queue at all. This module — [`EditState`], [`start`], [`handle`] — is
//! now the plain frontend's alone.
//!
//! Two shapes exist, sharing the same underlying `Session` primitives
//! (`buffer_index`, `set_line`, `insert_line`, `delete_line`):
//!
//! - **Queue-based, open-ended** ([`EditState`], [`start`], [`handle`]):
//!   `:edit`, `:deassert`, and `:insert` all produce an [`EditState`] and
//!   are driven the same way from there: retype (or, for `:deassert`/
//!   `:insert`, freely type any number of new lines) until `:done` leaves
//!   the mode, `:skip`/`:cancel` available throughout. `:edit`/
//!   `:deassert` start with one or more existing lines queued for
//!   retyping; `:insert` starts with an empty queue and nothing but a
//!   position to insert new lines at — see [`EditState`]'s own docs for
//!   how the same `next`/`end` pair models both.
//! - [`delete`] (`:delete`) is immediate: [`Session::delete_line`] per
//!   removed line, nothing queued or retyped.
//!
//! A few pieces are `pub(crate)` for the TUI's Vim mode to reuse directly,
//! since its `:deassert`/`:edit` entry points need the identical line-
//! finding/validation/hint logic: [`find_first_assertion`],
//! [`is_assertion_line`], [`assertion_constraint_text`],
//! [`suggest_related_constraints`], [`mentioned_vars`], [`parse_start_line`].
//!
//! While a queue-based edit is active, `commands::dispatch` is bypassed
//! entirely: [`handle`] is the sole entry point, recognizing `:skip`,
//! `:done`, `:cancel`, and — routed to their own command code, not
//! dispatch, since neither the undo-stack bookkeeping nor the `:formula
//! cancel` invalidation dispatch does around a call means anything for a
//! command that changes nothing — `:show`/`:list`/`:objective`/`:check`/
//! `:explain`/`:why` (see [`READONLY_DURING_EDIT`]). Everything else
//! starting with `:` is reported as unavailable rather than ambiguous
//! with proof-rule text (which never starts with `:` in v3 syntax);
//! anything not starting with `:` is the replacement for whatever's
//! currently queued (or, once the queue's empty, a new line inserted
//! after it).

use crate::commands::{check, explain, list, objective, resolve_command, show, why};
use crate::output::{Output, outln};
use crate::session::Session;

/// Commands safe to run without disturbing a queued edit — none of them
/// touch the buffer. `:show`/`:list`/`:objective`/`:explain`/`:why` just
/// print; `:check` is, per its own docs, "always a dry run... the live
/// session is never touched" even outside an edit. `:explain`/`:why`
/// still refuse a line that isn't checked, same as always — an active
/// edit doesn't change that.
pub(crate) const READONLY_DURING_EDIT: &[&str] =
    &["show", "list", "objective", "check", "explain", "why"];

/// Run one of `READONLY_DURING_EDIT`'s commands, bypassing
/// `commands::dispatch` entirely: dispatch's undo-stack bookkeeping and
/// `:formula cancel` invalidation both exist for commands that change
/// something, neither means anything here. `pub(crate)` — originally
/// [`handle`]'s own helper, now shared with `commands::debug`, which
/// reuses the exact same allowlist (`READONLY_DURING_EDIT`) for the same
/// reason: `:show`/`:list`/`:objective`/`:check`/`:explain`/`:why` are
/// just as safe to run without disturbing a `:debug` stop as they are
/// mid-`:edit`.
pub(crate) fn run_readonly(
    session: &Session,
    cmd: &str,
    args: &str,
    out: &mut dyn Output,
) -> anyhow::Result<()> {
    match cmd {
        "show" => show::run(session, args, out),
        "list" => list::run(session, out),
        "objective" => objective::run(session, out),
        "explain" => explain::run(session, args, out),
        "check" => check::run(session, args, out)?,
        "why" => why::run(session, args, out),
        _ => unreachable!("only ever called with a READONLY_DURING_EDIT name"),
    }
    Ok(())
}

/// A queue-based edit's entire state: which buffer lines are still
/// waiting to be offered for retyping, plus enough of the pre-edit buffer
/// to restore on `:cancel`.
///
/// `next` and `end` together model both halves of what this covers:
/// while `next < end`, `buffer[next]` is an *original* line still
/// waiting to be retyped or skipped (retyping it commits in place via
/// [`Session::set_line`] and advances `next`; skipping it removes it via
/// [`Session::delete_line`] and shrinks `end`, since the line after it
/// slides into the same index). Once `next == end`, nothing original is
/// left — any further typed line is a brand new insertion at that same
/// position (`Session::insert_line`), advancing both `next` and `end`
/// together so repeated free typing keeps inserting in order. `:insert`
/// starts directly in this second state (`next == end` from the outset,
/// nothing pulled out to offer); `:edit`/`:deassert` start in the first
/// and fall into the second once their queue drains.
pub struct EditState {
    next: usize,
    end: usize,
    /// The bare-number shorthand (`:edit n`, no `-`) finishes the instant
    /// its one queued line is accepted; an explicit range — including a
    /// degenerate one-line range (`:edit n-n`) — always waits for an
    /// explicit `:done`, since there's no way to tell "that submission
    /// was your last edit" from "you want to keep typing more lines"
    /// without being told. `:deassert`/`:insert` always behave like the
    /// latter: both are inherently open-ended, never a one-line swap.
    single_line: bool,
    /// For a `:deassert`-started edit, the bare constraint text (label and
    /// `a` keyword stripped) of the assertion being replaced — captured
    /// once at the start, so it stays available for the rest of the edit.
    /// `None` for an ordinary `:edit`/`:insert`. See [`Self::deassert_source`].
    deassert_source: Option<String>,
    /// The buffer, checked prefix, and known-bad state from immediately
    /// before this edit began — what `:cancel` restores, verbatim, no
    /// re-verification needed (all of it was already exactly this before
    /// a single keystroke of this edit landed).
    prior_buffer: Vec<String>,
    prior_checked_len: usize,
    prior_known_bad: Option<String>,
}

impl EditState {
    /// What `:deassert` is replacing, for display alongside the edit —
    /// e.g. the TUI's Output-pane heading, or the plain frontend's
    /// prompt — so it stays visible for the whole edit, not just the
    /// intro message printed once at the start. `None` outside a
    /// `:deassert`-started edit.
    pub(crate) fn deassert_source(&self) -> Option<&str> {
        self.deassert_source.as_deref()
    }
}

/// What the frontend should do after routing one line through [`handle`].
pub enum EditFlow {
    /// Still editing.
    Continue,
    /// The edit just ended (finished or cancelled) — normal mode resumes.
    Ended,
}

/// Parses `<n>` or `<n>-<m>`, returning `(lo, hi, was_a_range)` — the
/// third element distinguishes `:edit 5` from `:edit 5-5`, which parse to
/// the same numbers but mean different things (see `EditState::single_line`).
/// `pub(crate)` so `commands::formula::start` can reuse the exact same
/// syntax (and single-vs-range distinction) for `:formula <n>`/`:formula
/// <n>-<m>` rather than duplicating it.
pub(crate) fn parse_range(args: &str) -> Result<(usize, usize, bool), String> {
    if args.is_empty() {
        return Err("usage :edit <n> or :edit <n>-<m>".to_string());
    }
    match args.split_once('-') {
        Some((lo, hi)) => {
            let lo: usize = lo
                .trim()
                .parse()
                .map_err(|_| format!("invalid line number '{lo}'"))?;
            let hi: usize = hi
                .trim()
                .parse()
                .map_err(|_| format!("invalid line number '{hi}'"))?;
            if hi < lo {
                return Err(format!("usage :edit <n>-<m> with n <= m (got {lo}-{hi})"));
            }
            Ok((lo, hi, true))
        }
        None => {
            let n: usize = args
                .parse()
                .map_err(|_| format!("invalid line number '{args}'"))?;
            Ok((n, n, false))
        }
    }
}

/// Just the starting line of `:edit <n>` / `:edit <n>-<m>`, for the TUI's
/// Vim-mode entry point (`tui::App::start_vim_edit`) — a range only ever
/// picks where the cursor starts there, never queues anything, so the end
/// of the range (and whether a `-` was used) is unused.
pub(crate) fn parse_start_line(args: &str) -> Result<usize, String> {
    parse_range(args).map(|(lo, ..)| lo)
}

/// Shared mechanics behind `:edit` and `:deassert`: queue lines `lo..=hi`
/// for retyping. Otherwise read-only — nothing is truncated or otherwise
/// touched until an actual retype/skip happens — except for one
/// deliberate marker bump of `session.generation`, for the same reason
/// `commands::formula::start` bumps it at mode entry: with no other
/// mutation to notice, `commands::dispatch`'s undo-stack tracking
/// wouldn't otherwise realize an edit session began, and everything a
/// later retype commits (via `edit::handle`, which bypasses `dispatch`
/// entirely while the mode is active) would be unreachable to `:undo`
/// once the mode ends. `single_line` is passed in rather than derived
/// from `lo == hi`, since a caller may want a degenerate one-line range
/// to still behave like a real range (see `EditState::single_line`).
/// Callers own their own intro messaging — this only reports a bad line
/// number.
fn start_at(
    session: &mut Session,
    lo: usize,
    hi: usize,
    single_line: bool,
    out: &mut dyn Output,
) -> Option<EditState> {
    let Some(start_idx) = session.buffer_index(lo) else {
        outln!(
            out,
            "Error: line {lo} is the synthesized preamble or past the end of the proof — \
             nothing to edit there."
        );
        return None;
    };
    let Some(end_idx) = session.buffer_index(hi) else {
        outln!(
            out,
            "Error: line {hi} is the synthesized preamble or past the end of the proof — \
             nothing to edit there."
        );
        return None;
    };

    session.generation += 1;
    Some(EditState {
        next: start_idx,
        end: end_idx + 1,
        single_line,
        // Set by `start_deassert` after this returns, for a
        // `:deassert`-started edit specifically — plain `:edit`/`:insert`
        // have no single "assertion being replaced" to remember.
        deassert_source: None,
        prior_buffer: session.buffer.clone(),
        prior_checked_len: session.checked_len,
        prior_known_bad: session.known_bad.clone(),
    })
}

/// `:edit <arg>` — parse and start a *plain-mode* queue edit on the named
/// line(s). Never called by the TUI — see the module docs. With no
/// argument, targets the last buffer line, matching the TUI's Vim mode
/// (which starts there too when opened bare) rather than erroring —
/// otherwise a plain-mode user would need to already know that line
/// number just to get the same starting point a TUI user gets for free.
pub fn start(session: &mut Session, args: &str, out: &mut dyn Output) -> Option<EditState> {
    let (lo, hi, is_range) = if args.trim().is_empty() {
        if session.buffer.is_empty() {
            outln!(out, "Error: no proof lines yet to edit.");
            return None;
        }
        let last = session.display_line(session.buffer.len() - 1);
        (last, last, false)
    } else {
        match parse_range(args.trim()) {
            Ok(range) => range,
            Err(msg) => {
                outln!(out, "Error: {msg}");
                return None;
            }
        }
    };
    let state = start_at(session, lo, hi, !is_range, out)?;
    if is_range {
        outln!(
            out,
            "Editing lines {lo}-{hi} — retype each in turn (:skip to drop one), then \
             :done when finished. :cancel to abandon the whole edit."
        );
    } else {
        outln!(
            out,
            "Editing line {lo} — retype it and submit. :cancel to abandon instead."
        );
    }
    announce_next(&state, session, out);
    Some(state)
}

/// Whether `line` is an `a`-rule (unchecked assertion) — `a <constraint>
/// ...;`, optionally preceded by a `@label` (labels sit ahead of the rule
/// keyword, not after — see `proof_format_overview.md`'s "Constraint
/// Labels" section). An exact token match, not a prefix check, so
/// distinct rules that happen to start with the letter `a` (there are
/// none today, but `ia`/`obj`-style names are the shape to worry about)
/// can never false-match. `pub(crate)` so the TUI can check a
/// double-clicked line itself, to route to `:deassert` instead of plain
/// Vim-mode editing when it's an assertion.
pub(crate) fn is_assertion_line(line: &str) -> bool {
    let mut tokens = line.split_whitespace();
    match tokens.next() {
        Some(t) if t.starts_with('@') => tokens.next() == Some("a"),
        Some(t) => t == "a",
        None => false,
    }
}

/// The bare constraint text of an `a`-rule line — its label (if any) and
/// the `a` keyword itself stripped, along with a trailing `;` — for
/// display alongside "what am I replacing" while `:deassert` is active
/// (see `EditState::deassert_source`). `line` must already satisfy
/// [`is_assertion_line`]; this doesn't re-validate, and returns whatever
/// happens to be left if it doesn't. `pub(crate)` so the TUI's own Vim-
/// mode `:deassert` entry point can build the same header text without
/// going through the plain queue's `start_deassert`.
pub(crate) fn assertion_constraint_text(line: &str) -> String {
    let mut rest = line.trim();
    // A label sits ahead of the `a` keyword — skip it first, if present.
    if rest.starts_with('@')
        && let Some((_, after)) = rest.split_once(char::is_whitespace)
    {
        rest = after.trim_start();
    }
    // Skip the `a` keyword itself.
    rest = match rest.split_once(char::is_whitespace) {
        Some((_, after)) => after.trim(),
        None => "",
    };
    rest.strip_suffix(';').unwrap_or(rest).trim().to_string()
}

/// Every variable `text` mentions, in first-seen order — picked out the
/// same lookup-based way the TUI's syntax highlighter finds variable
/// references in raw rule text: no fixed grammar to lean on, since a
/// proof line's constraint syntax is free-form, so this can under-detect a
/// variable glued directly to punctuation with no separating space, but
/// never mislabel something else as one. Shared by
/// `suggest_related_constraints` (the one-time hint list `:deassert`
/// prints at the start) and the TUI's Database-pane highlighting (the
/// same relevance signal, kept current for the whole edit instead of
/// frozen at the start — see `tui::draw::database_lines`).
pub(crate) fn mentioned_vars(text: &str, var_names: &VarNameManager) -> Vec<VarIdx> {
    let mut vars = Vec::new();
    for token in text.split_whitespace() {
        let name = token.strip_prefix('~').unwrap_or(token);
        if let Some(var) = var_names.get_idx(name)
            && !vars.contains(&var)
        {
            vars.push(var);
        }
    }
    vars
}

/// The display line number of the first `a`-rule in the buffer, if any —
/// what bare `:deassert` (no line number) targets. Always the *earliest*
/// one: assertions are meant to be temporary scaffolding (see
/// `proof_format_overview.md`'s "Unchecked Assertion" section), and
/// clearing them from the front keeps the remaining count shrinking in a
/// predictable order rather than jumping around the proof. Looks at the
/// whole buffer, checked or not — an `a`-rule is never itself rejected
/// (it's unchecked by definition), so one can perfectly well be sitting
/// in the unchecked tail too. `pub(crate)` so the TUI's own Vim-mode
/// `:deassert` entry point can reuse the identical search.
pub(crate) fn find_first_assertion(session: &Session) -> Option<usize> {
    session
        .buffer
        .iter()
        .position(|line| is_assertion_line(line))
        .map(|idx| session.display_line(idx))
}

/// `:deassert [<n>]` — find an `a`-rule and open it for editing exactly
/// like an explicit one-line range (`single_line = false`): accepting a
/// replacement doesn't auto-finish, so any number of real derivation
/// steps (`rup`, `pol`, `red`, ...) can go in before `:done` retires it.
/// Nothing requires the replacement to be a single rule — that's the
/// whole point, versus `:edit n`. With no argument, targets the earliest
/// assertion in the proof (see `find_first_assertion`); with one, targets
/// that specific line instead — still validated as an actual `a`-rule,
/// so pointing it at an ordinary line errors rather than silently doing
/// something else.
pub fn start_deassert(
    session: &mut Session,
    args: &str,
    out: &mut dyn Output,
) -> Option<EditState> {
    let line = if args.trim().is_empty() {
        match find_first_assertion(session) {
            Some(line) => line,
            None => {
                outln!(out, "No `a` (unchecked assertion) rules left in the proof.");
                return None;
            }
        }
    } else {
        let n: usize = match args.trim().parse() {
            Ok(n) => n,
            Err(_) => {
                outln!(out, "Error: usage :deassert [<n>]");
                return None;
            }
        };
        let Some(idx) = session.buffer_index(n) else {
            outln!(
                out,
                "Error: line {n} is the synthesized preamble or past the end of the proof — \
                 nothing to deassert there."
            );
            return None;
        };
        if !is_assertion_line(&session.buffer[idx]) {
            outln!(
                out,
                "Error: line {n} isn't an `a`-rule (unchecked assertion) — nothing to deassert \
                 there. Try :edit {n} if you meant to retype it anyway."
            );
            return None;
        }
        n
    };

    // Captured up front, since `assertion_text` needs to outlive `state`'s
    // construction below (both borrow `session`).
    let assertion_text = session.buffer[session.buffer_index(line)?].clone();

    let mut state = start_at(session, line, line, false, out)?;
    state.deassert_source = Some(assertion_constraint_text(&assertion_text));
    outln!(
        out,
        "De-asserting line {line} — replace the `a` rule with one or more real derivation \
         steps, then :done once you're satisfied. :cancel to abandon and restore the assertion."
    );
    announce_next(&state, session, out);
    suggest_related_constraints(session, &assertion_text, out);
    Some(state)
}

/// Hint at what's around to build the replacement from: every checked
/// constraint currently live that mentions any variable the assertion
/// itself mentioned (see `mentioned_vars`). Printed once, here, as a
/// starting point — the TUI additionally keeps this signal current for
/// the *whole* edit by highlighting the same matches live in the
/// Database pane (see `tui::draw::database_lines`), since a one-time list
/// goes stale the moment a new intermediate constraint joins the database.
/// Purely a courtesy listing either way: nothing here is verified or
/// required. `pub(crate)` so the TUI's own Vim-mode `:deassert` entry
/// point can print the identical hint list.
pub(crate) fn suggest_related_constraints(
    session: &Session,
    assertion_text: &str,
    out: &mut dyn Output,
) {
    let checker = &session.current_checker;
    let var_names = &checker.context.var_names;

    let vars = mentioned_vars(assertion_text, var_names);
    if vars.is_empty() {
        return;
    }

    // (constraint ID, how many of `vars` it mentions) — most-relevant
    // first (most variables in common), then most-recent, since that's
    // usually what you were just working on and so most likely relevant.
    let mut matches: Vec<(usize, usize)> = checker
        .database
        .entries
        .iter()
        .enumerate()
        .filter_map(|(id, entry)| {
            let entry = entry.as_ref()?;
            let shared = (0..entry.constraint.len())
                .filter_map(|i| entry.constraint.get_lit(i))
                .filter(|lit| vars.contains(&lit.get_var()))
                .count();
            (shared > 0).then_some((id, shared))
        })
        .collect();
    if matches.is_empty() {
        return;
    }
    matches.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));

    let var_list: Vec<&str> = vars.iter().map(|&v| var_names.get_name(v)).collect();
    outln!(out, "Constraints mentioning {}:", var_list.join(", "));

    const SHOWN: usize = 8;
    let labels_by_id = session.labels_by_id();
    for &(id, _) in matches.iter().take(SHOWN) {
        let entry = checker.database.entries[id]
            .as_ref()
            .expect("just matched above — still present");
        let tag = if entry.is_core_constraint() {
            "core"
        } else {
            "derived"
        };
        let labels = labels_by_id
            .get(&(id as isize))
            .map(|names| format!("{} ", names.join(" ")))
            .unwrap_or_default();
        outln!(
            out,
            "  ConstraintId {id}: {labels}{} [{tag}]",
            entry.constraint.to_pretty_string(var_names)
        );
    }
    if matches.len() > SHOWN {
        outln!(
            out,
            "  ... (+{} more — :show {} for everything mentioning just that one)",
            matches.len() - SHOWN,
            var_list[0]
        );
    }
}

/// Remove lines `lo..=hi` immediately — `:delete`'s mechanics. Deleted
/// back-to-front internally so earlier removals don't shift the indices
/// later ones still need — from the caller's perspective it's just
/// "these lines are gone," in one step, nothing to reject and nothing
/// left to reverify.
fn delete(
    session: &mut Session,
    lo: usize,
    hi: usize,
    out: &mut dyn Output,
) -> anyhow::Result<()> {
    let Some(start_idx) = session.buffer_index(lo) else {
        outln!(
            out,
            "Error: line {lo} is the synthesized preamble or past the end of the proof — \
             nothing to delete there."
        );
        return Ok(());
    };
    let Some(end_idx) = session.buffer_index(hi) else {
        outln!(
            out,
            "Error: line {hi} is the synthesized preamble or past the end of the proof — \
             nothing to delete there."
        );
        return Ok(());
    };

    let removed = end_idx - start_idx + 1;
    for _ in 0..removed {
        session.delete_line(start_idx)?;
    }
    outln!(out, "Deleted {removed} line(s).");
    Ok(())
}

/// `:delete <n>` / `:delete <n>-<m>` — parse and delete. No `:edit
/// cancel`-style safety net, deliberately: this is meant to be a quick,
/// direct action, not a staged one — same recovery as any other committed
/// change (retype it, or :undo).
pub fn start_delete(session: &mut Session, args: &str, out: &mut dyn Output) -> anyhow::Result<()> {
    let (lo, hi, _) = match parse_range(args.trim()) {
        Ok(range) => range,
        Err(msg) => {
            outln!(out, "Error: {msg}");
            return Ok(());
        }
    };
    delete(session, lo, hi, out)
}

/// `:insert <n>` — start an open-ended, queue-based edit that inserts new
/// lines *before* the line currently numbered `n` (or at the very end,
/// if `n` names the line one past the last one — the same convention
/// `Vec::insert` uses for its index), pushing everything from `n` onward
/// down without touching it. Nothing is pulled out to retype — the queue
/// starts empty (`next == end` from the outset — see `EditState`'s own
/// docs), so this behaves exactly like `:deassert` from the first
/// keystroke on: type as many new lines as it takes, `:done` when
/// finished, `:skip`/`:cancel` still apply (skip is simply a no-op here,
/// since there's nothing queued to skip).
pub fn start_insert(session: &mut Session, args: &str, out: &mut dyn Output) -> Option<EditState> {
    let n: usize = match args.trim().parse() {
        Ok(n) => n,
        Err(_) => {
            outln!(out, "Error: usage :insert <n>");
            return None;
        }
    };
    let end_of_proof = session.display_line(session.buffer.len());
    let insert_idx = if n == end_of_proof {
        session.buffer.len()
    } else {
        match session.buffer_index(n) {
            Some(idx) => idx,
            None => {
                outln!(
                    out,
                    "Error: line {n} is the synthesized preamble or past the end of the proof \
                     — nowhere to insert there. Use :insert {end_of_proof} to add lines at the \
                     end, or just type them normally."
                );
                return None;
            }
        }
    };

    let state = EditState {
        next: insert_idx,
        end: insert_idx,
        single_line: false,
        deassert_source: None,
        prior_buffer: session.buffer.clone(),
        prior_checked_len: session.checked_len,
        prior_known_bad: session.known_bad.clone(),
    };
    // Marker bump — see `start_at`'s own doc comment for why: mode entry
    // otherwise mutates nothing at all, and `commands::dispatch`'s
    // undo-stack tracking needs to notice one began anyway.
    session.generation += 1;
    outln!(
        out,
        "Inserting before line {n} — type one or more new lines, then :done when finished. \
         :cancel to abandon and restore things as they were."
    );
    announce_next(&state, session, out);
    Some(state)
}

/// Print whatever's queued next: the current text of `buffer[next]` if
/// anything original is still left, or an invitation to type freely once
/// the queue's drained. Always reads the buffer fresh rather than a
/// stored copy — there's no separate "queued text" state to keep in sync.
fn announce_next(state: &EditState, session: &Session, out: &mut dyn Output) {
    if state.next < state.end {
        let display = session.display_line(state.next);
        outln!(out, "Line {display}: {}", session.buffer[state.next]);
    } else {
        outln!(
            out,
            "(nothing left queued — type new lines freely, then :done)"
        );
    }
}

/// Commit `line` as either the retyped replacement for the currently
/// queued original line (`next < end`) or a brand new insertion once the
/// queue's drained — see `EditState`'s own docs for how `next`/`end`
/// model both. Always succeeds (nothing is checked), so there's nothing
/// left to report beyond "here's what's queued next."
fn submit(
    session: &mut Session,
    state: &mut EditState,
    line: &str,
    out: &mut dyn Output,
) -> anyhow::Result<EditFlow> {
    if state.next < state.end {
        session.set_line(state.next, line)?;
        outln!(out, "Line {} updated.", session.display_line(state.next));
        state.next += 1;
    } else {
        session.insert_line(state.next, line)?;
        outln!(out, "Line {} added.", session.display_line(state.next));
        state.next += 1;
        state.end += 1;
    }
    if state.single_line {
        outln!(out, "Edit complete.");
        return Ok(EditFlow::Ended);
    }
    announce_next(state, session, out);
    Ok(EditFlow::Continue)
}

/// `:skip` — drop the currently queued line without replacing it (i.e.
/// delete it from the buffer outright, checked or not).
fn skip(session: &mut Session, state: &mut EditState, out: &mut dyn Output) -> anyhow::Result<()> {
    if state.next < state.end {
        let display = session.display_line(state.next);
        session.delete_line(state.next)?;
        state.end -= 1;
        outln!(out, "Skipped line {display} (deleted).");
    } else {
        outln!(out, "(nothing queued to skip)");
    }
    announce_next(state, session, out);
    Ok(())
}

/// `:cancel` — instantly restore the buffer (and checked prefix) exactly
/// as they were before the edit began. No re-verification needed:
/// `prior_buffer`/`prior_checked_len` were already known-good.
fn cancel(session: &mut Session, state: &EditState, out: &mut dyn Output) -> anyhow::Result<()> {
    session.restore_buffer_state(
        state.prior_buffer.clone(),
        state.prior_checked_len,
        state.prior_known_bad.clone(),
    )?;
    outln!(out, "Edit cancelled — buffer restored.");
    Ok(())
}

/// Route one input line while a queue-based edit is active — the sole
/// entry point either frontend needs during one (`:edit` in plain,
/// `:deassert`/`:insert` in both): `commands::dispatch` isn't consulted
/// at all until `EditFlow::Ended` comes back.
pub fn handle(
    session: &mut Session,
    state: &mut EditState,
    line: &str,
    out: &mut dyn Output,
) -> anyhow::Result<EditFlow> {
    match line.trim() {
        ":cancel" => {
            cancel(session, state, out)?;
            Ok(EditFlow::Ended)
        }
        ":skip" => {
            skip(session, state, out)?;
            Ok(EditFlow::Continue)
        }
        ":done" => {
            if state.next < state.end {
                outln!(
                    out,
                    "Leaving {} original line(s) unchanged.",
                    state.end - state.next
                );
            }
            outln!(out, "Edit complete.");
            Ok(EditFlow::Ended)
        }
        // A bare Enter. This module's prompt is never pre-filled (that's
        // only the TUI's Vim-mode `Insert`, which has its own delete-on-
        // empty-line meaning via `x`/`dd` instead) — so what's on screen
        // is just the
        // reference text `announce_next` printed, e.g. "Line 5: ...". That
        // reads like a value already sitting there, and the expectation is
        // that Enter alone accepts it rather than silently doing nothing.
        // So: keep the queued line's text exactly as it is and move on.
        // Once the queue's drained (free typing after it), there's no
        // line "shown" to reaffirm, so this stays the no-op it always was.
        "" => {
            if state.next < state.end {
                let display = session.display_line(state.next);
                outln!(out, "Keeping line {display} unchanged.");
                state.next += 1;
                if state.single_line {
                    outln!(out, "Edit complete.");
                    Ok(EditFlow::Ended)
                } else {
                    announce_next(state, session, out);
                    Ok(EditFlow::Continue)
                }
            } else {
                Ok(EditFlow::Continue)
            }
        }
        other if other.starts_with(':') => {
            let rest = &other[1..];
            let mut parts = rest.splitn(2, char::is_whitespace);
            let cmd_name = parts.next().unwrap_or("");
            let cmd_args = parts.next().unwrap_or("").trim();
            match resolve_command(cmd_name) {
                Ok(resolved) if READONLY_DURING_EDIT.contains(&resolved) => {
                    run_readonly(session, resolved, cmd_args, out)?;
                    Ok(EditFlow::Continue)
                }
                _ => {
                    outln!(
                        out,
                        "Only :show, :list, :objective, :check, :explain, :why, :skip, \
                         :done, and :cancel work while editing — type the replacement \
                         line, or one of those."
                    );
                    Ok(EditFlow::Continue)
                }
            }
        }
        _ => submit(session, state, line, out),
    }
}
