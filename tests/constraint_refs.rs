//! Tests for `checker::parse::constraint_refs`: which tokens of a proof
//! line refer to a constraint (no binary needed).

use veripb_repl::checker::parse::{ConstraintRef, constraint_refs};

/// Each reference in `line` as its text and what it names.
fn refs(line: &str) -> Vec<(&str, ConstraintRef<'_>)> {
    constraint_refs(line)
        .into_iter()
        .map(|(range, reference)| (&line[range], reference))
        .collect()
}

use ConstraintRef::{Id, Label};

#[test]
fn pol_operands_are_ids_but_scalars_are_not() {
    assert_eq!(
        refs("@x pol 3 @a + 2 * 5 d 0 +;"),
        [("@x", Label("@x")), ("3", Id(3)), ("@a", Label("@a"))]
    );
    assert_eq!(refs("pol 7 x1 + ;"), [("7", Id(7))]);
}

#[test]
fn hints_after_a_colon_are_ids() {
    assert_eq!(
        refs("rup 1 x1 2 x2 >= 1 : 4 ~ @h 9;"),
        [("4", Id(4)), ("@h", Label("@h")), ("9", Id(9))]
    );
    assert_eq!(refs("rup 1 x1 >= 1 ;"), []);
    assert_eq!(refs("ia 1 x1 >= 1 : 12 ;"), [("12", Id(12))]);
}

#[test]
fn id_lists_and_proofgoals_are_ids() {
    assert_eq!(refs("del id 3 4 ;"), [("3", Id(3)), ("4", Id(4))]);
    assert_eq!(refs("core id 8 ;"), [("8", Id(8))]);
    assert_eq!(refs("del spec 1 x1 >= 1 ;"), []);
    assert_eq!(refs("del range 3 5 ;"), []);
    assert_eq!(refs("proofgoal 6"), [("6", Id(6))]);
}

#[test]
fn constraint_text_and_relative_ids_are_not_references() {
    assert_eq!(refs("a 1 x1 1 x2 >= 2 ;"), []);
    assert_eq!(refs("red 1 x1 >= 1 ; x1 -> 1 ;"), []);
    assert_eq!(refs("pol -1 -2 + ;"), []);
    assert_eq!(refs("@f 1 x1 >= 1 ;"), [("@f", Label("@f"))]);
}
