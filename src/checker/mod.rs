//! The `veripb` subprocess boundary. Only this module invokes `veripb`;
//! everything else goes through the functions below.
//!
//! - [`types`]: result/data types crossing the boundary.
//! - [`invoke`]: spawns `veripb` and captures stdout/stderr/exit status.
//! - [`parse`]: parses `invoke`'s raw output into [`types`].
//!
//! `proof_text` arguments are the complete proof text, preamble included, so
//! reported line numbers match the REPL's display numbering.

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

/// Like [`check`], also returning the final database from the same call,
/// whether or not the proof was accepted.
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
/// `proof_text`, or `None` if it has none. `proof_text` must end at the step
/// being asked about. The inner `Err` is the checker's rejection message.
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
