//! Tests for rules split across proof lines: as in veripb, an unfinished
//! rule is no error, and its constraint comes with the line that finishes
//! it, whether checked line by line or all at once (require
//! `$VERIPB_REPL_VERIPB_BIN`; skip otherwise).

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::session::{AppendOutcome, Session};

macro_rules! require_veripb {
    () => {
        if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
            eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
            return;
        }
    };
}

/// A session on two constraints (IDs 1 and 2) with `lines` in the buffer,
/// unchecked; they are display lines 3 on.
fn session(lines: &[&str]) -> (NamedTempFile, Session) {
    let mut file = NamedTempFile::with_suffix(".opb").unwrap();
    file.write_all(b"1 x1 1 x2 >= 1 ;\n1 x2 1 x3 >= 1 ;\n")
        .unwrap();
    let mut session = Session::load(file.path().to_str().unwrap()).unwrap();
    for line in lines {
        session.buffer.push(line.to_string());
    }
    (file, session)
}

fn database_ids(session: &Session) -> Vec<usize> {
    let database = session.database().unwrap();
    database.entries.iter().map(|entry| entry.id).collect()
}

/// Display lines 3-7; the `pol` rule spans 4-6.
const SPLIT: &[&str] = &[
    "rup 1 x1 1 x2 1 x3 >= 1 ;",
    "pol 1 2",
    "+",
    ";",
    "rup 1 x2 1 x3 >= 1 ;",
];

#[test]
fn a_rule_can_be_typed_in_parts() {
    require_veripb!();
    let (_file, mut session) = session(&[]);
    for part in ["pol 1 2", "+"] {
        assert!(
            matches!(
                session.append_line(part).unwrap(),
                AppendOutcome::Verified { .. }
            ),
            "{part}"
        );
        assert!(session.last_step_constraint_ids.is_empty());
    }
    assert!(matches!(
        session.append_line(";").unwrap(),
        AppendOutcome::Verified { .. }
    ));
    assert_eq!(session.last_step_constraint_ids, [3]);
    assert_eq!(session.checked_len, 3);
}

#[test]
fn stepping_through_a_split_rule_matches_verifying_it() {
    require_veripb!();
    let (_file, mut stepped) = session(SPLIT);
    while let Some((_, rejection)) = stepped.step(1).unwrap() {
        assert_eq!(rejection, None);
    }
    assert_eq!(stepped.checked_len, SPLIT.len());
    let derived: Vec<Vec<usize>> = (3..=7)
        .map(|n| stepped.line_constraint_ids(n).unwrap().unwrap())
        .collect();
    assert_eq!(derived, [vec![3], vec![], vec![], vec![4], vec![5]]);

    let (_file, mut verified) = session(SPLIT);
    assert_eq!(verified.drive_forward().unwrap().1, None);
    assert_eq!(database_ids(&stepped), database_ids(&verified));
    assert_eq!(database_ids(&verified), [1, 2, 3, 4, 5]);
}

#[test]
fn stepping_back_over_an_unfinished_rule_keeps_the_constraint_before_it() {
    require_veripb!();
    let (_file, mut session) = session(&["rup 1 x1 1 x2 1 x3 >= 1 ;", "pol 1 2 +"]);
    assert_eq!(session.drive_forward().unwrap().1, None);
    assert_eq!(database_ids(&session), [1, 2, 3]);
    assert!(session.step_back().unwrap());
    assert_eq!(database_ids(&session), [1, 2, 3]);
}
