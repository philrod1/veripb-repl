//! Parses `invoke`'s raw output into `types`'s shapes.

use std::path::Path;

use anyhow::Context;

use super::invoke::RawInvocation;
use super::types::{CheckOutcome, Database, DatabaseEntry, ObjectiveBounds, RupHint};

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

/// A proof ending before its closing `output`/`conclusion`/`end` lines
/// fails with a parse error containing this phrase — every line actually
/// supplied still checked out fine, so this means "wants more input,"
/// not a real rejection. Confirmed against real `veripb` output.
const RAN_OUT_OF_INPUT_MARKER: &str = "found end of file (EOF)";

/// Returns whether checking succeeded, and if not, which line it
/// stopped at.
///
/// An accepted outcome's `trace` is `raw.stdout` alone. Verified against
/// real `veripb` output: on any acceptance path — a clean exit, or the
/// "ran out of input" EOF case (see [`RAN_OUT_OF_INPUT_MARKER`]) — stdout
/// carries every genuinely useful trace line, and stderr is either empty
/// or, for the EOF case, entirely the generic "Error: Syntax error while
/// parsing proof file! ... found end of file (EOF) ..." wrapper: real
/// text veripb prints, but noise here, since every per-line check feeds
/// it a proof with no closing tail and so hits this every time. Dropping
/// stderr on acceptance avoids that block reappearing on every accepted
/// line during `:verify`.
///
/// On rejection, the line number is read from a `Checking error at
/// <path>:<line>` or `Verification error at <path>:<line>`-style message,
/// falling back to a bare `at line <N>` for a syntax error that names a
/// line without the path. Fails loudly, with the full raw output, if
/// neither shape is found.
pub fn check_outcome(raw: &RawInvocation) -> anyhow::Result<CheckOutcome> {
    if raw.success {
        return Ok(CheckOutcome::Accepted {
            trace: raw.stdout.clone(),
        });
    }

    let combined = format!("{}{}", raw.stdout, raw.stderr);

    if combined.contains(RAN_OUT_OF_INPUT_MARKER) {
        return Ok(CheckOutcome::Accepted {
            trace: raw.stdout.clone(),
        });
    }

    let line = find_path_anchored_line(&combined, &raw.proof_file_path).or_else(|| find_line_number(&combined));

    match line {
        Some(line) => Ok(CheckOutcome::Rejected {
            line,
            message: combined.clone(),
            trace: combined,
        }),
        None => anyhow::bail!(
            "veripb exited with a failure (code {:?}) but no line number could be found in \
             its output — the error format may not be recognized by this parser yet. Raw \
             output:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            raw.code,
            raw.stdout,
            raw.stderr,
        ),
    }
}

/// Returns the line number in a `"<proof_file_path>:<line>"` message —
/// the shape a `Checking error at ...`/`Verification error at ...`
/// carries. Anchored to the invocation's own proof file path so it can't
/// false-match unrelated numeric content elsewhere in the message.
fn find_path_anchored_line(text: &str, proof_file_path: &Path) -> Option<usize> {
    let marker = format!("{}:", proof_file_path.display());
    let after = text.find(marker.as_str()).map(|idx| &text[idx + marker.len()..])?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Returns the line number in a message shaped like `"... at line N
/// ..."` — the fallback for a syntax error that names a line without
/// the file's own path (unlike [`find_path_anchored_line`]'s target
/// messages).
fn find_line_number(text: &str) -> Option<usize> {
    let after = text.find("line ").map(|idx| &text[idx + "line ".len()..])?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Returns the constraint IDs a trace attributes to the *last* proof
/// line it traced — every `ConstraintId N: ...` confirmation after the
/// trace's final `line <N>: ...` marker (or, if it never printed one,
/// every occurrence in the whole trace — already scoped to one line in
/// that case, e.g. `explain_line`'s own single-line trace). Multiple ids
/// come back if that one line's rule derived more than one constraint;
/// none if it derived none (e.g. a `del` line). Only meaningful for an
/// accepted trace — a rejection's trace names no useful "last line".
pub fn last_line_constraint_ids(trace: &str) -> Vec<usize> {
    let lines: Vec<&str> = trace.lines().collect();
    let after_marker = lines
        .iter()
        .rposition(|line| is_line_marker(line))
        .map_or(0, |idx| idx + 1);
    lines[after_marker..]
        .iter()
        .filter_map(|line| constraint_id_in(line))
        .collect()
}

/// Returns whether `line` is a checker `line <N>: ...` trace marker
/// (unindented, unlike the `ConstraintId` confirmations printed under
/// it).
fn is_line_marker(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix("line") else {
        return false;
    };
    rest.trim_start().starts_with(|c: char| c.is_ascii_digit())
}

/// Returns the id in one `"  ConstraintId <N>: ..."` trace line, or
/// `None` if `line` isn't one.
fn constraint_id_in(line: &str) -> Option<usize> {
    let after = line.trim_start().strip_prefix("ConstraintId ")?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Strips a dump line's terminating `;`, plus whitespace either side of it.
fn strip_terminator<'a>(text: &'a str, line: &str) -> anyhow::Result<&'a str> {
    Ok(text
        .trim_end()
        .strip_suffix(';')
        .with_context(|| format!("malformed dump line (no terminating ';'): {line:?}"))?
        .trim_end())
}

/// The version header and footer every `--dump-database` file must match.
const EXPECTED_DATABASE_DUMP_HEADER: &str = "pseudo-Boolean database dump version 1";
const EXPECTED_DATABASE_DUMP_FOOTER: &str = "end pseudo-Boolean database dump;";

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
        header == EXPECTED_DATABASE_DUMP_HEADER,
        "unrecognized database dump header {header:?} (expected {EXPECTED_DATABASE_DUMP_HEADER:?}) — \
         the core binary's dump format may have changed since this parser was written",
    );

    let footer = lines
        .next_back()
        .context("truncated database dump — missing its end line")?;
    anyhow::ensure!(
        footer == EXPECTED_DATABASE_DUMP_FOOTER,
        "unexpected end of database dump {footer:?} (expected {EXPECTED_DATABASE_DUMP_FOOTER:?}) — \
         the core binary's dump format may have changed since this parser was written",
    );

    let entries = lines
        .map(parse_database_dump_line)
        .collect::<anyhow::Result<Vec<DatabaseEntry>>>()?;

    Ok(Database { entries })
}

