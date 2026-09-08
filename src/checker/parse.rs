//! Parses `invoke`'s raw output into `types`'s shapes.

use anyhow::Context;

use super::invoke::RawInvocation;
use super::types::{CheckOutcome, Database, DatabaseEntry, RupHint};

/// Returns the minimized hint list from the last `rup` step in an
/// elaborated proof, or `None` if the text contains no `rup` line.
///
/// Callers must elaborate a proof truncated to end exactly at the line
/// being asked about, so the last `rup` line in the output is
/// unambiguously the one being asked about.
pub fn last_rup_hints(elaborated_proof: &str) -> Option<Vec<RupHint>> {
    let last_rup_line = elaborated_proof.lines().rev().find(|line| is_rup_line(line))?;
    let (_constraint, hints) = last_rup_line.split_once(':')?;
    let hints = hints.trim();
    let hints = hints.strip_suffix(';').unwrap_or(hints);
    Some(
        hints
            .split_whitespace()
            .filter_map(|tok| {
                if tok == "~" {
                    Some(RupHint::NegatedPremise)
                } else {
                    // An unparsable token is dropped rather than aborting
                    tok.parse::<usize>().ok().map(RupHint::ConstraintId)
                }
            })
            .collect(),
    )
}

/// Returns whether `line` is a `rup` rule line, optionally preceded by
/// an `@label`.
fn is_rup_line(line: &str) -> bool {
    let mut tokens = line.split_whitespace();
    match tokens.next() {
        Some(t) if t.starts_with('@') => tokens.next() == Some("rup"),
        Some(t) => t == "rup",
        None => false,
    }
}

/// Returns whether checking succeeded, and if not, which line it
/// stopped at.
///
/// This heuristic — scanning combined stdout/stderr for a `line N`
/// substring — has not been verified against a real `veripb` run. Fails
/// loudly, with the full raw output, rather than guessing when no line
/// number can be found.
pub fn check_outcome(raw: &RawInvocation) -> anyhow::Result<CheckOutcome> {
    let combined = format!("{}{}", raw.stdout, raw.stderr);

    if raw.success {
        return Ok(CheckOutcome::Accepted { trace: combined });
    }

    match find_line_number(&combined) {
        Some(line) => Ok(CheckOutcome::Rejected {
            line,
            message: combined.clone(),
            trace: combined,
        }),
        None => anyhow::bail!(
            "veripb exited with a failure (code {:?}) but this parser couldn't find a `line \
             N` it could point to — this is exactly the unverified case this module's own \
             docs warn about (possibly \"ran out of input\", not a real rejection, or possibly \
             a real error in a format this parser doesn't recognize yet). Raw output:\n\
             --- stdout ---\n{}\n--- stderr ---\n{}",
            raw.code,
            raw.stdout,
            raw.stderr,
        ),
    }
}

/// Returns the first line number found in `text`, matched as `"line "`
/// followed by digits.
fn find_line_number(text: &str) -> Option<usize> {
    let after = text.find("line ").map(|idx| &text[idx + "line ".len()..])?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// The version header every `--dump-database` file must start with.
const EXPECTED_DUMP_HEADER: &str = "pseudo-Boolean database dump version 1";

/// Parses a `--dump-database` file's contents into a [`Database`].
///
/// Fails on any unrecognized header or malformed entry line, rather than
/// skipping it — a dropped entry would misreport the database's actual
/// contents.
pub fn parse_database_dump(dump: &str) -> anyhow::Result<Database> {
    let mut lines = dump.lines();

    let header = lines
        .next()
        .context("empty database dump — missing its version header line")?;
    anyhow::ensure!(
        header == EXPECTED_DUMP_HEADER,
        "unrecognized database dump header {header:?} (expected {EXPECTED_DUMP_HEADER:?}) — \
         the core binary's dump format may have changed since this parser was written",
    );

    let entries = lines
        .map(parse_database_dump_line)
        .collect::<anyhow::Result<Vec<DatabaseEntry>>>()?;

    Ok(Database { entries })
}

/// Parses one `<id> <core|derived> <text>` database-dump line.
fn parse_database_dump_line(line: &str) -> anyhow::Result<DatabaseEntry> {
    let (id, rest) = line
        .split_once(' ')
        .with_context(|| format!("malformed database dump line (no id): {line:?}"))?;
    let id: usize = id
        .parse()
        .with_context(|| format!("malformed database dump line (bad id {id:?}): {line:?}"))?;

    let (tag, text) = rest
        .split_once(' ')
        .with_context(|| format!("malformed database dump line (no core/derived tag): {line:?}"))?;
    let is_core = match tag {
        "core" => true,
        "derived" => false,
        _ => anyhow::bail!("malformed database dump line (unknown tag {tag:?}): {line:?}"),
    };

    Ok(DatabaseEntry {
        id,
        is_core,
        text: text.to_string(),
    })
}
