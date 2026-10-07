//! Tests for formula-error reporting: `checker::parse::formula_error` on
//! veripb's message shape (no binary needed), and `Session::load_checked`
//! (requires `$VERIPB_REPL_VERIPB_BIN`; skips otherwise).

use std::io::Write;
use std::path::PathBuf;

use tempfile::NamedTempFile;
use veripb_repl::checker::invoke::RawInvocation;
use veripb_repl::checker::parse::formula_error;
use veripb_repl::session::Session;

fn failed_run(stderr: &str) -> RawInvocation {
    RawInvocation {
        stdout: "Running VeriPB version 3.0.2\n".to_string(),
        stderr: stderr.to_string(),
        success: false,
        code: Some(1),
        proof_file_path: PathBuf::from("/tmp/proof.pbp"),
        formula_file_path: PathBuf::from("/tmp/formula.opb"),
    }
}

#[test]
fn rewrites_the_formula_location() {
    let raw = failed_run(
        "Error: Unexpected token starting at /tmp/formula.opb:4:6! Expected '>=', '<=', or integer!\n",
    );
    assert_eq!(
        formula_error(&raw).as_deref(),
        Some(
            "the formula has an error: Unexpected token starting at line 4, column 6! \
             Expected '>=', '<=', or integer!"
        )
    );
}

#[test]
fn ignores_errors_that_dont_name_the_formula() {
    let raw = failed_run("Error: Checking error at /tmp/proof.pbp:3\n\nCaused by:\n\tnope\n");
    assert_eq!(formula_error(&raw), None);
}

#[test]
fn ignores_successful_runs() {
    let mut raw = failed_run("/tmp/formula.opb:1:1 mentioned");
    raw.success = true;
    assert_eq!(formula_error(&raw), None);
}

fn formula_file(text: &str) -> NamedTempFile {
    let mut file = NamedTempFile::with_suffix(".opb").expect("failed to create temp formula file");
    file.write_all(text.as_bytes())
        .expect("failed to write temp formula file");
    file
}

#[test]
fn load_checked_rejects_a_malformed_formula() {
    if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
        eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
        return;
    }
    let file = formula_file("* comment\nmin: 1 x1 ;\n1 x1 1 x2 >= 1 ;\n1 x1 x2 >= ;\n");
    let err = Session::load_checked(file.path().to_str().unwrap())
        .err()
        .expect("a malformed formula must not load");
    let message = format!("{err:#}");
    assert!(
        message.contains("the formula has an error") && message.contains("line 4, column 6"),
        "{message}"
    );
}

#[test]
fn load_checked_accepts_a_valid_formula() {
    if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
        eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
        return;
    }
    let file = formula_file("min: 1 x1 ;\npreserved: x1 ;\n1 x1 1 x2 >= 1 ;\n");
    Session::load_checked(file.path().to_str().unwrap()).expect("a valid formula loads");
}
