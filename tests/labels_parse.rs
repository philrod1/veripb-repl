//! Tests for the label and constraint-count helpers in
//! `veripb_repl::checker::parse` (no binary needed).

use veripb_repl::checker::RupHint::{ConstraintId, Label, NegatedPremise};
use veripb_repl::checker::parse::{
    constraint_ids_by_line, formula_line_constraints, leading_labels, rule_keyword, split_rup_hints,
};

#[test]
fn constraints_per_formula_line_match_veripb() {
    // Counts confirmed against veripb's own `f N` check.
    assert_eq!(formula_line_constraints("1 x1 1 x2 >= 1 ;"), 1);
    assert_eq!(formula_line_constraints("1 x1 1 x2 <= 1 ;"), 1);
    assert_eq!(formula_line_constraints("1 x1 1 x2 = 1 ;"), 2);
    assert_eq!(formula_line_constraints("x3 ==> 1 x1 1 x2 >= 1 ;"), 1);
    assert_eq!(formula_line_constraints("x3 <== 1 x1 1 x2 >= 1 ;"), 1);
    assert_eq!(formula_line_constraints("x3 <==> 1 x1 1 x2 >= 1 ;"), 2);
    assert_eq!(formula_line_constraints("x3 ==> 1 x1 1 x2 = 1 ;"), 2);
    assert_eq!(
        formula_line_constraints("@up @dn x3 <==> 1 x1 1 x2 >= 1 ;"),
        2
    );
}

#[test]
fn leading_labels_in_order() {
    assert_eq!(
        leading_labels("@up @dn x3 <==> 1 x1 >= 1 ;"),
        vec!["@up", "@dn"]
    );
    assert_eq!(leading_labels("@d pol 1 2 + ;"), vec!["@d"]);
    assert!(leading_labels("pol @a @b + ;").is_empty());
}

#[test]
fn rule_keyword_skips_every_leading_label() {
    assert_eq!(rule_keyword("@a @b rup >= 1 ;"), Some("rup"));
    assert_eq!(rule_keyword("pol 1 2 + ;"), Some("pol"));
    assert_eq!(rule_keyword("@a"), None);
}

#[test]
fn label_hints_are_kept() {
    assert_eq!(
        split_rup_hints("rup 1 ~x1 >= 1 : @c 2 ~ ;").1,
        Some(vec![
            Label("@c".to_string()),
            ConstraintId(2),
            NegatedPremise
        ])
    );
}

#[test]
fn constraint_ids_grouped_by_traced_line() {
    let trace = "\
Running VeriPB version 3.0.2
  ConstraintId 1: 1 x1 1 x2 >= 1
line    1: pseudo-Boolean proof version 3.0
line    2: f 3;
line    3: @d pol @one @two + ;
  ConstraintId 4: 2 x2 >= 1
line    4: @ee e 1 x1 1 x2 >= 1 ;
line    5: @r red 1 x5 1 ~x1 >= 1 : x5 1 ;
  ** proofgoal from satisfying added constraint **
proofgoal #1: [] |- 1 ~x1 >= 0
  ConstraintId 6: 1 ~x1 1 x5 >= 1
";
    let by_line = constraint_ids_by_line(trace);
    assert_eq!(by_line.get(&3), Some(&vec![4]));
    assert_eq!(by_line.get(&4), Some(&vec![]));
    assert_eq!(by_line.get(&5), Some(&vec![6]));
    assert_eq!(by_line.get(&1), Some(&vec![]));

    // A line at a time, keeping only some lines. Lines that are not trace
    // (including a `ConstraintId` line before the first marker) are not
    // taken.
    let mut parser = veripb_repl::checker::parse::ConstraintIdsByLine::new(|n| n != 3);
    let taken: Vec<bool> = trace.lines().map(|line| parser.feed(line)).collect();
    let streamed = parser.finish();
    assert_eq!(streamed.get(&3), None);
    assert_eq!(streamed.get(&5), by_line.get(&5));
    assert_eq!(streamed.get(&4), by_line.get(&4));
    let kept: Vec<&str> = trace
        .lines()
        .zip(&taken)
        .filter(|(_, taken)| !**taken)
        .map(|(line, _)| line)
        .collect();
    assert_eq!(
        kept,
        vec![
            "Running VeriPB version 3.0.2",
            "  ConstraintId 1: 1 x1 1 x2 >= 1",
            "  ** proofgoal from satisfying added constraint **",
            "proofgoal #1: [] |- 1 ~x1 >= 0",
        ]
    );
}

#[test]
fn pol_table_shows_operands_as_typed() {
    use veripb_repl::checker::parse::pol_table_with_typed_operands;
    let trace = "\
Running VeriPB version 3.0.2
line    5: @at_most_one pol @sum_all 2 d ;
    depth  operand  top of stack
    1      6        3 ~x1 1 ~x2 2 ~x3 >= 3
    2      2        2
    1      d        2 ~x1 1 ~x2 1 ~x3 >= 2
  ConstraintId 7: 2 ~x1 1 ~x2 1 ~x3 >= 2
";
    let shown = pol_table_with_typed_operands(trace, "@at_most_one pol @sum_all 2 d ;");
    assert_eq!(
        shown,
        "\
Running VeriPB version 3.0.2
line    5: @at_most_one pol @sum_all 2 d ;
    depth  operand   top of stack
    1      @sum_all  3 ~x1 1 ~x2 2 ~x3 >= 3
    2      2         2
    1      d         2 ~x1 1 ~x2 1 ~x3 >= 2
  ConstraintId 7: 2 ~x1 1 ~x2 1 ~x3 >= 2
"
    );
    // A token count that doesn't match the rows leaves the trace alone.
    assert_eq!(pol_table_with_typed_operands(trace, "pol 6 ;"), trace);
}

#[test]
fn rejection_reasons_name_labelled_ids() {
    use veripb_repl::checker::parse::annotate_constraint_ids;
    let mut labels = ahash::AHashMap::new();
    labels.insert(3, vec!["@no_34".to_string()]);
    assert_eq!(
        annotate_constraint_ids(
            "The given assignment falsifies the constraint with ID 3.",
            &labels
        ),
        "The given assignment falsifies the constraint with ID 3 (@no_34)."
    );
    assert_eq!(
        annotate_constraint_ids("constraint with ID 8 is not contradicting", &labels),
        "constraint with ID 8 is not contradicting"
    );
}