/// Parses one `<id> <core|derived> <text> ;` database-dump line.
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

    let text = strip_terminator(text, line)?;

    Ok(DatabaseEntry {
        id,
        is_core,
        text: text.to_string(),
    })
}

/// The version header and footer every `--dump-objective` file must provide.
const EXPECTED_OBJECTIVE_DUMP_HEADER: &str = "pseudo-Boolean objective dump version 1";
const EXPECTED_OBJECTIVE_DUMP_FOOTER: &str = "end pseudo-Boolean objective dump;";

/// Parses a `--dump-objective` file's contents into [`ObjectiveBounds`].
/// Expected format:
///
/// ```text
/// pseudo-Boolean objective dump version 1
/// best_objective_value: <integer-or-none> ;
/// best_valid_objective_value: <integer-or-none> ;
/// end pseudo-Boolean objective dump;
/// ```
pub fn parse_objective_dump(dump: &str) -> anyhow::Result<ObjectiveBounds> {
    let mut lines = dump.lines();

    let header = lines
        .next()
        .context("empty objective dump — missing its version header line")?;
    anyhow::ensure!(
        header == EXPECTED_OBJECTIVE_DUMP_HEADER,
        "unrecognized objective dump header {header:?} (expected \
         {EXPECTED_OBJECTIVE_DUMP_HEADER:?}) — the core binary's dump format may have changed \
         since this parser was written",
    );

    let footer = lines
        .next_back()
        .context("truncated objective dump — missing its end line")?;
    anyhow::ensure!(
        footer == EXPECTED_OBJECTIVE_DUMP_FOOTER,
        "unexpected end of objective dump {footer:?} (expected {EXPECTED_OBJECTIVE_DUMP_FOOTER:?}) — \
         the core binary's dump format may have changed since this parser was written",
    );

    let best = parse_objective_dump_field(&mut lines, "best_objective_value")?;
    let best_valid = parse_objective_dump_field(&mut lines, "best_valid_objective_value")?;

    Ok(ObjectiveBounds { best, best_valid })
}

/// Parses one `<field>: <integer-or-none> ;` objective-dump line, checking
/// its field name matches `expected_field`.
fn parse_objective_dump_field(
    lines: &mut std::str::Lines<'_>,
    expected_field: &str,
) -> anyhow::Result<Option<String>> {
    let line = lines
        .next()
        .with_context(|| format!("objective dump is missing its {expected_field:?} line"))?;
    let (field, value) = line
        .split_once(": ")
        .with_context(|| format!("malformed objective dump line (no ': ' separator): {line:?}"))?;
    anyhow::ensure!(
        field == expected_field,
        "malformed objective dump line (expected field {expected_field:?}, found {field:?}): \
         {line:?}",
    );
    let value = strip_terminator(value, line)?;
    anyhow::ensure!(
        value == "none"
            || value.strip_prefix('-').unwrap_or(value).chars().all(|c| c.is_ascii_digit())
                && !value.trim_start_matches('-').is_empty(),
        "malformed objective dump line (value {value:?} is not an integer or `none`): {line:?}",
    );
    Ok((value != "none").then(|| value.to_string()))
}
