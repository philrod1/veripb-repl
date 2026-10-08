//! `:formula [<n>]` / `:formula <n>-<m>` — switch into formula-editing
//! mode: retype one or more existing formula constraints' content in
//! place, each commit immediately reverifying the whole proof against the
//! edited formula. Also `:formula cancel`, its undo, usable both mid-mode
//! and afterward.
//!
//! Shaped as a direct mirror of `commands::edit`'s queue-based `:edit`
//! flow (`start`/`handle`, [`FormulaEditState`]/[`FormulaEditFlow`]), but
//! carrying formula-constraint numbers instead of buffer lines, and with
//! no "tail" concept at all: a formula constraint is replaced in place,
//! live, the moment it's retyped. That also means cancelling before
//! anything's been retyped is a true no-op (nothing was ever touched),
//! and `:done` ending the mode early with some of a range still unvisited
//! leaves those constraints exactly as they were — nothing to "discard".
//!
//! Only in-place, same-count edits are supported (see
//! `Session::replace_formula_constraint`'s own docs for why `=`
//! constraints are rejected) — constraint IDs are purely positional, so an
//! in-place edit leaves every other ID, formula and proof-derived alike,
//! numerically unchanged. Inserting or deleting a formula constraint would
//! reindex the whole proof instead; that's out of scope here, and remains
//! a job for editing the OPB file directly and `:load`ing it.
//!
//! Every individual commit ([`commit`]) can invalidate *any* proof line,
//! not just ones positionally after it, so the whole buffer is reverified
//! from scratch each time via `Session::replace_buffer_and_verify` — one
//! O(n) batched replay for the common case, falling back to a one-line-
//! at-a-time loop only if that fails, to pinpoint exactly where (the same
//! two-tier approach `:source` used to use before it stopped checking on
//! load at all). A commit whose new constraint text is itself valid
//! always lands; whatever of the prior buffer still reverifies cleanly is
//! committed (`checked_len` advances that far), the first line that no
//! longer holds stays in the buffer, unchecked, right where it is — never
//! discarded — for the user to fix forward with `:edit`/`:delete`/
//! `:insert` and `:verify` again. `:formula cancel` undoes only the
//! single most recent commit, wherever it happened (mid-mode or at the
//! ordinary prompt afterward) — see `commands::dispatch`'s
//! `last_formula_edit` invalidation for the one-shot scope.

use std::collections::VecDeque;

use crate::commands::edit;
use crate::commands::help::Topic;
use crate::output::{self, Output, outln};
use crate::session::Session;

/// Constraint numbers still to walk through, front-first.
pub struct FormulaEditState {
    queue: VecDeque<usize>,
    /// Bare `:formula n` (no `-`): auto-exits after the one retype/keep,
    /// mirroring `edit::EditState::single_line`. An explicit range
    /// (including a degenerate `n-n`) always waits for `:done`.
    single: bool,
}

/// Every `:`-command actually recognized while a formula edit is active,
/// mode or browse alike (see [`handle`] and `tui::App::formula_browse_apply`)
/// — just these two; unlike `edit::handle`, no read-only commands work
/// here yet (see this module's own note in `edit::READONLY_DURING_EDIT`'s
/// docs). Neither is a real dispatchable command outside this mode, so
/// neither is registered in `help::COMMANDS` — same [`Topic`] shape as
/// that registry purely so the TUI's suggestion strip can build
/// candidates for these the way it already does for everything else.
pub(crate) const MODE_VOCABULARY: &[Topic] = &[
    Topic {
        name: "formula cancel",
        usage: ":formula cancel",
        summary: "undo the most recent commit, without leaving the mode",
        details: &[],
    },
    Topic {
        name: "done",
        usage: ":done",
        summary: "leave formula-editing mode",
        details: &[],
    },
];

/// What the frontend should do after routing one line through [`handle`].
pub enum FormulaEditFlow {
    /// Still editing.
    Continue,
    /// The mode just ended (finished or `:done`) — normal mode resumes.
    Ended,
}

fn announce_next(state: &FormulaEditState, session: &Session, out: &mut dyn Output) {
    let Some(&n) = state.queue.front() else {
        return;
    };
    let idx = session
        .formula_index(n)
        .expect("validated when the mode started");
    outln!(out, "Constraint {}: {}", session.formula_line_ids(idx), session.formula[idx]);
}

