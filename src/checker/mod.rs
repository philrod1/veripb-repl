//! The `veripb` subprocess boundary. Only this module invokes `veripb`;
//! everything else goes through the functions below.
//!
//! - [`types`]: result/data types crossing the boundary.
//! - [`invoke`]: spawns `veripb` and captures stdout/stderr/exit status.
//! - [`parse`]: parses `invoke`'s raw output into [`types`].
//! - [`progress`]: progress reports while `veripb` runs.
//!
//! `proof` arguments are the complete proof, preamble included, so reported
//! line numbers match the REPL's display numbering.

pub mod invoke;
pub mod parse;
pub mod progress;
pub mod types;

use std::path::Path;

use anyhow::Context as _;

pub use types::{CheckOutcome, Database, DatabaseEntry, ObjectiveBounds, ProofText, RupHint};

/// Returns the `--trace-lines <lo>..=<hi>[,<lo>..=<hi>...]` argument for
/// `ranges`, or no arguments at all (a silent check) if there are none.
fn trace_args(ranges: &[(usize, usize)]) -> Vec<String> {
    if ranges.is_empty() {
        return Vec::new();
    }
    let list: Vec<String> = ranges
        .iter()
        .map(|(lo, hi)| format!("{lo}..={hi}"))
        .collect();
    vec!["--trace-lines".to_string(), list.join(",")]
}

/// Checks `proof` against `formula_path`. `trace_range`, if given,
/// is a 1-based, inclusive line range to trace (the checker's own
/// `ConstraintId N: ...` confirmations); `None` checks silently.
pub fn check<'a>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
    trace_range: Option<(usize, usize)>,
) -> anyhow::Result<CheckOutcome> {
    let raw = invoke::run(formula_path, proof, trace_args(trace_range.as_slice()))?;
    parse::check_outcome(&raw)
}

/// Like [`check`], also returning the final database from the same call,
/// whether or not the proof was accepted. `trace_ranges` are the 1-based,
/// inclusive line ranges to trace; none checks silently. Each line of
/// veripb's output goes to `consume` as it is written; the lines it returns
/// `true` for are left out of the outcome's trace (see
/// [`invoke::run_streaming`]).
pub fn check_with_database<'a>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
    trace_ranges: &[(usize, usize)],
    consume: impl FnMut(&str) -> bool,
) -> anyhow::Result<(CheckOutcome, Database)> {
    let (outcome, database, _) =
        check_with_dumps(formula_path, proof, trace_ranges, false, consume)?;
    Ok((outcome, database))
}

/// Like [`check_with_database`], also returning the objective bounds from
/// the same call if `objective` is set: an `Err` if that dump failed, which
/// fails nothing else.
pub fn check_with_dumps<'a>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
    trace_ranges: &[(usize, usize)],
    objective: bool,
    consume: impl FnMut(&str) -> bool,
) -> anyhow::Result<(
    CheckOutcome,
    Database,
    Option<anyhow::Result<ObjectiveBounds>>,
)> {
    let objective_file = objective
        .then(tempfile::NamedTempFile::new)
        .transpose()
        .context("failed to create a scratch file for the objective dump")?;
    let mut args: Vec<std::ffi::OsString> = trace_args(trace_ranges)
        .into_iter()
        .map(Into::into)
        .collect();
    if let Some(file) = &objective_file {
        args.push("--dump-objective".into());
        args.push(file.path().into());
    }
    let dump = invoke::run_with_database_dump(formula_path, proof, args, consume)?;
    let outcome = parse::check_outcome(&dump.raw)?;
    let database = parse::parse_database_dump(&dump.database_dump)?;
    let bounds = objective_file.map(|file| {
        let text = std::fs::read_to_string(file.path())
            .context("failed to read back the objective dump scratch file")?;
        anyhow::ensure!(!text.is_empty(), "veripb wrote no objective dump");
        parse::parse_objective_dump(&text)
    });
    Ok((outcome, database, bounds))
}

/// Returns the trace for the last line of `proof`, scoped via
/// `--trace-lines`/`--trace-pol`.
pub fn explain_line<'a>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
) -> anyhow::Result<CheckOutcome> {
    let proof = proof.into();
    let target_line = proof.line_count();
    let range = format!("{target_line}..={target_line}");
    let raw = invoke::run(
        formula_path,
        proof,
        ["--trace-lines", range.as_str(), "--trace-pol"],
    )?;
    parse::check_outcome(&raw)
}

/// The most bytes of elaborated proof [`elaborate_rup`] lets `veripb`
/// write. An elaboration spells out the hints of every `rup` step, not just
/// the last; on a long proof that can run to terabytes.
pub const ELABORATION_LIMIT: u64 = 256 * 1024 * 1024;

/// What [`elaborate_rup`] found.
#[derive(Debug, PartialEq)]
pub enum Elaboration {
    /// The last step's minimized hints, or `None` if it isn't `rup`.
    Hints(Option<Vec<RupHint>>),
    /// The checker rejected the proof, with this message.
    Rejected(String),
    /// The elaboration passed [`ELABORATION_LIMIT`] and was stopped.
    TooLarge,
}

/// Returns the minimized RUP-hint list for the last `rup` line in
/// `proof`. `proof` must end at the step being asked about.
pub fn elaborate_rup<'a>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
) -> anyhow::Result<Elaboration> {
    elaborate_rup_within(formula_path, proof, ELABORATION_LIMIT)
}

/// [`elaborate_rup`], with an elaboration of at most `limit` bytes.
#[doc(hidden)]
pub fn elaborate_rup_within<'a>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
    limit: u64,
) -> anyhow::Result<Elaboration> {
    let Some(elaborated) = invoke::run_with_elaboration(formula_path, proof, limit)? else {
        return Ok(Elaboration::TooLarge);
    };
    Ok(match parse::check_outcome(&elaborated.raw)? {
        CheckOutcome::Accepted { .. } => {
            Elaboration::Hints(parse::last_rup_hints(&elaborated.elaborated_proof))
        }
        CheckOutcome::Rejected { message, .. } => Elaboration::Rejected(message),
    })
}

/// Returns the live database after replaying `proof`.
pub fn show_database<'a>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
) -> anyhow::Result<Database> {
    let dump =
        invoke::run_with_database_dump(formula_path, proof, std::iter::empty::<&str>(), |_| false)?;
    parse::parse_database_dump(&dump.database_dump)
}

/// Returns the best-known objective bounds after replaying `proof`.
pub fn show_objective_bounds<'a>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
) -> anyhow::Result<ObjectiveBounds> {
    let dump = invoke::run_with_objective_dump(formula_path, proof, std::iter::empty::<&str>())?;
    parse::parse_objective_dump(&dump.objective_dump)
}
