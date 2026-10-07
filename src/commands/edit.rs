//! Plain-frontend line editing: `:edit <n>[-<m>]` (retype lines in place),
//! `:deassert [<n>]` (replace an `a`-rule with real derivation steps),
//! `:insert <n>` (add lines) and `:delete <n>[-<m>]` (remove lines).
//!
//! Edits never check anything: each mutation is an immediate edit to
//! `session.buffer`, and retyping a checked line only retracts `checked_len`
//! to it. Re-verification is `:verify`'s job.
//!
//! `:edit`/`:deassert`/`:insert` produce an [`EditState`] driven by [`handle`]
//! until `:done`/`:cancel`; `delete` is immediate. The TUI does not use the
//! queue (its Vim mode calls [`Session::set_line`]/`insert_line`/`delete_line`
//! directly) but reuses the `pub(crate)` helpers `find_first_assertion`,
//! `is_assertion_line`, `assertion_constraint_text`,
//! `suggest_related_constraints` and `parse_start_line`.
//!
//! While an edit is active, `commands::dispatch` is bypassed: [`handle`]
//! accepts `:skip`, `:done`, `:cancel` and the `READONLY_DURING_EDIT`
//! commands, rejects any other `:` input (proof-rule text never starts with
//! `:`), and treats everything else as the replacement or inserted line.

use crate::commands::{check, explain, list, objective, preserved, resolve_command, show};
use crate::output::{Output, outln};
use crate::session::Session;

/// The single allowlist of commands safe to run mid-`:edit` (none touch the
/// buffer; `:check` is always a dry run). `:debug` reuses it.
pub(crate) const READONLY_DURING_EDIT: &[&str] =
    &["show", "list", "objective", "preserved", "check", "explain"];

/// Runs a `READONLY_DURING_EDIT` command directly, skipping `dispatch`'s undo
/// and `:formula` bookkeeping (these commands change nothing). Used by `:edit`
/// and `:debug`. Panics on any other `cmd`.
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
        "preserved" => preserved::run(session, out),
        "explain" => explain::run(session, args, out),
        "check" => check::run(session, args, out)?,
        _ => unreachable!("only ever called with a READONLY_DURING_EDIT name"),
    }
    Ok(())
}

/// State of a queue-based edit: the queued buffer range plus the pre-edit
/// state that `:cancel` restores.
///
/// While `next < end`, `buffer[next]` is an original line awaiting retype
/// ([`Session::set_line`], advances `next`) or skip ([`Session::delete_line`],
/// shrinks `end`). Once `next == end`, each typed line is inserted at `next`
/// (`Session::insert_line`), advancing both. `:insert` starts with
/// `next == end`.
pub struct EditState {
    next: usize,
    end: usize,
    /// True only for bare `:edit n`: the edit ends once its one line is
    /// accepted. Ranges (even `n-n`), `:deassert` and `:insert` wait for `:done`.
    single_line: bool,
    /// For `:deassert`, the replaced assertion's bare constraint text; `None`
    /// otherwise. See [`Self::deassert_source`].
    deassert_source: Option<String>,
    /// Buffer, checked prefix and known-bad state from before the edit,
    /// restored verbatim by `:cancel` without re-verification.
    prior_buffer: Vec<String>,
    prior_checked_len: usize,
    prior_known_bad: Option<String>,
}

impl EditState {
    /// The constraint text `:deassert` is replacing, for display for the
    /// duration of the edit. `None` outside a `:deassert`-started edit.
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

/// Parses `<n>` or `<n>-<m>` into `(lo, hi, was_a_range)`; `was_a_range`
/// distinguishes `5` from `5-5` (see `EditState::single_line`). Errors on
/// empty input, non-numbers, or `hi < lo`. Also used by `:formula`.
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

/// The start line of `:edit <n>[-<m>]`, for the TUI's Vim-mode entry point
/// (`tui::App::start_vim_edit`), which only positions the cursor.
pub(crate) fn parse_start_line(args: &str) -> Result<usize, String> {
    parse_range(args).map(|(lo, ..)| lo)
}

/// Starts an edit with display lines `lo..=hi` queued for retyping. Leaves the
/// buffer untouched but bumps `session.generation` so `dispatch` records an
/// undo point for the edit (later changes go through [`handle`], bypassing
/// `dispatch`). Prints an error and returns `None` on an invalid line; callers
/// print their own intro.
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
        // Filled in by `start_deassert`.
        deassert_source: None,
        prior_buffer: session.buffer.clone(),
        prior_checked_len: session.checked_len,
        prior_known_bad: session.known_bad.clone(),
    })
}

/// `:edit [<n>[-<m>]]` — starts a plain-frontend queue edit. With no argument,
/// targets the last buffer line (matching the TUI's bare `:edit`).
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

