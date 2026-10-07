//! `:explain [<n>]` — shows the expanded derivation of checked line `n` (see
//! [`Session::explain_line`]), plus for `rup` the minimal needed hints
//! ([`Session::rup_needed_hints`]).
//!
//! If `n` is the pending rejected line (`Session::known_bad`), shows the
//! rejection reason and, for `rup`, [`Session::diagnose_rejected_rup`]'s
//! findings. Bare `:explain` targets the rejected line, else the last checked
//! line. Read-only: replays through a throwaway checker.

use crate::checker::RupHint;
use crate::checker::parse::is_rup_line;
use crate::commands::help;
use crate::output::{self, Output, outln};
use crate::session::{RejectionDiagnosis, Session};

// Output phrases matched by the TUI scrollback highlighter
// (`tui::draw::scrollback`); it imports these, so edit them only here.
pub(crate) const NEEDED_SUFFIX: &str = " needed:";
pub(crate) const REJECTED_INFIX: &str = " is rejected: ";
pub(crate) const TYPED_HINTS: &str = "Typed hints:";
pub(crate) const VERDICT_CHECKS: &str = "Without hints it DOES check";
pub(crate) const VERDICT_FAILS: &str = "Without hints it still fails";
pub(crate) const NEGATED_PREMISE: &str = "~ (the negated constraint itself)";

pub fn run(session: &Session, args: &str, out: &mut dyn Output) {
    let arg = args.trim();
    let rejected_line = session
        .known_bad
        .is_some()
        .then(|| session.display_line(session.checked_len));

    let n = if arg.is_empty() {
        match (rejected_line, session.checked_len) {
            (Some(n), _) => n,
            (None, 0) => {
                outln!(out, "Nothing checked yet — nothing to explain.");
                return;
            }
            (None, checked_len) => session.display_line(checked_len - 1),
        }
    } else {
        match arg.parse() {
            Ok(n) => n,
            Err(_) => {
                outln!(out, "Error: usage :explain [<n>]");
                return;
            }
        }
    };

    if rejected_line == Some(n) {
        print_rejection(session, n, out);
    } else {
        explain_checked(session, n, out);
    }
}

/// Explains checked line `n` — the per-rule trace, plus the needed hints
/// for a `rup` line.
fn explain_checked(session: &Session, n: usize, out: &mut dyn Output) {
    let explanation = match session.explain_line(n) {
        Ok(Ok(text)) => text,
        Ok(Err(msg)) => {
            outln!(out, "Error: {msg}");
            return;
        }
        Err(err) => {
            outln!(out, "Error: {err:#}");
            return;
        }
    };

    let is_rup = session
        .checked_index(n)
        .is_some_and(|idx| is_rup_line(&session.buffer[idx]));

    if explanation.trim().is_empty() && !is_rup {
        outln!(
            out,
            "(no trace output for line {n} — nothing more to show beyond :list.)"
        );
        return;
    }
    if !explanation.trim().is_empty() {
        output::text(out, &explanation);
    }

    if is_rup {
        match session.rup_needed_hints(n) {
            Ok(Ok(hints)) => {
                outln!(out, "Line {n}{NEEDED_SUFFIX}");
                print_hint_list(session, &hints, "  ", out);
            }
            Ok(Err(msg)) => outln!(out, "Error: {msg}"),
            Err(err) => outln!(out, "Error: {err:#}"),
        }
    }
}

/// Prints the rejection reason (`known_bad`) for line `n`, then
/// [`Session::diagnose_rejected_rup`]'s findings for a `rup` line.
fn print_rejection(session: &Session, n: usize, out: &mut dyn Output) {
    let reason = session.known_bad.as_deref().unwrap_or_default();
    outln!(out, "Line {n}{REJECTED_INFIX}{reason}");
    help::print_rejection_hint(out, &session.buffer[session.checked_len]);

    match session.diagnose_rejected_rup() {
        Ok(Some(diagnosis)) => print_diagnosis(session, &diagnosis, out),
        Ok(None) => {}
        Err(err) => outln!(out, "Error: {err:#}"),
    }
}

fn print_diagnosis(session: &Session, diagnosis: &RejectionDiagnosis, out: &mut dyn Output) {
    if diagnosis.typed_hints.is_empty() {
        outln!(out, "  {TYPED_HINTS} (none)");
    } else {
        let typed: Vec<String> = diagnosis
            .typed_hints
            .iter()
            .map(|hint| match hint {
                RupHint::ConstraintId(id) => id.to_string(),
                RupHint::NegatedPremise => "~".to_string(),
            })
            .collect();
        outln!(out, "  {TYPED_HINTS} {}", typed.join(" "));
    }
    for id in &diagnosis.missing_ids {
        outln!(
            out,
            "  ConstraintId {id} is not in the database (deleted or never derived)"
        );
    }
    match &diagnosis.without_hints {
        Ok(hints) if diagnosis.typed_hints.is_empty() => {
            // Unreachable in practice (the bare line is the rejected line);
            // report the checker's result anyway.
            outln!(out, "  Re-checking it found these hints:");
            print_hint_list(session, hints, "    ", out);
        }
        Ok(hints) => {
            outln!(
                out,
                "  {VERDICT_CHECKS} — the hint list is the problem. The checker needed:"
            );
            print_hint_list(session, hints, "    ", out);
        }
        Err(_) => outln!(
            out,
            "  {VERDICT_FAILS}: the constraint isn't RUP-implied by the current database."
        ),
    }
}

/// Prints each hint on its own `indent`ed line with its constraint's current
/// labels and text. A lone `~` is reported as "no other constraint needed".
fn print_hint_list(session: &Session, hints: &[RupHint], indent: &str, out: &mut dyn Output) {
    match hints {
        [] => {
            outln!(
                out,
                "{indent}(no hints — the constraint is trivially implied)"
            );
            return;
        }
        [RupHint::NegatedPremise] => {
            outln!(
                out,
                "{indent}the negated constraint itself — no other constraint was needed"
            );
            return;
        }
        _ => {}
    }

    let labels_by_id = session.labels_by_id();
    let database = match session.database() {
        Ok(database) => database,
        Err(err) => {
            outln!(out, "Error: {err:#}");
            return;
        }
    };
    for hint in hints {
        match hint {
            RupHint::NegatedPremise => {
                outln!(out, "{indent}{NEGATED_PREMISE}");
            }
            RupHint::ConstraintId(id) => {
                let labels = labels_by_id
                    .get(&(*id as isize))
                    .map(|names| format!("{} ", names.join(" ")))
                    .unwrap_or_default();
                match database.get(*id) {
                    Some(entry) => {
                        outln!(out, "{indent}ConstraintId {id}: {labels}{}", entry.text)
                    }
                    None => outln!(out, "{indent}ConstraintId {id} — no longer in the database"),
                }
            }
        }
    }
}
