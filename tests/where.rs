//! Tests for `:where`: a constraint and the line it came from, and the
//! constraints a line derived (require `$VERIPB_REPL_VERIPB_BIN`; skip
//! otherwise).

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::commands;
use veripb_repl::output::Output;
use veripb_repl::session::Session;

macro_rules! require_veripb {
    () => {
        if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
            eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
            return;
        }
    };
}

struct Capture(Vec<String>);

impl Output for Capture {
    fn line(&mut self, text: &str) {
        self.0.push(text.to_string());
    }
}

const FORMULA: &str = "@f1 1 x1 1 x2 >= 1 ;\n1 x2 1 x3 >= 1 ;\n1 x3 1 x4 = 1 ;\n";

/// Display lines 3 to 6; constraint IDs 1-4 are the formula's.
const PROOF: &[&str] = &[
    "@p rup 1 x1 1 x2 1 x3 >= 1 ;",
    "rup 1 x2 1 x3 1 x5 >= 1 ;",
    "del id 6 ;",
    "pol 1 2 + ;",
];

/// A session on `FORMULA` with `PROOF` checked.
fn session() -> (NamedTempFile, Option<Session>) {
    let mut file = NamedTempFile::with_suffix(".opb").unwrap();
    file.write_all(FORMULA.as_bytes()).unwrap();
    let mut session = Session::load(file.path().to_str().unwrap()).unwrap();
    for line in PROOF {
        session.buffer.push(line.to_string());
    }
    let (_, rejection) = session.drive_forward().unwrap();
    assert!(rejection.is_none(), "{rejection:?}");
    (file, Some(session))
}

fn run(session: &mut Option<Session>, command: &str) -> String {
    let mut out = Capture(Vec::new());
    commands::dispatch(session, command, &mut out).expect("dispatch");
    out.0.join("\n")
}

#[test]
fn a_constraint_shows_where_it_came_from() {
    require_veripb!();
    let (_file, mut session) = session();
    assert_eq!(
        run(&mut session, ":where 2"),
        "ConstraintId 2: 1 x2 1 x3 >= 1 [core]\n  from the formula: 1 x2 1 x3 >= 1 ;"
    );
    assert_eq!(
        run(&mut session, ":where @f1"),
        "ConstraintId 1: @f1 1 x1 1 x2 >= 1 [core]\n  from the formula: @f1 1 x1 1 x2 >= 1 ;"
    );
    assert_eq!(
        run(&mut session, ":where @p"),
        "ConstraintId 5: @p 1 x1 1 x2 1 x3 >= 1 [derived]\n  from line 3: @p rup 1 x1 1 x2 1 x3 >= 1 ;"
    );
    let gone = run(&mut session, ":where 6");
    assert!(
        gone.starts_with("ConstraintId 6 — no longer in the database\n  from line 4:"),
        "{gone}"
    );
}

#[test]
fn a_line_shows_what_it_derived() {
    require_veripb!();
    let (_file, mut session) = session();
    let pol = run(&mut session, ":where line 6");
    assert!(
        pol.starts_with("Line 6: pol 1 2 + ;\n  derived ConstraintId 7: "),
        "{pol}"
    );
    assert_eq!(
        run(&mut session, ":where line 5"),
        "Line 5: del id 6 ;\n  derived no constraints"
    );
}

#[test]
fn bad_targets_say_why() {
    require_veripb!();
    let (_file, mut session) = session();
    assert_eq!(run(&mut session, ":where line 99"), "No proof line 99.");
    assert_eq!(run(&mut session, ":where line 2"), "No proof line 2.");
    assert_eq!(
        run(&mut session, ":where 99"),
        "No constraint 99 in the formula or from a checked line."
    );
    assert_eq!(run(&mut session, ":where @nope"), "Error: no label @nope");
    assert_eq!(
        run(&mut session, ":where"),
        "Error: usage :where <id>|@label|line <n>"
    );
    session
        .as_mut()
        .unwrap()
        .buffer
        .push("rup 1 x1 >= 1 ;".to_string());
    assert_eq!(
        run(&mut session, ":where line 7"),
        "Line 7 is not checked yet."
    );
}
