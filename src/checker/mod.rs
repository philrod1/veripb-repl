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

pub use types::{CheckOutcome, Database, DatabaseEntry, RupHint};

/// Checks `proof_text` against `formula_path`.
pub fn check(formula_path: &Path, proof_text: &str) -> anyhow::Result<CheckOutcome> {
    let raw = invoke::run(formula_path, proof_text, std::iter::empty::<&str>())?;
    parse::check_outcome(&raw)
}

/// Checks `proof_text` against `formula_path` and returns the resulting
/// database state from the same subprocess call. The database reflects
/// its final state whether or not the proof was fully accepted.
pub fn check_with_database(formula_path: &Path, proof_text: &str) -> anyhow::Result<(CheckOutcome, Database)> {
    let dump = invoke::run_with_database_dump(formula_path, proof_text)?;
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
/// must end exactly at the step being asked about.
pub fn why_rup(formula_path: &Path, proof_text: &str) -> anyhow::Result<Option<Vec<RupHint>>> {
    let elaborated = invoke::run_with_elaboration(formula_path, proof_text)?;
    anyhow::ensure!(
        elaborated.raw.success,
        "internal error: elaborating a proof that should already be fully checked failed \
         (exit code {:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        elaborated.raw.code,
        elaborated.raw.stdout,
        elaborated.raw.stderr,
    );
    Ok(parse::last_rup_hints(&elaborated.elaborated_proof))
}

/// Returns the live database after replaying `proof_text`.
pub fn show_database(formula_path: &Path, proof_text: &str) -> anyhow::Result<Database> {
    let dump = invoke::run_with_database_dump(formula_path, proof_text)?;
    parse::parse_database_dump(&dump.database_dump)
}
