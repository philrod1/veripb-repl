//! `:explain [<n>]` — explains checked line `n`: in plain English for the
//! rules [`teach::explain`] covers, then, under `Checker:`, its expanded
//! derivation (see [`Session::explain_line`]), plus for `rup` the minimal
//! needed hints ([`Session::rup_needed_hints`]); when veripb's account of
//! them passes [`checker::ELABORATION_LIMIT`], the hints of the replay the
//! plain-English steps show ([`teach::explain::rup_hints`]).
//!
//! If `n` is the pending rejected line (`Session::known_bad`), shows the
//! rejection reason and, for `rup`, [`Session::diagnose_rejected_rup`]'s
//! findings. Bare `:explain` targets the rejected line, else the last checked
//! line. Read-only: replays through a throwaway checker.

use crate::checker::parse::{is_rup_line, pol_table_with_typed_operands, rule_keyword};
use crate::checker::{self, RupHint};
use crate::commands::help;
use crate::output::{self, Output, outln};
use crate::session::{RejectionDiagnosis, Session};
use crate::teach;

// Output phrases matched by the TUI scrollback highlighter
// (`tui::draw::scrollback`); it imports these, so edit them only here.
pub(crate) const NEEDED_SUFFIX: &str = " needed:";
pub(crate) const REJECTED_INFIX: &str = " is rejected: ";
pub(crate) const TYPED_HINTS: &str = "Typed hints:";
pub(crate) const VERDICT_CHECKS: &str = "Without hints it DOES check";
pub(crate) const VERDICT_FAILS: &str = "Without hints it still fails";
pub(crate) const NEGATED_PREMISE: &str = "~ (the negated constraint itself)";
/// Says that veripb's account of `hints` (the hints a `rup` line needs)
/// passed [`checker::ELABORATION_LIMIT`].
fn past_the_limit(hints: &str) -> String {
    format!(
        "veripb's account of {hints} for a proof this long passed the {} MiB limit",
        checker::ELABORATION_LIMIT / (1024 * 1024)
    )
}

/// Said when the hints aren't shown, the limit having been passed.
fn too_large_to_elaborate() -> String {
    format!(
        "the hints it needed aren't shown: {}",
        past_the_limit("them")
    )
}

/// Heads the checker's account after a plain-English explanation.
pub(crate) const CHECKER_HEADING: &str = "Checker:";

pub fn run(session: &Session, args: &str, out: &mut dyn Output) {
    match target(session, args, out) {
        Some((n, true)) => print_rejection(session, n, out),
        Some((n, false)) => explain_checked(session, n, out),
        None => {}
    }
}

/// The display line `:explain <args>` explains, and whether it is the
/// pending rejected line; `None`, saying why, if there is none.
pub(crate) fn target(session: &Session, args: &str, out: &mut dyn Output) -> Option<(usize, bool)> {
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
                return None;
            }
            (None, checked_len) => session.display_line(checked_len - 1),
        }
    } else {
        match arg.parse() {
            Ok(n) => n,
            Err(_) => {
                outln!(out, "Error: usage :explain [<n>]");
                return None;
            }
        }
    };
    Some((n, rejected_line == Some(n)))
}

/// Explains checked line `n`: in plain English when [`teach::explain`]
/// covers it, then the checker's account (see [`print_checker`]).
fn explain_checked(session: &Session, n: usize, out: &mut dyn Output) {
    let explanation = teach::explain::explain(session, n);
    if let Some(explanation) = &explanation {
        for line in explanation.text_lines() {
            outln!(out, "{line}");
        }
        outln!(out);
        outln!(out, "{CHECKER_HEADING}");
    }
    let replay_hints = explanation.and_then(|e| e.replay_hints);
    print_checker(session, n, replay_hints.as_deref(), out);
}

