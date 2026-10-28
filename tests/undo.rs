//! Tests for `:undo` and `:redo` through `commands::dispatch`, as both
//! frontends drive them. Only the last test needs `$VERIPB_REPL_VERIPB_BIN`
//! (it skips otherwise).

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::commands::{self, Flow, edit, formula};
use veripb_repl::output::Output;
use veripb_repl::session::Session;

struct Capture(Vec<String>);

impl Output for Capture {
    fn line(&mut self, text: &str) {
        self.0.push(text.to_string());
    }
}

const FORMULA: &str = "1 x1 1 x2 >= 1 ;\n1 ~x1 1 x2 >= 1 ;\n1 ~x2 1 x3 >= 1 ;\n";

fn load(lines: &[&str]) -> Option<Session> {
    let mut file = NamedTempFile::with_suffix(".opb").unwrap();
    file.write_all(FORMULA.as_bytes()).unwrap();
    let mut session = Session::load(file.path().to_str().unwrap()).unwrap();
    for line in lines {
        session.buffer.push(line.to_string());
    }
    Some(session)
}

/// Dispatches each of `commands`, returning the last one's output.
fn run(session: &mut Option<Session>, commands: &[&str]) -> Vec<String> {
    let mut out = Capture(Vec::new());
    for command in commands {
        out.0.clear();
        commands::dispatch(session, command, &mut out).expect("dispatch");
    }
    out.0
}

fn buffer(session: &Option<Session>) -> Vec<String> {
    session.as_ref().unwrap().buffer.to_vec()
}

fn lines(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|t| t.to_string()).collect()
}

// Display line n is buffer index n - 3 (after the two preamble lines).

#[test]
fn undo_and_redo_step_through_actions() {
    let mut session = load(&["a", "b", "c", "d"]);
    session.as_mut().unwrap().checked_len = 4;
    run(&mut session, &[":delete 4", ":delete 3"]);
    assert_eq!(buffer(&session), lines(&["c", "d"]));
    assert_eq!(session.as_ref().unwrap().checked_len, 0);

    run(&mut session, &[":undo"]);
    assert_eq!(buffer(&session), lines(&["a", "c", "d"]));
    assert_eq!(session.as_ref().unwrap().checked_len, 1);
    run(&mut session, &[":undo"]);
    assert_eq!(buffer(&session), lines(&["a", "b", "c", "d"]));
    assert_eq!(session.as_ref().unwrap().checked_len, 4);
    assert_eq!(run(&mut session, &[":undo"]), ["(nothing to undo)"]);

    run(&mut session, &[":redo"]);
    assert_eq!(buffer(&session), lines(&["a", "c", "d"]));
    assert_eq!(session.as_ref().unwrap().checked_len, 1);
    run(&mut session, &[":redo 5"]);
    assert_eq!(buffer(&session), lines(&["c", "d"]));
    assert_eq!(run(&mut session, &[":redo"]), ["(nothing to redo)"]);

    run(&mut session, &[":undo 2"]);
    assert_eq!(buffer(&session), lines(&["a", "b", "c", "d"]));
    assert_eq!(session.as_ref().unwrap().checked_len, 4);
}

#[test]
fn a_new_change_discards_the_redo_history() {
    let mut session = load(&["a", "b", "c"]);
    run(&mut session, &[":delete 3", ":undo", ":delete 5"]);
    assert_eq!(buffer(&session), lines(&["a", "b"]));
    assert_eq!(run(&mut session, &[":redo"]), ["(nothing to redo)"]);
    run(&mut session, &[":undo"]);
    assert_eq!(buffer(&session), lines(&["a", "b", "c"]));
}

#[test]
fn a_plain_frontend_edit_is_one_action() {
    let mut session = load(&["a", "b", "c"]);
    let mut out = Capture(Vec::new());
    let Flow::StartEdit(mut state) =
        commands::dispatch(&mut session, ":edit 3-4", &mut out).expect("dispatch")
    else {
        panic!("expected edit mode: {:?}", out.0);
    };
    let edited = session.as_mut().unwrap();
    for line in ["A", "B", "new", ":done"] {
        edit::handle(edited, &mut state, line, &mut out).expect("edit");
    }
    assert_eq!(buffer(&session), lines(&["A", "B", "new", "c"]));

    run(&mut session, &[":list"]);
    run(&mut session, &[":undo"]);
    assert_eq!(buffer(&session), lines(&["a", "b", "c"]));
    run(&mut session, &[":redo"]);
    assert_eq!(buffer(&session), lines(&["A", "B", "new", "c"]));
}

#[test]
fn source_into_a_full_buffer_can_be_undone_but_reset_cannot() {
    let mut proof = NamedTempFile::with_suffix(".pbp").unwrap();
    writeln!(
        proof,
        "pseudo-Boolean proof version 3.0\nf 3;\nrup 1 x2 >= 1 ;"
    )
    .unwrap();
    let mut session = load(&["a", "b"]);
    run(&mut session, &[":delete 4"]);
    let source = format!(":source {}", proof.path().display());
    run(&mut session, &[&source]);
    assert_eq!(buffer(&session), lines(&["rup 1 x2 >= 1 ;"]));
    run(&mut session, &[":undo"]);
    assert_eq!(buffer(&session), lines(&["a"]));
    assert_eq!(run(&mut session, &[":undo"]), ["(nothing to undo)"]);
    run(&mut session, &[":redo"]);
    assert_eq!(buffer(&session), lines(&["rup 1 x2 >= 1 ;"]));

    run(&mut session, &[":delete 3", ":undo", ":reset"]);
    assert_eq!(run(&mut session, &[":redo"]), ["(nothing to redo)"]);
    assert_eq!(run(&mut session, &[":undo"]), ["(nothing to undo)"]);
}

#[test]
fn undo_restores_an_edited_formula() {
    if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
        eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
        return;
    }
    let mut session = load(&["rup 1 x2 >= 1 ;"]);
    run(&mut session, &[":verify"]);
    assert_eq!(session.as_ref().unwrap().checked_len, 1);
    let original = session.as_ref().unwrap().formula.to_vec();

    let mut out = Capture(Vec::new());
    let Flow::StartFormulaEdit(mut state) =
        commands::dispatch(&mut session, ":formula 3", &mut out).expect("dispatch")
    else {
        panic!("expected formula mode: {:?}", out.0);
    };
    let edited = session.as_mut().unwrap();
    for line in ["1 ~x2 1 x3 1 x1 >= 1 ;", ":done"] {
        formula::handle(edited, &mut state, line, &mut out).expect("formula");
    }
    let changed = session.as_ref().unwrap().formula.to_vec();
    assert_ne!(changed, original);

    run(&mut session, &[":undo"]);
    let restored = session.as_ref().unwrap();
    assert_eq!(restored.formula.to_vec(), original);
    assert_eq!(restored.checked_len, 1);
    assert_eq!(restored.formula_constraint_count(), 3);
    run(&mut session, &[":redo"]);
    assert_eq!(session.as_ref().unwrap().formula.to_vec(), changed);
}
