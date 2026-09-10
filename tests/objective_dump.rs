//! Tests for `veripb_repl::checker::parse::parse_objective_dump` — pure
//! text parsing against hand-crafted fixtures, no `veripb` binary needed.
//! `--dump-objective` doesn't exist on any binary available when this was
//! written, so these are the only coverage until a real one does; see
//! `tests/checker.rs` for the same posture `parse_database_dump` had
//! before `--dump-database` existed.

use veripb_repl::checker::parse::parse_objective_dump;

#[test]
fn parses_two_known_values() {
    let dump = "\
pseudo-Boolean objective dump version 1
best_objective_value: 7
best_valid_objective_value: 5
";
    let bounds = parse_objective_dump(dump).expect("should parse");
    assert_eq!(bounds.best.as_deref(), Some("7"));
    assert_eq!(bounds.best_valid.as_deref(), Some("5"));
}

#[test]
fn parses_negative_values() {
    let dump = "\
pseudo-Boolean objective dump version 1
best_objective_value: -3
best_valid_objective_value: -3
";
    let bounds = parse_objective_dump(dump).expect("should parse");
    assert_eq!(bounds.best.as_deref(), Some("-3"));
    assert_eq!(bounds.best_valid.as_deref(), Some("-3"));
}

#[test]
fn parses_none_values() {
    let dump = "\
pseudo-Boolean objective dump version 1
best_objective_value: none
best_valid_objective_value: none
";
    let bounds = parse_objective_dump(dump).expect("should parse");
    assert!(bounds.best.is_none());
    assert!(bounds.best_valid.is_none());
}

#[test]
fn parses_mixed_known_and_none() {
    let dump = "\
pseudo-Boolean objective dump version 1
best_objective_value: 12
best_valid_objective_value: none
";
    let bounds = parse_objective_dump(dump).expect("should parse");
    assert_eq!(bounds.best.as_deref(), Some("12"));
    assert!(bounds.best_valid.is_none());
}

#[test]
fn rejects_wrong_header() {
    let dump = "\
pseudo-Boolean objective dump version 2
best_objective_value: 7
best_valid_objective_value: 5
";
    assert!(parse_objective_dump(dump).is_err());
}

#[test]
fn rejects_empty_dump() {
    assert!(parse_objective_dump("").is_err());
}

#[test]
fn rejects_missing_second_field() {
    let dump = "\
pseudo-Boolean objective dump version 1
best_objective_value: 7
";
    assert!(parse_objective_dump(dump).is_err());
}

#[test]
fn rejects_wrong_field_order() {
    let dump = "\
pseudo-Boolean objective dump version 1
best_valid_objective_value: 5
best_objective_value: 7
";
    assert!(parse_objective_dump(dump).is_err());
}