/// `:formula [<n>]` / `:formula <n>-<m>` — parse and start formula-editing
/// mode on the named constraint(s). With no argument, targets the *last*
/// formula constraint, mirroring how bare `:edit` targets the last
/// *accepted* proof line — the "end of the sequence" default in both
/// cases. Never called by the TUI for entering the mode itself — see
/// `tui::App::start_formula_browse`, which drives its own arrow-cursor
/// browse instead — but `commit` (below) is shared by both frontends.
pub fn start(session: &mut Session, args: &str, out: &mut dyn Output) -> Option<FormulaEditState> {
    let (lo, hi, is_range) = if args.trim().is_empty() {
        if session.formula.is_empty() {
            outln!(out, "Error: the formula has no constraints to edit.");
            return None;
        }
        let last = *session.formula_first_ids().last().expect("checked non-empty");
        (last, last, false)
    } else {
        match edit::parse_range(args.trim()) {
            Ok(range) => range,
            Err(msg) => {
                outln!(out, "Error: {msg}");
                return None;
            }
        }
    };
    let total = session.formula_constraint_count();
    if lo < 1 || hi > total {
        outln!(
            out,
            "Error: constraint {} is out of range — the formula has {total} constraint(s) \
             (1-{total}).",
            if lo < 1 { lo } else { hi },
        );
        return None;
    }

    // One queue entry per formula line in the range, as its first ID.
    let first_ids = session.formula_first_ids();
    let mut queue: Vec<usize> = (lo..=hi)
        .filter_map(|n| session.formula_index(n).map(|idx| first_ids[idx]))
        .collect();
    queue.dedup();
    let state = FormulaEditState {
        queue: queue.into(),
        single: !is_range,
    };
    // Formula mode entry doesn't otherwise mutate anything — a constraint
    // is only ever replaced once actually retyped, the same shape as
    // `:edit`'s queue entry, which likewise leaves every line alone until
    // one is. Bump the generation counter as a deliberate marker anyway,
    // purely so `commands::dispatch`'s undo-stack tracking notices a mode
    // began and pushes the pre-mode checkpoint; nothing else reads this
    // bump.
    session.generation += 1;
    if is_range {
        outln!(
            out,
            "Editing formula constraints {lo}-{hi} — retype each in turn (blank to keep it \
             unchanged), then :done when finished. :formula cancel undoes the most recent commit."
        );
    } else {
        let ids = session.formula_line_ids(session.formula_index(lo).expect("validated above"));
        outln!(
            out,
            "Editing formula constraint {ids} — retype it and submit; the proof reverifies \
             automatically. :formula cancel undoes it if needed."
        );
    }
    announce_next(&state, session, out);
    Some(state)
}

/// Parse `new_text` as constraint `n`'s replacement, commit it via
/// `Session::replace_formula_constraint`, and reverify as much of the
/// fullest buffer still worth trying as still holds (see
/// `Session::recoverable_buffer` — not necessarily just whatever `buffer`
/// happens to be right now, if an earlier edit in this sequence only
/// partially reverified). Shared by the plain queue's `handle` and the
/// TUI browse's commit action — the only two places either frontend
/// actually touches the session for a formula edit. Returns whether it
/// actually committed, so callers can decide whether to advance (queue)
/// or leave the cursor put (TUI) versus stay on the same constraint to
/// retry.
pub(crate) fn commit(
    session: &mut Session,
    n: usize,
    new_text: &str,
    out: &mut dyn Output,
) -> anyhow::Result<bool> {
    if let Err(err) = session.replace_formula_constraint(n, new_text) {
        outln!(out, "Error: {err:#}");
        outln!(out, "(still editing constraint {n})");
        return Ok(false);
    }

    // `replace_formula_constraint` just succeeded, so all of this is
    // guaranteed to resolve.
    let idx = session.formula_index(n).expect("just succeeded");
    let snapshot = session.last_formula_edit.as_ref().expect("just set");
    let old_text = snapshot.constraint.clone();
    let new_text_pretty = session.formula[idx].clone();
    let base_buffer = snapshot.buffer.clone();

    outln!(out, "Constraint {} updated:", session.formula_line_ids(idx));
    outln!(out, "  was: {old_text}");
    outln!(out, "  now: {new_text_pretty}");

    let total = base_buffer.len();
    if total == 0 {
        outln!(out, "(no proof lines to reverify)");
        return Ok(true);
    }

    let (captured, rejection) = session.replace_buffer_and_verify(base_buffer)?;
    output::text(out, &captured);
    match rejection {
        None => {
            // `buffer` now equals the base this edit reverified, so
            // there's nothing extra left to remember for a future edit to
            // retry — a fresh one starts fresh from `buffer` again.
            session.recoverable_buffer = None;
            outln!(
                out,
                "Reverified all {total} proof line(s) against the edited formula."
            );
        }
        Some(err) => {
            output::error(out, &err);
            let checked = session.checked_len;
            let display_line = session.display_line(checked);
            let remaining = total - checked - 1;
            outln!(
                out,
                "Only {checked} of {total} proof line(s) checked out against the edited \
                 formula — line {display_line} was rejected, and {remaining} line(s) after it \
                 are sitting in the buffer, unchecked. Retype line {display_line} and :verify \
                 again once it's fixed, or :formula cancel to undo this edit."
            );
        }
    }
    Ok(true)
}

