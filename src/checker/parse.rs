//! Parses `invoke`'s raw output into `types`'s shapes.

use std::path::Path;

use anyhow::Context;

use super::invoke::RawInvocation;
use super::types::{CheckOutcome, Database, DatabaseEntry, ObjectiveBounds, RupHint};

/// Returns the minimized hint list from the last `rup` step in an
/// elaborated proof, or `None` if the text contains no `rup` line. The proof
/// must end at the line being asked about.
pub fn last_rup_hints(elaborated_proof: &str) -> Option<Vec<RupHint>> {
    let last_rup_line = elaborated_proof.lines().rev().find(|line| is_rup_line(line))?;
    split_rup_hints(last_rup_line).1
}

/// Splits a `rup` line into its trimmed prefix (label, keyword, constraint)
/// and the hints after `:`. With no `:`, returns `None` hints and strips the
/// prefix's `;`. Unparsable hint tokens are dropped. Doesn't check that
/// `line` is a `rup` line; see [`is_rup_line`].
pub fn split_rup_hints(line: &str) -> (&str, Option<Vec<RupHint>>) {
    let Some((prefix, hints)) = line.split_once(':') else {
        let prefix = line.trim();
        return (prefix.strip_suffix(';').unwrap_or(prefix).trim_end(), None);
    };
    let hints = hints.trim();
    let hints = hints.strip_suffix(';').unwrap_or(hints);
    let hints = hints
        .split_whitespace()
        .filter_map(|tok| {
            if tok == "~" {
                Some(RupHint::NegatedPremise)
            } else {
                tok.parse::<usize>().ok().map(RupHint::ConstraintId)
            }
        })
        .collect();
    (prefix.trim(), Some(hints))
}

/// Returns a proof line's rule keyword — its first token, or its second
/// if the first is an `@label`.
pub fn rule_keyword(line: &str) -> Option<&str> {
    let mut tokens = line.split_whitespace();
    match tokens.next()? {
        t if t.starts_with('@') => tokens.next(),
        t => Some(t),
    }
}

/// Returns whether `line`'s rule keyword (after an optional `@label`) is
/// exactly `rup`.
pub fn is_rup_line(line: &str) -> bool {
    rule_keyword(line) == Some("rup")
}

/// Returns whether `line` is a `preserved_add` or `preserved_rm` rule (the
/// only rules that change the preserved set).
pub fn is_preserved_change_line(line: &str) -> bool {
    matches!(rule_keyword(line), Some("preserved_add" | "preserved_rm"))
}

/// Prefix of the line listing the preserved set, which veripb prints when
/// tracing a `preserved_add`/`preserved_rm` step.
const PRESERVED_SET_TRACE_PREFIX: &str = "Preserved set:";

/// Returns the preserved set from the last `Preserved set: <names>` line in
/// a trace, naturally sorted (veripb prints hash order), or
/// `None` if there is none. ANSI colour codes are stripped first.
///
/// TODO: replace with veripb `--dump-preserved` once it exists; this trace
/// line is not a stable format.
pub fn last_preserved_set(trace: &str) -> Option<Vec<String>> {
    trace.lines().rev().find_map(|line| {
        let line = strip_ansi(line);
        let names = line.trim().strip_prefix(PRESERVED_SET_TRACE_PREFIX)?;
        Some(sort_var_names(names.split_whitespace()))
    })
}

/// Returns the variable names in a formula's `preserved: x1 x3 ;` line,
/// naturally sorted (`x2` before `x10`).
pub fn preserved_declaration(line: &str) -> Vec<String> {
    let names = line.trim().strip_prefix("preserved:").unwrap_or(line);
    let names = names.trim().strip_suffix(';').unwrap_or(names);
    sort_var_names(names.split_whitespace())
}

/// Sorts variable names by stem, then trailing number (`x2` before `x10`).
fn sort_var_names<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut names: Vec<String> = names.map(str::to_string).collect();
    names.sort_by_key(|name| {
        let stem = name.trim_end_matches(|c: char| c.is_ascii_digit());
        let number: Option<u128> = name[stem.len()..].parse().ok();
        (stem.to_string(), number, name.clone())
    });
    names
}

