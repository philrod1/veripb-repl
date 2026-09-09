//! Integration tests for `veripb_repl::session::Session` — black-box,
//! against the public API only. Pure formula-file parsing; no `veripb`
//! subprocess needed.

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::session::Session;

fn load(text: &str) -> Session {
    let mut file = NamedTempFile::with_suffix(".opb").expect("failed to create temp formula file");
    file.write_all(text.as_bytes())
        .expect("failed to write temp formula file");
    Session::load(file.path().to_str().expect("temp path is valid UTF-8"))
        .expect("failed to load session")
}

#[test]
fn counts_plain_constraints_correctly() {
    let session = load("1 x1 1 x2 >= 1 ;\n1 ~x1 1 x2 >= 1 ;\n1 ~x2 >= 1 ;\n");
    assert_eq!(session.formula.len(), 3);
    assert!(session.objective.is_none());
    assert!(session.preserved.is_none());
}

#[test]
fn does_not_count_the_objective_line_as_a_constraint() {
    let session = load("min: 1 x1 2 x2 ;\n1 x1 1 x2 >= 1 ;\n1 ~x1 1 x2 >= 1 ;\n");
    assert_eq!(session.formula.len(), 2);
    assert_eq!(session.objective.as_deref(), Some("min: 1 x1 2 x2 ;"));
}

/// The bug this test guards against: a formula file's `preserved: ...;`
/// declaration (used by `preserved_add`/`preserved_rm`/`epreserved`/`solx`
/// proof rules) was being miscounted as an ordinary constraint, inflating
/// both the Formula pane's count and the synthesized `f N;` preamble line.
#[test]
fn does_not_count_the_preserved_line_as_a_constraint() {
    let session = load(
        "min: 1 x1 2 x2 ;\n\
         preserved: x1 x2 x3 ;\n\
         1 x1 1 x2 >= 1 ;\n\
         1 ~x1 1 x2 >= 1 ;\n",
    );
    assert_eq!(session.formula.len(), 2);
    assert_eq!(session.objective.as_deref(), Some("min: 1 x1 2 x2 ;"));
    assert_eq!(session.preserved.as_deref(), Some("preserved: x1 x2 x3 ;"));
}

#[test]
fn ignores_comment_lines() {
    let session = load("* #variable= 2 #constraint= 1\n1 x1 1 x2 >= 1 ;\n");
    assert_eq!(session.formula.len(), 1);
}

#[test]
fn preamble_line_count_matches_formula_len_not_raw_line_count() {
    // The regression this bug actually manifested as: the synthesized
    // `f N;` preamble line disagreeing with the real formula's own count,
    // because `formula.len()` was inflated by a miscounted declaration
    // line.
    let session = load(
        "min: 1 x1 2 x2 ;\n\
         preserved: x1 x2 ;\n\
         1 x1 1 x2 >= 1 ;\n\
         1 ~x1 1 x2 >= 1 ;\n\
         1 ~x2 >= 1 ;\n",
    );
    assert_eq!(session.formula.len(), 3);
    let preamble = session.preamble_lines();
    assert_eq!(preamble[1], "f 3;");
}
