//! Tests for `checker::parse::check_outcome` on veripb's rejection output
//! shapes (no binary needed).

use std::path::PathBuf;

use veripb_repl::checker::CheckOutcome;
use veripb_repl::checker::invoke::RawInvocation;
use veripb_repl::checker::parse::check_outcome;

fn failed_run(stdout: &str, stderr: &str) -> RawInvocation {
    RawInvocation {
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        success: false,
        code: Some(1),
        proof_file_path: PathBuf::from("/tmp/proof.pbp"),
        formula_file_path: PathBuf::from("/tmp/formula.opb"),
    }
}

const SYNTAX_ERROR: &str = "Error: Syntax error while parsing proof file!\n\nCaused by:\n    \
    Expected a constraint ID (`~`, label or signed integer) or `;` but found `x2` at line 20 col 50.\n";

fn rejected_line(raw: &RawInvocation) -> usize {
    match check_outcome(raw).expect("a rejection with a line") {
        CheckOutcome::Rejected { line, .. } => line,
        CheckOutcome::Accepted { .. } => panic!("expected a rejection"),
    }
}

#[test]
fn finds_a_syntax_error_line_after_a_padded_trace() {
    let stdout =
        "Running VeriPB version 3.0.2\nline    3: rup 1 x1 >= 1;\n  ConstraintId 59: 1 x1 >= 1\n";
    assert_eq!(rejected_line(&failed_run(stdout, SYNTAX_ERROR)), 20);
}

#[test]
fn ignores_unpadded_trace_markers() {
    let stdout =
        "Running VeriPB version 3.0.2\nline 1234: rup 1 x1 >= 1;\n  ConstraintId 59: 1 x1 >= 1\n";
    assert_eq!(rejected_line(&failed_run(stdout, SYNTAX_ERROR)), 20);
}

#[test]
fn finds_a_syntax_error_line_without_a_trace() {
    assert_eq!(
        rejected_line(&failed_run("Running VeriPB version 3.0.2\n", SYNTAX_ERROR)),
        20
    );
}

/// veripb's error on running out of input, expecting `expected`.
fn ran_out(expected: &str) -> RawInvocation {
    failed_run(
        "Running VeriPB version 3.0.2\n",
        &format!(
            "Error: Syntax error while parsing proof file!\n\nCaused by:\n    \
             Expected {expected} but found end of file (EOF) at line 5 col 1.\n"
        ),
    )
}

#[test]
fn running_out_of_input_after_a_complete_line_is_accepted() {
    for expected in ["a top level rule name or `output`", "`conclusion`", "`end`"] {
        assert!(
            check_outcome(&ran_out(expected)).unwrap().is_accepted(),
            "{expected}"
        );
    }
}

#[test]
fn running_out_of_input_mid_rule_is_accepted_too() {
    // A rule may span lines; veripb reports no error for an unfinished one.
    assert!(check_outcome(&ran_out("`:` or `;`")).unwrap().is_accepted());
}

#[test]
fn only_numbered_ids_count_as_naming_a_constraint() {
    use veripb_repl::checker::parse::mentions_constraint_id;
    assert!(mentions_constraint_id("falsifies the constraint with ID 3"));
    assert!(!mentions_constraint_id(
        "the line is incomplete: expected a constraint ID (label or signed integer) or `;` next"
    ));
}
