//! Integration tests for `veripb_repl::varnames` — black-box, against the
//! public API only (this is a separate crate from the library, so
//! anything not `pub` on `VarNames` isn't reachable here). Each test
//! writes a small formula to a real temp file and loads it through
//! [`VarNames::from_formula_file`], the same path a real session uses,
//! rather than exercising the scan against a bare string.

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::varnames::VarNames;

/// Write `text` to a fresh temp file and scan it.
fn scan(text: &str) -> VarNames {
    let mut file = NamedTempFile::new().expect("failed to create temp formula file");
    file.write_all(text.as_bytes())
        .expect("failed to write temp formula file");
    VarNames::from_formula_file(file.path()).expect("failed to read temp formula file")
}

#[test]
fn scans_plain_names() {
    let vars = scan("* #variable= 3 #constraint= 1\n1 x1 1 x2 1 ~x3 >= 1;\n");
    assert!(vars.contains("x1"));
    assert!(vars.contains("x2"));
    assert!(vars.contains("x3"));
    assert_eq!(vars.len(), 3);
}

#[test]
fn ignores_comment_lines_entirely() {
    // Even a comment line containing something shaped like a variable
    // name must not be picked up.
    let vars = scan("* mentions xNotAVar here\n1 x1 >= 1;\n");
    assert!(!vars.contains("xNotAVar"));
    assert!(vars.contains("x1"));
}

#[test]
fn accepts_bracketed_names() {
    let vars = scan("1 i[vertex0] 1 i[vertex1] >= 1;\n");
    assert!(vars.contains("i[vertex0]"));
    assert!(vars.contains("i[vertex1]"));
}

#[test]
fn ignores_coefficients_operators_and_labels() {
    let vars = scan("@lbl 2 x1 -1 x2 >= 1;\n");
    assert!(!vars.contains("2"));
    assert!(!vars.contains("-1"));
    assert!(!vars.contains(">="));
    assert!(!vars.contains("@lbl"));
    assert_eq!(vars.len(), 2);
}

#[test]
fn ignores_objective_keyword() {
    let vars = scan("min: 1 x1 1 x2 ;\n1 x1 1 x3 >= 1;\n");
    assert!(!vars.contains("min:"));
    assert!(vars.contains("x1"));
    assert!(vars.contains("x2"));
    assert!(vars.contains("x3"));
}

#[test]
fn rejects_single_character_tokens() {
    // The grammar requires at least two symbols; a lone-letter term
    // never counts, even in an otherwise well-formed line.
    let vars = scan("1 x >= 1;\n");
    assert!(!vars.contains("x"));
    assert!(vars.is_empty());
}

#[test]
fn contains_ignores_negation_marker_by_caller_convention() {
    // `contains` itself expects an already-stripped name — this just
    // documents that `~x1` is not, itself, a known name.
    let vars = scan("1 x1 >= 1;\n");
    assert!(!vars.contains("~x1"));
    assert!(vars.contains("x1"));
}

#[test]
fn mentioned_dedupes_in_first_seen_order() {
    let vars = scan("1 x1 1 x2 1 x3 >= 1;\n");
    assert_eq!(
        vars.mentioned("rup 1 x2 1 ~x1 1 x2 : 5;"),
        vec!["x2".to_string(), "x1".to_string()]
    );
}

#[test]
fn mentioned_excludes_shape_matches_never_actually_seen() {
    let vars = scan("1 x1 >= 1;\n");
    // "rup" is shaped like a variable name but was never in the
    // formula, so it must not be reported as mentioned.
    assert_eq!(vars.mentioned("rup x1 >= 1;"), vec!["x1".to_string()]);
}

#[test]
fn names_iterates_every_known_variable() {
    let vars = scan("1 x1 1 x2 >= 1;\n");
    let mut names: Vec<&str> = vars.names().collect();
    names.sort_unstable();
    assert_eq!(names, vec!["x1", "x2"]);
}

#[test]
fn empty_formula_yields_empty_set() {
    let vars = scan("");
    assert!(vars.is_empty());
    assert_eq!(vars.len(), 0);
}