/// Removes `ESC [ ... <letter>` terminal escape sequences from `text`.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// veripb exits non-zero with this phrase for a proof with no closing
/// `output`/`conclusion`/`end` lines; every supplied line was accepted, so
/// treat it as accepted.
const RAN_OUT_OF_INPUT_MARKER: &str = "found end of file (EOF)";

/// Classifies a `veripb` run as accepted or rejected at a line.
///
/// Accepted on a clean exit or `RAN_OUT_OF_INPUT_MARKER`; `trace` is then
/// stdout only (on acceptance stderr holds only the EOF syntax-error
/// wrapper).
///
/// On rejection the line comes from `<proof path>:<line>` (as in `Checking
/// error at ...`/`Verification error at ...`), else from `line <N>`. Errors
/// with the full raw output if neither is found.
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
            message: rejection_reason(&combined),
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

/// Returns the one-line reason from a rejection's raw output: the first
/// non-blank line after `Caused by:`, else the text after `Error: `, else
/// the whole output trimmed.
pub fn rejection_reason(output: &str) -> String {
    let mut lines = output.lines();
    if lines.by_ref().any(|line| line.trim() == "Caused by:")
        && let Some(reason) = lines.map(str::trim).find(|line| !line.is_empty())
    {
        return reason.to_string();
    }
    output
        .lines()
        .find_map(|line| line.strip_prefix("Error: "))
        .unwrap_or(output)
        .trim()
        .to_string()
}

/// Returns the line number after `"<proof_file_path>:"` in `text`. Anchored
/// to the path to avoid matching unrelated numbers.
fn find_path_anchored_line(text: &str, proof_file_path: &Path) -> Option<usize> {
    let marker = format!("{}:", proof_file_path.display());
    let after = text.find(marker.as_str()).map(|idx| &text[idx + marker.len()..])?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Returns the number after the first `line ` in `text` (syntax errors that
/// omit the path).
fn find_line_number(text: &str) -> Option<usize> {
    let after = text.find("line ").map(|idx| &text[idx + "line ".len()..])?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Returns the IDs of every `ConstraintId N: ...` line after the trace's
/// last `line <N>:` marker (or in the whole trace if there is none): the
/// constraints the last traced line derived, possibly none. Only meaningful
/// for an accepted trace.
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

/// Returns whether `line` is a `line <N>: ...` trace marker.
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

/// Parses a `--dump-database` file's contents into a [`Database`]. Errors on
/// a bad header/footer or any malformed entry; never skips entries.
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
/// objective: min <coeff> <lit> ... <constant> | none ;
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

    let objective = parse_objective_dump_field(&mut lines, "objective", ValueShape::Any)?;
    let best = parse_objective_dump_field(&mut lines, "best_objective_value", ValueShape::Integer)?;
    let best_valid =
        parse_objective_dump_field(&mut lines, "best_valid_objective_value", ValueShape::Integer)?;

    Ok(ObjectiveBounds {
        objective,
        best,
        best_valid,
    })
}

/// What a [`parse_objective_dump_field`] value is allowed to look like,
/// beyond the universal `none` sentinel.
enum ValueShape {
    /// An arbitrary-precision integer (optionally `-`-prefixed).
    Integer,
    /// Any non-empty text, kept opaque.
    Any,
}

/// Parses one `<field>: <value> ;` objective-dump line, checking its field
/// name matches `expected_field` and its value matches `shape`.
fn parse_objective_dump_field(
    lines: &mut std::str::Lines<'_>,
    expected_field: &str,
    shape: ValueShape,
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
    match shape {
        ValueShape::Integer => anyhow::ensure!(
            value == "none"
                || value.strip_prefix('-').unwrap_or(value).chars().all(|c| c.is_ascii_digit())
                    && !value.trim_start_matches('-').is_empty(),
            "malformed objective dump line (value {value:?} is not an integer or `none`): {line:?}",
        ),
        ValueShape::Any => anyhow::ensure!(
            value == "none" || !value.is_empty(),
            "malformed objective dump line (empty value): {line:?}",
        ),
    }
    Ok((value != "none").then(|| value.to_string()))
}