/// `:formula cancel` — undo the most recent commit, restoring both the
/// constraint it replaced and the buffer exactly as it was, in one step.
/// Reruns the same reverify `commit` used (rather than trusting a stored
/// checked-length) since restoring the identical formula/buffer pairing
/// that held before the edit is guaranteed to reproduce the identical
/// result — simpler than also snapshotting `checked_len`/`known_bad`
/// alongside the buffer. Only ever valid for the one commit that's still
/// the "most recent" thing that happened; anything else dispatched in
/// between clears it first (see `commands::dispatch`), so this can't
/// silently discard unrelated work. `pub(crate)` — called from
/// `dispatch`'s top-level `:formula cancel`, the plain queue's `handle`,
/// and the TUI browse's commit action, all three recognizing the same
/// command text.
pub(crate) fn cancel(session: &mut Session, out: &mut dyn Output) -> anyhow::Result<()> {
    let Some(snapshot) = session.last_formula_edit.take() else {
        outln!(
            out,
            "No formula edit to cancel — either none happened yet, or something since made it \
             stale."
        );
        return Ok(());
    };
    let idx = session
        .formula_index(snapshot.n)
        .expect("constraint count is invariant under a formula edit");
    session.formula[idx] = snapshot.constraint;
    let (captured, rejection) = session.replace_buffer_and_verify(snapshot.buffer)?;
    output::text(out, &captured);
    if let Some(err) = rejection {
        // Shouldn't happen — this is exactly the formula/buffer pairing
        // that held immediately before the edit being cancelled — but
        // report rather than silently leaving `known_bad` set with no
        // explanation if it somehow does.
        output::error(out, &err);
    }
    // `buffer` and the base this edit had tried to reverify are now back
    // in sync by construction — nothing extra left to remember.
    session.recoverable_buffer = None;
    outln!(
        out,
        "Formula edit to constraint {} cancelled — formula and buffer both restored.",
        snapshot.n
    );
    Ok(())
}

/// Shown when a `:`-command other than `:done`/`:formula cancel` is typed
/// in formula mode (plain frontend and TUI).
pub(crate) const ONLY_DONE_OR_CANCEL: &str = "Only :done and :formula cancel work while \
     editing the formula — type the replacement constraint, or one of those.";

/// Route one input line while formula-editing mode is active — the sole
/// entry point the plain frontend needs during one: `commands::dispatch`
/// isn't consulted at all until `FormulaEditFlow::Ended` comes back
/// (mirrors `edit::handle` exactly). The TUI never calls this — its own
/// arrow-browse commits directly via `commit`/`cancel` instead (see
/// `tui::App::formula_browse_apply`).
pub fn handle(
    session: &mut Session,
    state: &mut FormulaEditState,
    line: &str,
    out: &mut dyn Output,
) -> anyhow::Result<FormulaEditFlow> {
    match line.trim() {
        ":formula cancel" => {
            cancel(session, out)?;
            Ok(FormulaEditFlow::Continue)
        }
        ":done" => {
            outln!(out, "Left formula mode.");
            Ok(FormulaEditFlow::Ended)
        }
        // Nothing typed: keep the shown constraint unchanged and move on —
        // mirrors `:edit`'s own bare-Enter fix (Enter confirms the shown
        // reference rather than silently doing nothing), but there's
        // nothing to resubmit through the session here: unlike a proof
        // line, an unedited formula constraint needs no re-verification.
        "" => {
            let n = state
                .queue
                .pop_front()
                .expect("handle only called while queue is non-empty");
            let ids = session.formula_line_ids(
                session.formula_index(n).expect("validated when the mode started"),
            );
            outln!(out, "Keeping constraint {ids} unchanged.");
            if state.single || state.queue.is_empty() {
                outln!(out, "Left formula mode.");
                Ok(FormulaEditFlow::Ended)
            } else {
                announce_next(state, session, out);
                Ok(FormulaEditFlow::Continue)
            }
        }
        other if other.starts_with(':') => {
            outln!(out, "{ONLY_DONE_OR_CANCEL}");
            Ok(FormulaEditFlow::Continue)
        }
        _ => {
            let n = *state
                .queue
                .front()
                .expect("handle only called while queue is non-empty");
            if commit(session, n, line, out)? {
                state.queue.pop_front();
                if state.single || state.queue.is_empty() {
                    outln!(out, "Left formula mode.");
                    Ok(FormulaEditFlow::Ended)
                } else {
                    announce_next(state, session, out);
                    Ok(FormulaEditFlow::Continue)
                }
            } else {
                Ok(FormulaEditFlow::Continue)
            }
        }
    }
}
