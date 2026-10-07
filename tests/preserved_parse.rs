//! Tests `checker::parse`'s preserved-set helpers on sample `veripb` trace
//! output; no `veripb` binary needed.

use veripb_repl::checker::parse::{
    is_preserved_change_line, last_preserved_set, preserved_declaration,
};

fn names(names: &[&str]) -> Option<Vec<String>> {
    Some(names.iter().map(|name| name.to_string()).collect())
}

#[test]
fn reads_the_set_after_a_traced_preserved_add() {
    let trace = "\
Running VeriPB version 3.0.2
line    7: preserved_add x5 : 1 x1 >= 1 ;
  autoproving remaining proofgoals:
    proofgoal #1 is equal to a database constraint
Preserved set: x5 x3 x1
s VERIFIED NO CONCLUSION
";
    assert_eq!(last_preserved_set(trace), names(&["x1", "x3", "x5"]));
}

#[test]
fn takes_the_last_of_several() {
    let trace =
        "Preserved set: x1 x3 x5\nline 8: preserved_rm x5 : 1 x1 >= 1 ;\nPreserved set: x3 x1\n";
    assert_eq!(last_preserved_set(trace), names(&["x1", "x3"]));
}

#[test]
fn sorts_numeric_suffixes_naturally() {
    assert_eq!(
        last_preserved_set("Preserved set: x10 y1 x2 x1\n"),
        names(&["x1", "x2", "x10", "y1"])
    );
}

#[test]
fn empty_set() {
    assert_eq!(last_preserved_set("Preserved set: \n"), names(&[]));
}

#[test]
fn strips_colour_codes() {
    let trace = "\x1b[35mPreserved set\x1b[0m: \x1b[34mx3 x1\x1b[0m\n";
    assert_eq!(last_preserved_set(trace), names(&["x1", "x3"]));
}

#[test]
fn no_preserved_set_line() {
    assert_eq!(last_preserved_set("line 3: rup >= 1 ;\n"), None);
}

#[test]
fn reads_the_formula_declaration() {
    assert_eq!(
        Some(preserved_declaration("preserved: x3 x1 x10;")),
        names(&["x1", "x3", "x10"])
    );
    assert_eq!(
        Some(preserved_declaration("preserved: x1 ;")),
        names(&["x1"])
    );
}

#[test]
fn recognizes_preserved_change_lines() {
    assert!(is_preserved_change_line("preserved_add x5 : 1 x1 >= 1 ;"));
    assert!(is_preserved_change_line("preserved_rm x5 : 1 x1 >= 1 ;"));
    assert!(is_preserved_change_line(
        "@l preserved_add x5 : 1 x1 >= 1 ;"
    ));
    assert!(!is_preserved_change_line("preserved_addx x5 ;"));
    assert!(!is_preserved_change_line("epreserved x1 x3 ;"));
    assert!(!is_preserved_change_line("rup >= 1 ;"));
    assert!(!is_preserved_change_line(""));
}
