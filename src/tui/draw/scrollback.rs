//! Structural highlighting for the scrollback pane. Lines are stored as
//! plain text (see `tui::Scrollback`); each visible one is tokenized here
//! at draw time by recognizing a handful of fixed shapes — veripb's own
//! trace lines (`line N: ...`, `ConstraintId N: ...`, its version banner),
//! `:explain`'s headers and verdicts (keyed off the phrases it exports, so
//! the two can't drift apart), and `Error:` lines from any command.
//! Anything unrecognized stays one plain span, exactly as before.
//!
//! Every result is **character-for-character the same width as its
//! input**: the scrollback's horizontal scroll and longest-line math run
//! on the raw strings, so a tokenization that changed a line's width
//! would misalign it. Constraint and rule text goes through
//! `tokenize_proof_line` (width-exact, lookup-based variable/label
//! colouring) rather than `tokenize_constraint`, which rebuilds the text
//! with single spaces.

use super::{Span, TokenStyle, plain, styled, tokenize_proof_line};
use crate::commands::explain::{
    NEEDED_SUFFIX, NEGATED_PREMISE, REJECTED_INFIX, TYPED_HINTS, VERDICT_CHECKS, VERDICT_FAILS,
};
use crate::varnames::VarNames;

/// Styled spans for one scrollback line. `vars` colours variable names in
/// constraint/rule text; `None` (no formula loaded) just leaves them plain.
pub(super) fn tokenize(line: &str, vars: Option<&VarNames>) -> Vec<Span> {
    let body = line.trim_start();
    let indent = &line[..line.len() - body.len()];
    let mut spans = Vec::new();
    if !indent.is_empty() {
        spans.push(plain(indent.to_string()));
    }
    spans.extend(tokenize_body(body, vars));
    spans
}

fn tokenize_body(body: &str, vars: Option<&VarNames>) -> Vec<Span> {
    use TokenStyle::{Accent, Dim, Error, Heading, Ok};

    if body.is_empty() {
        return Vec::new();
    }
    if body.starts_with("Error: ") || body.starts_with(VERDICT_FAILS) {
        return vec![styled(body, Error)];
    }
    if body.starts_with(VERDICT_CHECKS) {
        return vec![styled(body, Ok)];
    }
    if body.starts_with("Running VeriPB version") {
        return vec![styled(body, Dim)];
    }
    if let Some(rest) = body.strip_prefix(TYPED_HINTS) {
        return vec![styled(TYPED_HINTS, Dim), plain(rest.to_string())];
    }
    if let Some(rest) = body.strip_prefix(NEGATED_PREMISE) {
        let (tilde, description) = NEGATED_PREMISE.split_at(1);
        return vec![
            styled(tilde, Accent),
            styled(description, Dim),
            plain(rest.to_string()),
        ];
    }

    // veripb's trace marker: `line    3: <rule as typed>`.
    if let Some((head, rest)) = numbered(body, "line")
        && let Some(rule) = rest.strip_prefix(':')
    {
        let mut spans = vec![styled(&format!("{head}:"), Heading)];
        spans.extend(rule_spans(rule, vars));
        return spans;
    }

    // `:explain`'s own headers: `Line 3 needed:`, `Line 3 is rejected: <reason>`.
    if let Some((head, rest)) = numbered(body, "Line ") {
        if let Some(tail) = rest.strip_prefix(NEEDED_SUFFIX) {
            return vec![
                styled(&format!("{head}{NEEDED_SUFFIX}"), Heading),
                plain(tail.to_string()),
            ];
        }
        if let Some(reason) = rest.strip_prefix(REJECTED_INFIX) {
            return vec![
                styled(&format!("{head}{REJECTED_INFIX}"), Heading),
                styled(reason, Error),
            ];
        }
    }

    // `ConstraintId 3: <constraint>` (veripb's trace, or a listed hint), or
    // `ConstraintId 99 — no longer in the database` / `... is not in the
    // database` (a hint that's gone).
    if let Some((head, rest)) = numbered(body, "ConstraintId ") {
        if let Some(constraint) = rest.strip_prefix(':') {
            let mut spans = vec![styled(&format!("{head}:"), Dim)];
            spans.extend(rule_spans(constraint, vars));
            return spans;
        }
        return vec![styled(head, Dim), styled(rest, Error)];
    }

    vec![plain(body.to_string())]
}

/// Splits `body` after `keyword`, any whitespace, and a run of digits —
/// `("line    3", ": rup ...")` — or `None` if it doesn't start that way.
fn numbered<'a>(body: &'a str, keyword: &str) -> Option<(&'a str, &'a str)> {
    let after_keyword = body.strip_prefix(keyword)?.trim_start();
    let after_digits = after_keyword.trim_start_matches(|c: char| c.is_ascii_digit());
    if after_digits.len() == after_keyword.len() {
        return None;
    }
    Some(body.split_at(body.len() - after_digits.len()))
}

