//! Parses `invoke`'s raw output into `types`'s shapes.

use std::path::Path;

use anyhow::Context;

use super::invoke::RawInvocation;
use super::types::{CheckOutcome, Database, DatabaseEntry, ObjectiveBounds, RupHint};

/// Returns the minimized hint list from the last `rup` step in an
/// elaborated proof, or `None` if the text contains no `rup` line. The proof
/// must end at the line being asked about.
pub fn last_rup_hints(elaborated_proof: &str) -> Option<Vec<RupHint>> {
    let last_rup_line = elaborated_proof
        .lines()
        .rev()
        .find(|line| is_rup_line(line))?;
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
            } else if tok.starts_with('@') {
                Some(RupHint::Label(tok.to_string()))
            } else {
                tok.parse::<usize>().ok().map(RupHint::ConstraintId)
            }
        })
        .collect();
    (prefix.trim(), Some(hints))
}

/// Returns a proof line's rule keyword: its first token after any
/// leading `@label`s.
pub fn rule_keyword(line: &str) -> Option<&str> {
    line.split_whitespace().find(|t| !t.starts_with('@'))
}

/// Returns the `@label`s at the start of a proof or formula line, in order.
pub fn leading_labels(line: &str) -> Vec<&str> {
    line.split_whitespace().take_while(|t| t.starts_with('@')).collect()
}

