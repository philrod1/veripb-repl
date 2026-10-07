//! Tests `checker::parse`'s `rup`-line helpers (`last_rup_hints`,
//! `split_rup_hints`, `is_rup_line`) on hand-written `--elaborate` output.

use veripb_repl::checker::RupHint::{ConstraintId, NegatedPremise};
use veripb_repl::checker::parse::{is_rup_line, last_rup_hints, split_rup_hints};

#[test]
fn constraint_ids_only() {
    assert_eq!(
        last_rup_hints("rup 1 x2 >= 1 : 1 2;\n"),
        Some(vec![ConstraintId(1), ConstraintId(2)])
    );
}

#[test]
fn negated_premise_only() {
    assert_eq!(
        last_rup_hints("rup 1 x2 >= 1 : ~;\n"),
        Some(vec![NegatedPremise])
    );
}

#[test]
fn mixed_hints_keep_their_order() {
    assert_eq!(
        last_rup_hints("rup >= 1 : 3 ~ 4 ;\n"),
        Some(vec![ConstraintId(3), NegatedPremise, ConstraintId(4)])
    );
}

#[test]
fn labelled_line() {
    assert_eq!(
        last_rup_hints("@lemma rup 1 x2 >= 1 : 1 ;\n"),
        Some(vec![ConstraintId(1)])
    );
}

#[test]
fn rup_line_without_hints_yields_none() {
    assert_eq!(last_rup_hints("rup 1 x2 >= 1 ;\n"), None);
}

#[test]
fn no_rup_line_at_all_yields_none() {
    assert_eq!(
        last_rup_hints("pseudo-Boolean proof version 3.0\nf 3;\npol 1 2 + ;\n"),
        None
    );
}

#[test]
fn unparsable_tokens_are_dropped() {
    assert_eq!(
        last_rup_hints("rup >= 1 : 1 x 2 ;\n"),
        Some(vec![ConstraintId(1), ConstraintId(2)])
    );
}

#[test]
fn picks_the_last_rup_line() {
    let proof = "\
pseudo-Boolean proof version 3.0
f 3;
rup 1 x2 >= 1 : 1 2 ;
pol 4 3 + ;
rup >= 1 : 4 ~ ;
";
    assert_eq!(
        last_rup_hints(proof),
        Some(vec![ConstraintId(4), NegatedPremise])
    );
}

#[test]
fn split_keeps_label_and_constraint_as_prefix() {
    let (prefix, hints) = split_rup_hints("@l rup 1 x2 >= 1 : 99 ;");
    assert_eq!(prefix, "@l rup 1 x2 >= 1");
    assert_eq!(hints, Some(vec![ConstraintId(99)]));
}

#[test]
fn split_without_hints_strips_the_terminator() {
    assert_eq!(split_rup_hints("rup 1 x2 >= 1 ;"), ("rup 1 x2 >= 1", None));
    assert_eq!(split_rup_hints("rup 1 x2 >= 1;"), ("rup 1 x2 >= 1", None));
}

#[test]
fn split_with_empty_hint_list() {
    assert_eq!(split_rup_hints("rup >= 1 : ;"), ("rup >= 1", Some(vec![])));
}

#[test]
fn is_rup_line_matches_exact_tokens() {
    assert!(is_rup_line("rup 1 x2 >= 1 ;"));
    assert!(is_rup_line("  @l rup >= 1 ;"));
    assert!(!is_rup_line("rupx 1 x2 >= 1 ;"));
    assert!(!is_rup_line("@l pol 1 2 + ;"));
    assert!(!is_rup_line("@l"));
    assert!(!is_rup_line(""));
}
