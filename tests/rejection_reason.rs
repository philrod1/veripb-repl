//! Tests for `veripb_repl::checker::parse::rejection_reason` against real
//! `veripb` rejection output shapes, no `veripb` binary needed.

use veripb_repl::checker::parse::rejection_reason;

#[test]
fn checking_error_keeps_only_the_cause() {
    let output = "\
Running VeriPB version 3.0.2
Error: Checking error at /tmp/.tmpZdydfJ.pbp:3

Caused by:
\tAccessing the database out of bound with index 99. The index should be between -4 and 3.
";
    assert_eq!(
        rejection_reason(output),
        "Accessing the database out of bound with index 99. The index should be between -4 and 3."
    );
}

#[test]
fn syntax_error_drops_the_source_excerpt() {
    let output = "\
Running VeriPB version 3.0.2
Error: Syntax error while parsing proof file!

Caused by:
    Expected a top level rule name or `output` but found `foo` at line 3 col 1.
    foo
    ^^^
";
    assert_eq!(
        rejection_reason(output),
        "Expected a top level rule name or `output` but found `foo` at line 3 col 1."
    );
}

#[test]
fn without_a_cause_falls_back_to_the_error_line() {
    let output = "Running VeriPB version 3.0.2\nError: something went wrong at line 3\n";
    assert_eq!(rejection_reason(output), "something went wrong at line 3");
}

#[test]
fn unrecognized_output_is_kept_whole() {
    assert_eq!(rejection_reason("  odd failure at line 3\n"), "odd failure at line 3");
}