/// Rule or constraint text, variables and `@labels` coloured when there's
/// a formula to look names up in.
fn rule_spans(text: &str, vars: Option<&VarNames>) -> Vec<Span> {
    match vars {
        Some(vars) => tokenize_proof_line(text, vars),
        None if text.is_empty() => Vec::new(),
        None => vec![plain(text.to_string())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TokenStyle::{Accent, Dim, Error, Heading, Label, Ok, Plain, Variable};

    fn vars() -> VarNames {
        VarNames::from_formula_text("1 x1 1 x2 >= 1 ;\n")
    }

    /// Tokenizes `line` (with variables known), checks the result is
    /// width-exact, and returns the non-blank spans as `(text, style)`.
    fn styles(line: &str) -> Vec<(String, TokenStyle)> {
        let spans = tokenize(line, Some(&vars()));
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, line, "tokenizing must preserve the line exactly");
        spans
            .into_iter()
            .filter(|s| !s.text.trim().is_empty())
            .map(|s| (s.text.trim().to_string(), s.style))
            .collect()
    }

    fn s(text: &str, style: TokenStyle) -> (String, TokenStyle) {
        (text.to_string(), style)
    }

    #[test]
    fn trace_marker_header_and_rule() {
        assert_eq!(
            styles("line    3: rup 1 x2 >= 1 : 3 ~ ;"),
            vec![
                s("line    3:", Heading),
                s("rup", Plain),
                s("1", Plain),
                s("x2", Variable),
                s(">=", Plain),
                s("1", Plain),
                s(":", Plain),
                s("3", Plain),
                s("~", Plain),
                s(";", Plain),
            ]
        );
    }

    #[test]
    fn constraint_id_line_with_label_and_negated_literal() {
        assert_eq!(
            styles("    ConstraintId 3: @l 1 ~x2 >= 1"),
            vec![
                s("ConstraintId 3:", Dim),
                s("@l", Label),
                s("1", Plain),
                s("~", Plain),
                s("x2", Variable),
                s(">=", Plain),
                s("1", Plain),
            ]
        );
    }

    #[test]
    fn missing_constraint_id() {
        assert_eq!(
            styles("  ConstraintId 99 is not in the database (deleted or never derived)"),
            vec![
                s("ConstraintId 99", Dim),
                s("is not in the database (deleted or never derived)", Error),
            ]
        );
        assert_eq!(
            styles("  ConstraintId 7 — no longer in the database"),
            vec![s("ConstraintId 7", Dim), s("— no longer in the database", Error)]
        );
    }

    #[test]
    fn explain_headers() {
        assert_eq!(styles("Line 3 needed:"), vec![s("Line 3 needed:", Heading)]);
        assert_eq!(
            styles("Line 4 is rejected: Accessing the database out of bound"),
            vec![
                s("Line 4 is rejected:", Heading),
                s("Accessing the database out of bound", Error),
            ]
        );
    }

    #[test]
    fn negated_premise_hint() {
        assert_eq!(
            styles("  ~ (the negated constraint itself)"),
            vec![s("~", Accent), s("(the negated constraint itself)", Dim)]
        );
    }

    #[test]
    fn verdicts_and_errors() {
        assert_eq!(
            styles("  Without hints it DOES check — the hint list is the problem."),
            vec![s("Without hints it DOES check — the hint list is the problem.", Ok)]
        );
        assert_eq!(
            styles("  Without hints it still fails: nope."),
            vec![s("Without hints it still fails: nope.", Error)]
        );
        assert_eq!(styles("Error: bad thing"), vec![s("Error: bad thing", Error)]);
    }

    #[test]
    fn typed_hints_and_banner() {
        assert_eq!(
            styles("  Typed hints: 99 ~"),
            vec![s("Typed hints:", Dim), s("99 ~", Plain)]
        );
        assert_eq!(
            styles("Running VeriPB version 3.0.2"),
            vec![s("Running VeriPB version 3.0.2", Dim)]
        );
    }

    #[test]
    fn unrecognized_lines_stay_plain() {
        assert_eq!(
            styles("(added to the buffer, unchecked)"),
            vec![s("(added to the buffer, unchecked)", Plain)]
        );
        // Starts like a header but isn't one.
        assert_eq!(styles("lines remain"), vec![s("lines remain", Plain)]);
        assert_eq!(styles("Line 3 is fine"), vec![s("Line 3 is fine", Plain)]);
        assert!(styles("").is_empty());
    }

    #[test]
    fn without_a_formula_variables_stay_plain() {
        let spans = tokenize("  ConstraintId 3: 1 x2 >= 1", None);
        assert!(spans.iter().all(|s| s.style != Variable));
    }
}