/// The checker's account of checked line `n`: its trace, and for `rup` the
/// hints it needed, or `replay_hints` (the plain-English replay's) when
/// veripb's account of them passes the limit.
pub(crate) fn print_checker(
    session: &Session,
    n: usize,
    replay_hints: Option<&[RupHint]>,
    out: &mut dyn Output,
) {
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
    // Show a `pol` table's operands as typed (labels rather than IDs).
    let explanation = match session.checked_index(n) {
        Some(idx) if rule_keyword(&session.buffer[idx]) == Some("pol") => {
            pol_table_with_typed_operands(&explanation, &session.buffer[idx])
        }
        _ => explanation,
    };
    if !explanation.trim().is_empty() {
        output::text(out, &explanation);
    }

    if is_rup {
        match session.rup_needed_hints(n) {
            Ok(Ok(Some(hints))) => {
                outln!(out, "Line {n}{NEEDED_SUFFIX}");
                print_hint_list(session, &hints, "  ", out);
            }
            Ok(Ok(None)) => match replay_hints {
                Some(hints) => {
                    outln!(
                        out,
                        "(The hints below are the ones the steps above used: {}.)",
                        past_the_limit("the hints")
                    );
                    outln!(out, "Line {n}{NEEDED_SUFFIX}");
                    print_hint_list(session, hints, "  ", out);
                }
                None => outln!(out, "({})", too_large_to_elaborate()),
            },
            Ok(Err(msg)) => outln!(out, "Error: {msg}"),
            Err(err) => outln!(out, "Error: {err:#}"),
        }
    }
}

/// Prints the rejection reason (`known_bad`) for line `n`, then
/// [`Session::diagnose_rejected_rup`]'s findings for a `rup` line.
pub(crate) fn print_rejection(session: &Session, n: usize, out: &mut dyn Output) {
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
                RupHint::Label(name) => name.clone(),
                RupHint::NegatedPremise => "~".to_string(),
            })
            .collect();
        outln!(out, "  {TYPED_HINTS} {}", typed.join(" "));
    }
    for name in &diagnosis.unknown_labels {
        outln!(out, "  {name} is not a defined label");
    }
    for id in &diagnosis.missing_ids {
        outln!(
            out,
            "  ConstraintId {id} is not in the database (deleted or never derived)"
        );
    }
    // Re-checking without hints: veripb's result, else unit propagation's.
    let (without_hints, needed_by) = match &diagnosis.without_hints {
        Some(result) => (Some(result.clone().map_err(drop)), "The checker needed"),
        None => {
            let replayed = replay_rejected(session);
            if replayed.is_some() {
                outln!(
                    out,
                    "  (This is from replaying unit propagation: {}.)",
                    past_the_limit("the hints")
                );
            }
            (replayed, "Unit propagation used")
        }
    };
    match &without_hints {
        None => outln!(out, "  ({})", too_large_to_elaborate()),
        Some(Ok(hints)) if diagnosis.typed_hints.is_empty() => {
            // Unreachable in practice (the bare line is the rejected line);
            // report the checker's result anyway.
            outln!(out, "  Re-checking it found these hints:");
            print_hint_list(session, hints, "    ", out);
        }
        Some(Ok(hints)) => {
            outln!(
                out,
                "  {VERDICT_CHECKS} — the hint list is the problem. {needed_by}:"
            );
            print_hint_list(session, hints, "    ", out);
        }
        Some(Err(_)) => outln!(
            out,
            "  {VERDICT_FAILS}: the constraint isn't RUP-implied by the current database."
        ),
    }
}

/// Unit propagation's result for the rejected `rup` line without its typed
/// hints, over the current database: `Ok` with the hints it used, `Err` if
/// it reaches no conflict, `None` if it gives no answer.
fn replay_rejected(session: &Session) -> Option<Result<Vec<RupHint>, ()>> {
    let database = session.database().ok()?;
    match teach::explain::rup_hints(&session.buffer[session.checked_len], &database) {
        teach::explain::RupReplay::Hints(hints) => Some(Ok(hints)),
        teach::explain::RupReplay::NotImplied => Some(Err(())),
        teach::explain::RupReplay::Unknown => None,
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
            RupHint::Label(name) => outln!(out, "{indent}{name}"),
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
