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
}
