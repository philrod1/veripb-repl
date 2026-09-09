//! End-to-end tests for `veripb_repl::checker` against a real `veripb`
//! binary. Requires `$VERIPB_REPL_VERIPB_BIN` to point at one; skipped
//! (not failed) otherwise, so this doesn't break on a machine without
//! `veripb` installed.

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::checker::{self, CheckOutcome};

const FORMULA: &str = "\
* #variable= 2 #constraint= 3
1 x1 1 x2 >= 1 ;
1 ~x1 1 x2 >= 1 ;
1 ~x2 >= 1 ;
";

fn veripb_available() -> bool {
    std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_some()
}

fn formula_file() -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("failed to create temp formula file");
    file.write_all(FORMULA.as_bytes())
        .expect("failed to write temp formula file");
    file
}

#[test]
fn accepts_a_fully_checked_proof() {
    if !veripb_available() {
        eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
        return;
    }
    let formula = formula_file();
    let proof = "\
pseudo-Boolean proof version 3.0
f 3;
rup 1 x2 >= 1 ;
rup >= 1 ;
output NONE;
conclusion UNSAT;
end pseudo-Boolean proof;
";
    let outcome = checker::check(formula.path(), proof, None).expect("check should not fail to invoke");
    assert!(outcome.is_accepted());
}

#[test]
fn accepts_a_proof_missing_its_conclusion() {
    if !veripb_available() {
        eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
        return;
    }
    let formula = formula_file();
    // Valid rules, but no closing output/conclusion/end lines — exactly
    // what an ordinary :verify hands the checker. Must not be
    // misreported as a rejection.
    let proof = "\
pseudo-Boolean proof version 3.0
f 3;
rup 1 x2 >= 1 ;
";
    let outcome = checker::check(formula.path(), proof, None).expect("check should not fail to invoke");
    assert!(outcome.is_accepted());
}

#[test]
fn rejects_a_genuinely_bad_line_with_the_right_line_number() {
    if !veripb_available() {
        eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
        return;
    }
    let formula = formula_file();
    // Line 3 references a constraint ID that doesn't exist.
    let proof = "\
pseudo-Boolean proof version 3.0
f 3;
rup 1 x2 >= 1 : 99;
";
    let outcome = checker::check(formula.path(), proof, None).expect("check should not fail to invoke");
    match outcome {
        CheckOutcome::Rejected { line, .. } => assert_eq!(line, 3),
        CheckOutcome::Accepted { trace } => {
            panic!("expected a rejection, got Accepted with trace: {trace}")
        }
    }
}

#[test]
fn check_with_database_reports_every_live_entry() {
    if !veripb_available() {
        eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
        return;
    }
    let formula = formula_file();
    let proof = "\
pseudo-Boolean proof version 3.0
f 3;
rup 1 x2 >= 1 ;
rup >= 1 ;
output NONE;
conclusion UNSAT;
end pseudo-Boolean proof;
";
    let (outcome, database) =
        checker::check_with_database(formula.path(), proof, None).expect("check should not fail to invoke");
    assert!(outcome.is_accepted());
    assert_eq!(database.entries.len(), 5);
    assert_eq!(database.entries.iter().filter(|e| e.is_core).count(), 3);
    assert_eq!(database.entries.iter().filter(|e| !e.is_core).count(), 2);
}

#[test]
fn check_with_database_reflects_partial_progress_on_rejection() {
    if !veripb_available() {
        eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
        return;
    }
    let formula = formula_file();
    // Line 3 is rejected before it can add anything to the database, so
    // only the 3 core constraints should be live.
    let proof = "\
pseudo-Boolean proof version 3.0
f 3;
rup 1 x2 >= 1 : 99;
";
    let (outcome, database) =
        checker::check_with_database(formula.path(), proof, None).expect("check should not fail to invoke");
    assert!(!outcome.is_accepted());
    assert_eq!(database.entries.len(), 3);
}
