//! `:why [all|needed]` — explain the last successful `rup` step, or, if
//! a line is currently rejected (`Session::known_bad` is set — see its
//! own docs), explain *that* instead: which already-derived constraints
//! (and/or the rule's own negation) actually contributed to the
//! conflict, as opposed to whatever hint list, if any, was typed.
//! `needed` (also what bare `:why` shows for a successful step, since
//! it's the only half currently buildable — see
//! [`crate::session::Session::rup_needed_hints`]'s own docs for why) is
//! the minimized set the checker's own elaborator already computes
//! internally. `all` — the fuller literal-by-literal propagation trail —
//! needs upstream checker changes and isn't implemented yet; asking for
//! it explains why rather than silently doing nothing or falling back to
//! `needed` unannounced. A pending rejection short-circuits both `all`
//! and `needed`: there's no minimized-premise set to show for a step
//! that failed, only the reason it failed, so any argument gets the same
//! rejection explanation.

use veripb_formula::prelude::*;

use crate::output::{Output, outln};
use crate::session::{RupHint, Session};

/// Whether `line` is a `rup` rule — `rup <constraint> ...;`, optionally
/// preceded by a `@label` (labels sit ahead of the rule keyword, not
/// after — see `proof_format_overview.md`'s "Constraint Labels" section).
/// An exact token match, not a prefix check, mirroring
/// `edit::is_assertion_line`.
pub(crate) fn is_rup_line(line: &str) -> bool {
    let mut tokens = line.split_whitespace();
    match tokens.next() {
        Some(t) if t.starts_with('@') => tokens.next() == Some("rup"),
        Some(t) => t == "rup",
        None => false,
    }
}

/// The display line number of the *last* `rup` step in the checked
/// prefix, if any — what bare `:why` targets. Scanned from the end,
/// unlike `edit::find_first_assertion`'s from-the-front search: an
/// assertion is scaffolding meant to be cleared oldest-first, but a `rup`
/// step is real derivation — "explain the last one" means whichever was
/// just typed. Only the checked prefix is searched: an unchecked line
/// that merely looks like a `rup` step hasn't been verified to actually
/// be one yet.
fn find_last_rup(session: &Session) -> Option<usize> {
    session.buffer[..session.checked_len]
        .iter()
        .rposition(|line| is_rup_line(line))
        .map(|idx| session.display_line(idx))
}

pub fn run(session: &Session, args: &str, out: &mut dyn Output) {
    let arg = args.trim();
    if !arg.is_empty() && arg != "all" && arg != "needed" {
        outln!(out, "Error: usage :why [all|needed]");
        return;
    }

    // A pending rejection always wins, whatever `arg` asked for: there's
    // no "last successful rup" explanation more relevant than "here's
    // why the line you're actually stuck on doesn't check" — the thing
    // `:debug`'s own doc comment promises ("why that line is bad").
    // `checked_len` still points at the rejected line itself (see
    // `Session::verify_next`'s own docs: rejection never advances it),
    // and `known_bad` is the checker's own already-computed reason —
    // nothing left to elaborate, just show it.
    if let Some(reason) = &session.known_bad {
        let display_line = session.display_line(session.checked_len);
        outln!(out, "Line {display_line} is rejected: {reason}");
        return;
    }

    if arg == "all" {
        outln!(
            out,
            "`:why all` — the full literal-by-literal propagation trail — needs upstream \
             checker changes and isn't implemented yet. `:why` (or `:why needed`) shows the \
             minimized hint list instead, which is real, already-computed data, not a \
             placeholder."
        );
        return;
    }

    let Some(display_line) = find_last_rup(session) else {
        outln!(
            out,
            "No `rup` step in the proof yet — nothing for :why to explain."
        );
        return;
    };

    let hints = match session.rup_needed_hints(display_line) {
        Ok(Ok(hints)) => hints,
        Ok(Err(msg)) => {
            outln!(out, "Error: {msg}");
            return;
        }
        Err(err) => {
            outln!(out, "Error: {err:#}");
            return;
        }
    };

    print_needed(session, display_line, &hints, out);
}

/// Print `hints` (already computed by `Session::rup_needed_hints`) under a
/// `Line <n> needed:` header, cross-referencing each hinted constraint's
/// current label(s) and pretty-printed text against the live session —
/// same lookup `:show` uses. Shared with `commands::explain`, which folds
/// this in automatically when `:explain <n>` targets a `rup` line, rather
/// than making `:why` the only way to see it.
pub(crate) fn print_needed(
    session: &Session,
    display_line: usize,
    hints: &[RupHint],
    out: &mut dyn Output,
) {
    outln!(out, "Line {display_line} needed:");
    let labels_by_id = session.labels_by_id();
    let var_names = &session.current_checker.context.var_names;
    for hint in hints {
        match hint {
            RupHint::NegatedPremise => {
                outln!(
                    out,
                    "  the negated constraint itself — no other constraint was needed"
                );
            }
            RupHint::ConstraintId(id) => {
                let labels = labels_by_id
                    .get(&(*id as isize))
                    .map(|names| format!("{} ", names.join(" ")))
                    .unwrap_or_default();
                match session
                    .current_checker
                    .database
                    .entries
                    .get(*id)
                    .and_then(Option::as_ref)
                {
                    Some(entry) => outln!(
                        out,
                        "  ConstraintId {id}: {labels}{}",
                        entry.constraint.to_pretty_string(var_names)
                    ),
                    None => outln!(out, "  ConstraintId {id} — no longer in the database"),
                }
            }
        }
    }
}