/// Returns how many constraints veripb loads for one formula constraint
/// line: 2 for an equivalence (`<==>`) or an equality (`=`, also behind
/// `==>`), else 1. (`<==` or `<==>` with `=` is rejected by veripb.)
pub fn formula_line_constraints(line: &str) -> usize {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.contains(&"<==>") || tokens.contains(&"=") {
        2
    } else {
        1
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
/// error at ...`/`Verification error at ...`), else from `line <N>`, searched
/// in stderr then stdout, ignoring trace markers. Errors with the full raw
/// output if neither is found.
pub fn check_outcome(raw: &RawInvocation) -> anyhow::Result<CheckOutcome> {
    if raw.success {
        return Ok(CheckOutcome::Accepted {
            trace: raw.stdout.clone(),
        });
    }

    if let Some(message) = formula_error(raw) {
        anyhow::bail!(message);
    }

    let combined = format!("{}{}", raw.stdout, raw.stderr);

    if combined.contains(RAN_OUT_OF_INPUT_MARKER) {
        return Ok(CheckOutcome::Accepted {
            trace: raw.stdout.clone(),
        });
    }

    let line = find_path_anchored_line(&combined, &raw.proof_file_path)
        .or_else(|| find_line_number(&raw.stderr))
        .or_else(|| find_line_number(&raw.stdout));

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

/// Returns a user-facing message if `raw` failed because of an error in
/// the formula file (veripb's message names `raw.formula_file_path`, as
/// `<path>:<line>:<column>`), with that location rewritten as
/// `line L, column C`. `None` otherwise.
pub fn formula_error(raw: &RawInvocation) -> Option<String> {
    if raw.success {
        return None;
    }
    let combined = format!("{}{}", raw.stdout, raw.stderr);
    let path = raw.formula_file_path.display().to_string();
    let error_line = combined.lines().find(|line| line.contains(&path))?;

    let after_path = &error_line[error_line.find(&path)? + path.len()..];
    let mut location = after_path.strip_prefix(':').unwrap_or("").splitn(3, ':');
    let digits = |s: Option<&str>| -> Option<String> {
        let d: String = s?.chars().take_while(char::is_ascii_digit).collect();
        (!d.is_empty()).then_some(d)
    };
    let replaced = match (digits(location.next()), digits(location.next())) {
        (Some(line), Some(column)) => error_line.replacen(
            &format!("{path}:{line}:{column}"),
            &format!("line {line}, column {column}"),
            1,
        ),
        (Some(line), None) => {
            error_line.replacen(&format!("{path}:{line}"), &format!("line {line}"), 1)
        }
        _ => error_line.replacen(&path, "the formula", 1),
    };
    let message = replaced.trim();
    let message = message.strip_prefix("Error: ").unwrap_or(message);
    Some(format!("the formula has an error: {message}"))
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
    let after = text
        .find(marker.as_str())
        .map(|idx| &text[idx + marker.len()..])?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Returns the number after the first `line ` followed by a digit in
/// `text` (syntax errors that omit the path), skipping `line <N>:` trace
/// markers.
fn find_line_number(text: &str) -> Option<usize> {
    text.lines()
        .filter(|line| line_marker_number(line).is_none())
        .flat_map(|line| {
            line.match_indices("line ")
                .map(move |(idx, _)| &line[idx + "line ".len()..])
        })
        .find_map(|after| {
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().ok()
        })
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

/// Returns, for each `line <N>:` marker in a trace, the IDs of the
/// `ConstraintId` lines that follow it before the next marker: what each
/// traced proof line derived, by its line number in the traced proof.
pub fn constraint_ids_by_line(trace: &str) -> std::collections::BTreeMap<usize, Vec<usize>> {
    let mut parser = ConstraintIdsByLine::new(|_| true);
    for line in trace.lines() {
        parser.feed(line);
    }
    parser.finish()
}

/// Builds [`constraint_ids_by_line`]'s map one trace line at a time, for a
/// trace too large to hold in memory, keeping only the proof lines `wanted`
/// accepts.
pub struct ConstraintIdsByLine<F> {
    wanted: F,
    /// The proof line of the latest marker, and whether it is wanted.
    current: Option<(usize, bool)>,
    by_line: std::collections::BTreeMap<usize, Vec<usize>>,
}

impl<F: Fn(usize) -> bool> ConstraintIdsByLine<F> {
    pub fn new(wanted: F) -> Self {
        Self {
            wanted,
            current: None,
            by_line: std::collections::BTreeMap::new(),
        }
    }

    /// Takes one line of veripb's output (without its newline). Returns
    /// whether it was trace: a `line <N>:` marker, or a `ConstraintId` line
    /// after one. Other lines are not trace and are ignored.
    pub fn feed(&mut self, line: &str) -> bool {
        if let Some(n) = line_marker_number(line) {
            let wanted = (self.wanted)(n);
            if wanted {
                self.by_line.entry(n).or_default();
            }
            self.current = Some((n, wanted));
            return true;
        }
        match (self.current, constraint_id_in(line)) {
            (Some((n, wanted)), Some(id)) => {
                if wanted {
                    self.by_line.entry(n).or_default().push(id);
                }
                true
            }
            _ => false,
        }
    }

    /// The proof line of the latest `line <N>:` marker fed, if any.
    pub fn current_line(&self) -> Option<usize> {
        self.current.map(|(n, _)| n)
    }

    /// What each wanted proof line derived.
    pub fn finish(self) -> std::collections::BTreeMap<usize, Vec<usize>> {
        self.by_line
    }
}

/// Rewrites the operand column of a traced `pol` table (`depth  operand
/// top of stack`) to show `pol_line`'s operands as typed, e.g. `@label`
/// where veripb prints the constraint ID. The table has one row per token
/// of the `pol` expression; if the counts differ, `trace` is returned
/// unchanged. Columns are re-aligned to the new widths.
pub fn pol_table_with_typed_operands(trace: &str, pol_line: &str) -> String {
    let Some(expr) = pol_line
        .split_once(" pol ")
        .map(|(_, e)| e)
        .or_else(|| pol_line.trim_start().strip_prefix("pol "))
    else {
        return trace.to_string();
    };
    let expr = expr.trim().trim_end_matches(';');
    let tokens: Vec<&str> = expr
        .split_whitespace()
        .map(|t| t.trim_end_matches(';'))
        .filter(|t| !t.is_empty())
        .collect();

    let lines: Vec<&str> = trace.lines().collect();
    let Some(header) = lines.iter().position(|l| {
        let t = l.trim();
        t.starts_with("depth") && t.contains("operand") && t.contains("top of stack")
    }) else {
        return trace.to_string();
    };
    // Rows: `<depth> <operand> <top of stack>`, until the first other line.
    let rows: Vec<(&str, &str)> = lines[header + 1..]
        .iter()
        .map_while(|l| {
            let mut parts = l.split_whitespace();
            let depth = parts.next()?;
            depth.parse::<usize>().ok()?;
            parts.next()?;
            let rest = l.trim_start()[depth.len()..].trim_start();
            let operand_len = rest.split_whitespace().next()?.len();
            Some((depth, rest[operand_len..].trim_start()))
        })
        .collect();
    if rows.len() != tokens.len() {
        return trace.to_string();
    }

    let indent = &lines[header][..lines[header].len() - lines[header].trim_start().len()];
    let depth_w = rows.iter().map(|(d, _)| d.len()).max().unwrap_or(0).max("depth".len()) + 2;
    let operand_w = tokens.iter().map(|t| t.len()).max().unwrap_or(0).max("operand".len()) + 2;
    let mut out: Vec<String> = lines[..header].iter().map(|l| l.to_string()).collect();
    out.push(format!("{indent}{:<depth_w$}{:<operand_w$}top of stack", "depth", "operand"));
    for ((depth, top), token) in rows.iter().zip(&tokens) {
        out.push(format!("{indent}{depth:<depth_w$}{token:<operand_w$}{top}"));
    }
    out.extend(lines[header + 1 + rows.len()..].iter().map(|l| l.to_string()));
    let mut text = out.join("\n");
    if trace.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Appends each constraint's labels after every `ID <N>` in `text` (as in
/// veripb's "falsifies the constraint with ID 3"), e.g. `ID 3 (@no_34)`.
/// IDs without labels are left as they are.
pub fn annotate_constraint_ids(
    text: &str,
    labels_by_id: &ahash::AHashMap<isize, Vec<String>>,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("ID ") {
        let after = &rest[pos + 3..];
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        out.push_str(&rest[..pos + 3 + digits.len()]);
        rest = &after[digits.len()..];
        if let Some(names) = digits.parse::<isize>().ok().and_then(|id| labels_by_id.get(&id))
        {
            out.push_str(&format!(" ({})", names.join(" ")));
        }
    }
    out.push_str(rest);
    out
}

/// Returns `N` from a `line <N>: ...` trace marker.
fn line_marker_number(line: &str) -> Option<usize> {
    let rest = line.trim_start().strip_prefix("line")?.trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || !rest[digits.len()..].starts_with(':') {
        return None;
    }
    digits.parse().ok()
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
    anyhow::ensure!(
        entries.windows(2).all(|pair| pair[0].id < pair[1].id),
        "database dump IDs are not in ascending order — the core binary's dump format may have \
         changed since this parser was written",
    );

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
    let best_valid = parse_objective_dump_field(
        &mut lines,
        "best_valid_objective_value",
        ValueShape::Integer,
    )?;

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
                || value
                    .strip_prefix('-')
                    .unwrap_or(value)
                    .chars()
                    .all(|c| c.is_ascii_digit())
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
