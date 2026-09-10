//! Tests for `veripb_repl::checker::parse::last_line_constraint_ids` —
//! pure text parsing against hand-crafted trace fixtures shaped like real
//! `veripb --trace-lines` output, no `veripb` binary needed.

use veripb_repl::checker::parse::last_line_constraint_ids;

#[test]
fn single_traced_line_yields_its_one_id() {
    let trace = "\
Running VeriPB version 3.0.2
line  510: rup 1 i[colours][b0] 2 i[colours][b1] 4 i[colours][b2] >= 2;
  ConstraintId 440: 1 i[colours][b0] 2 i[colours][b1] 4 i[colours][b2] >= 2
";
    assert_eq!(last_line_constraint_ids(trace), vec![440]);
}

#[test]
fn batched_trace_only_keeps_the_final_lines_ids() {
    let trace = "\
Running VeriPB version 3.0.2
line  3: rup 1 x1 >= 1;
  ConstraintId 3: 1 x1 >= 1
line  4: rup 1 x2 >= 1;
  ConstraintId 4: 1 x2 >= 1
line  5: rup 1 x1 1 x2 >= 1;
  ConstraintId 5: 1 x1 1 x2 >= 1
";
    // Not [3, 4, 5] — only what the trace attributes to the last traced
    // line (line 5).
    assert_eq!(last_line_constraint_ids(trace), vec![5]);
}

#[test]
fn a_line_producing_no_constraint_yields_nothing() {
    let trace = "\
Running VeriPB version 3.0.2
line  6: del id 3;
";
    assert!(last_line_constraint_ids(trace).is_empty());
}

#[test]
fn a_line_producing_multiple_constraints_yields_all_of_them() {
    let trace = "\
Running VeriPB version 3.0.2
line  7: pol 3 4 +;
  ConstraintId 8: 1 x1 1 x2 >= 1
  ConstraintId 9: 1 x1 1 x2 >= 2
";
    assert_eq!(last_line_constraint_ids(trace), vec![8, 9]);
}

#[test]
fn no_line_marker_at_all_scopes_to_the_whole_trace() {
    // `explain_line`'s own single-line trace shape, if it ever comes back
    // without a marker line.
    let trace = "  ConstraintId 12: 1 x1 >= 1\n";
    assert_eq!(last_line_constraint_ids(trace), vec![12]);
}

#[test]
fn empty_trace_yields_nothing() {
    assert!(last_line_constraint_ids("").is_empty());
}