/// Whether `line` is an `a`-rule (unchecked assertion), optionally preceded
/// by a `@label`. Matches the rule keyword as a whole token, not a prefix.
pub(crate) fn is_assertion_line(line: &str) -> bool {
    let mut tokens = line.split_whitespace();
    match tokens.next() {
        Some(t) if t.starts_with('@') => tokens.next() == Some("a"),
        Some(t) => t == "a",
        None => false,
    }
}

/// The constraint text of an `a`-rule line, with label, `a` keyword and
/// trailing `;` stripped. Expects `line` to satisfy [`is_assertion_line`];
/// does not re-validate.
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

/// Display line of the earliest `a`-rule anywhere in the buffer (checked or
/// not), if any. The target of bare `:deassert`.
pub(crate) fn find_first_assertion(session: &Session) -> Option<usize> {
    session
        .buffer
        .iter()
        .position(|line| is_assertion_line(line))
        .map(|idx| session.display_line(idx))
}

/// `:deassert [<n>]` — opens an `a`-rule for open-ended replacement (any
/// number of lines until `:done`). Targets `find_first_assertion` by default;
/// an explicit `<n>` must be an `a`-rule or it errors. Prints related
/// constraints as a hint.
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

    // Cloned before `start_at` takes `session` mutably.
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

/// Prints up to 8 live database constraints sharing a variable with
/// `assertion_text`, most shared variables first, then newest. Informational
/// only.
pub(crate) fn suggest_related_constraints(
    session: &Session,
    assertion_text: &str,
    out: &mut dyn Output,
) {
    let vars = session.variables.mentioned(assertion_text);
    if vars.is_empty() {
        return;
    }

    let database = match session.database() {
        Ok(database) => database,
        Err(err) => {
            outln!(out, "Error: {err:#}");
            return;
        }
    };

    // (constraint ID, number of `vars` shared), sorted by shared count, then
    // newest first.
    let mut matches: Vec<(usize, usize)> = database
        .entries
        .iter()
        .filter_map(|entry| {
            let mentioned = session.variables.mentioned(&entry.text);
            let shared = mentioned.iter().filter(|v| vars.contains(v)).count();
            (shared > 0).then_some((entry.id, shared))
        })
        .collect();
    if matches.is_empty() {
        return;
    }
    matches.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));

    outln!(out, "Constraints mentioning {}:", vars.join(", "));

    const SHOWN: usize = 8;
    let labels_by_id = session.labels_by_id();
    for &(id, _) in matches.iter().take(SHOWN) {
        let entry = database
            .get(id)
            .expect("just matched above — still present");
        let tag = if entry.is_core { "core" } else { "derived" };
        let labels = labels_by_id
            .get(&(id as isize))
            .map(|names| format!("{} ", names.join(" ")))
            .unwrap_or_default();
        outln!(out, "  ConstraintId {id}: {labels}{} [{tag}]", entry.text);
    }
    if matches.len() > SHOWN {
        outln!(
            out,
            "  ... (+{} more — :show {} for everything mentioning just that one)",
            matches.len() - SHOWN,
            vars[0]
        );
    }
}

/// Removes display lines `lo..=hi` immediately. Prints an error and returns
/// `Ok` on an invalid line.
fn delete(session: &mut Session, lo: usize, hi: usize, out: &mut dyn Output) -> anyhow::Result<()> {
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

/// `:delete <n>[-<m>]` — parses and deletes immediately; recoverable via
/// `:undo`.
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

/// `:insert <n>` — starts an open-ended edit inserting lines before display
/// line `n`, or at the end if `n` is one past the last line. The queue starts
/// empty, so `:skip` is a no-op.
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
    // Marks an undo point for `dispatch` (see `start_at`).
    session.generation += 1;
    outln!(
        out,
        "Inserting before line {n} — type one or more new lines, then :done when finished. \
         :cancel to abandon and restore things as they were."
    );
    announce_next(&state, session, out);
    Some(state)
}

/// Prints the queued line's current buffer text, or a free-typing prompt once
/// the queue is drained.
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

/// Commits `line` as the replacement for the queued line, or as a new line
/// once the queue is drained (see [`EditState`]). Ends the edit if
/// `single_line`.
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

/// `:skip` — deletes the queued line from the buffer.
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

/// `:cancel` — restores the pre-edit buffer, checked prefix and known-bad
/// state without re-verification.
fn cancel(session: &mut Session, state: &EditState, out: &mut dyn Output) -> anyhow::Result<()> {
    session.restore_buffer_state(
        state.prior_buffer.clone(),
        state.prior_checked_len,
        state.prior_known_bad.clone(),
    )?;
    outln!(out, "Edit cancelled — buffer restored.");
    Ok(())
}

/// Routes one input line during a queue-based edit; the frontend must not call
/// `commands::dispatch` until this returns `EditFlow::Ended`.
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
        // Bare Enter keeps the queued line unchanged and advances; a no-op
        // once the queue is drained.
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
                        "Only :show, :list, :objective, :preserved, :check, :explain, :skip, \
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
