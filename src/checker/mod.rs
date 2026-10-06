//! Subprocess boundary: the only module that knows a `veripb` binary
//! exists. `session`, `commands::*`, and `tui::*` reach a formula/proof
//! exclusively through the functions below.
//!
//! - [`types`]: plain result/data types crossing the boundary.
//! - [`invoke`]: subprocess plumbing — spawns `veripb`, captures raw
//!   stdout/stderr/exit status.
//! - [`parse`]: parses `invoke`'s raw output into `types`'s shapes.
//!
//! `proof_text` arguments are the complete candidate text, preamble
//! included. Line numbers reported by these functions match this REPL's
//! own display numbering as long as that preamble is always present.

pub mod invoke;
pub mod parse;
pub mod types;

use std::path::Path;

pub use types::{CheckOutcome, Database, DatabaseEntry, ObjectiveBounds, RupHint};

/// Returns the `--trace-lines <lo>..=<hi>` argument for `trace_range`, or
/// no arguments at all for a silent check.
fn trace_args(trace_range: Option<(usize, usize)>) -> Vec<String> {
    match trace_range {
        Some((lo, hi)) => vec!["--trace-lines".to_string(), format!("{lo}..={hi}")],
        None => Vec::new(),
    }
}

/// Checks `proof_text` against `formula_path`. `trace_range`, if given,
/// is a 1-based, inclusive line range to trace (the checker's own
/// `ConstraintId N: ...` confirmations); `None` checks silently.
pub fn check(
    formula_path: &Path,
    proof_text: &str,
    trace_range: Option<(usize, usize)>,
) -> anyhow::Result<CheckOutcome> {
    let raw = invoke::run(formula_path, proof_text, trace_args(trace_range))?;
    parse::check_outcome(&raw)
}

/// Checks `proof_text` against `formula_path` and returns the resulting
/// database state from the same subprocess call. The database reflects
/// its final state whether or not the proof was fully accepted.
/// `trace_range` is as in [`check`].
pub fn check_with_database(
    formula_path: &Path,
    proof_text: &str,
    trace_range: Option<(usize, usize)>,
) -> anyhow::Result<(CheckOutcome, Database)> {
    let dump = invoke::run_with_database_dump(formula_path, proof_text, trace_args(trace_range))?;
    let outcome = parse::check_outcome(&dump.raw)?;
    let database = parse::parse_database_dump(&dump.database_dump)?;
    Ok((outcome, database))
}

/// Returns the trace for the last line of `proof_text`, scoped via
/// `--trace-lines`/`--trace-pol`.
pub fn explain_line(formula_path: &Path, proof_text: &str) -> anyhow::Result<CheckOutcome> {
    let target_line = proof_text.lines().count();
    let range = format!("{target_line}..={target_line}");
    let raw = invoke::run(
        formula_path,
        proof_text,
        ["--trace-lines", range.as_str(), "--trace-pol"],
    )?;
    parse::check_outcome(&raw)
}

/// Returns the minimized RUP-hint list for the last `rup` line in
/// `proof_text`, or `None` if that line isn't a `rup` step. `proof_text`
/// must end exactly at the step being asked about. The inner `Err` is the
/// checker's rejection message if `proof_text` doesn't check — an
/// internal error for an already-checked prefix, but the expected answer
/// when probing a candidate line (see `Session::diagnose_rejected_rup`).
pub fn elaborate_rup(
    formula_path: &Path,
    proof_text: &str,
) -> anyhow::Result<Result<Option<Vec<RupHint>>, String>> {
    let elaborated = invoke::run_with_elaboration(formula_path, proof_text)?;
    match parse::check_outcome(&elaborated.raw)? {
        CheckOutcome::Accepted { .. } => Ok(Ok(parse::last_rup_hints(&elaborated.elaborated_proof))),
        CheckOutcome::Rejected { message, .. } => Ok(Err(message)),
    }
}

/// Returns the live database after replaying `proof_text`.
pub fn show_database(formula_path: &Path, proof_text: &str) -> anyhow::Result<Database> {
    let dump = invoke::run_with_database_dump(formula_path, proof_text, std::iter::empty::<&str>())?;
    parse::parse_database_dump(&dump.database_dump)
}

/// Returns the best-known objective bounds after replaying `proof_text`.
pub fn show_objective_bounds(formula_path: &Path, proof_text: &str) -> anyhow::Result<ObjectiveBounds> {
    let dump = invoke::run_with_objective_dump(formula_path, proof_text, std::iter::empty::<&str>())?;
    parse::parse_objective_dump(&dump.objective_dump)
}
