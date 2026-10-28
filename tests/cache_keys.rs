//! Tests for what the session's caches are keyed on: `ProofBuffer`'s
//! low-water mark and `FormulaLines`' IDs and epoch (no binary needed), and
//! cache invalidation and veripb run counts (require
//! `$VERIPB_REPL_VERIPB_BIN`; skip otherwise).

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::checker::invoke::invocation_count;
use veripb_repl::commands::{self, Flow, formula};
use veripb_repl::output::Output;
use veripb_repl::session::{FormulaLines, ProofBuffer, Session};

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

fn lines(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|t| t.to_string()).collect()
}

fn buffer(texts: &[&str]) -> ProofBuffer {
    ProofBuffer::from(lines(texts))
}

#[test]
fn an_unchanged_buffer_has_no_mark() {
    let buffer = buffer(&["a", "b"]);
    assert_eq!(buffer.take_changed_from(), None);
}

#[test]
fn each_mutator_marks_the_lowest_changed_index() {
    let mut b = buffer(&["a", "b", "c", "d"]);
    b.set(2, "C".to_string());
    assert_eq!(b.take_changed_from(), Some(2));
    assert_eq!(b.take_changed_from(), None, "taking the mark resets it");

    b.insert(1, "x".to_string());
    b.remove(3);
    assert_eq!(
        b.take_changed_from(),
        Some(1),
        "the lowest of several changes"
    );

    b.push("e".to_string());
    assert_eq!(
        b.take_changed_from(),
        Some(4),
        "a push changes only the end"
    );
    b.pop();
    assert_eq!(b.take_changed_from(), Some(4));

    b.extend(lines(&["f", "g"]));
    assert_eq!(b.take_changed_from(), Some(4));

    b.clear();
    assert_eq!(b.take_changed_from(), Some(0));
}

#[test]
fn setting_a_line_to_its_own_text_changes_nothing() {
    let mut b = buffer(&["a", "b"]);
    b.set(1, "b".to_string());
    assert_eq!(b.take_changed_from(), None);
}

#[test]
fn replace_keeps_the_common_prefix() {
    let mut b = buffer(&["a", "b", "c"]);
    let old = b.replace(lines(&["a", "b", "x", "y"]));
    assert_eq!(old, lines(&["a", "b", "c"]));
    assert_eq!(b.take_changed_from(), Some(2));

    b.replace(lines(&["a", "b"]));
    assert_eq!(b.take_changed_from(), Some(2), "dropping the tail");

    b.replace(lines(&["a", "b"]));
    assert_eq!(b.take_changed_from(), None, "identical lines");
}

#[test]
fn formula_ids_count_two_constraint_lines() {
    let formula = FormulaLines::new(lines(&["1 x1 >= 1 ;", "1 x1 1 x2 = 1 ;", "1 x3 >= 1 ;"]));
    assert_eq!(formula.first_ids(), &[1, 2, 4]);
    assert_eq!(formula.constraint_count(), 4);
}

#[test]
fn formula_changes_bump_the_epoch_and_recount() {
    let mut formula = FormulaLines::new(lines(&["1 x1 >= 1 ;", "1 x2 >= 1 ;"]));
    let epoch = formula.epoch();
    let old = formula.set(0, "1 x1 1 x2 = 1 ;".to_string());
    assert_eq!(old, "1 x1 >= 1 ;");
    assert_ne!(formula.epoch(), epoch);
    assert_eq!(formula.first_ids(), &[1, 3]);
    assert_eq!(formula.constraint_count(), 3);

    let epoch = formula.epoch();
    formula.replace(lines(&["1 x1 >= 1 ;"]));
    assert_ne!(formula.epoch(), epoch);
    assert_eq!(formula.constraint_count(), 1);
}

fn load_formula(text: &str) -> Session {
    let mut file = NamedTempFile::with_suffix(".opb").expect("failed to create temp formula file");
    file.write_all(text.as_bytes())
        .expect("failed to write temp formula file");
    Session::load(file.path().to_str().expect("temp path is valid UTF-8"))
        .expect("failed to load session")
}

const FORMULA: &str = "1 x1 1 x2 >= 1 ;\n1 ~x1 1 x2 >= 1 ;\n1 ~x2 1 x3 >= 1 ;\n";

/// The database text of constraint `id`.
fn database_text(session: &Session, id: usize) -> String {
    session
        .database()
        .expect("database")
        .get(id)
        .expect("constraint is live")
        .text
        .clone()
}

#[test]
fn formula_cancel_on_an_empty_buffer_refreshes_the_database() {
    require_veripb!();
    let mut session = Some(load_formula(FORMULA));
    let mut out = Capture(Vec::new());
    let original = database_text(session.as_ref().unwrap(), 1);

    let Flow::StartFormulaEdit(mut state) =
        commands::dispatch(&mut session, ":formula 1", &mut out).expect("dispatch")
    else {
        panic!("expected formula mode: {:?}", out.0);
    };
    let edited = session.as_mut().unwrap();
    formula::handle(edited, &mut state, "2 x1 1 x2 >= 1 ;", &mut out).expect("commit");
    assert_ne!(database_text(edited, 1), original, "{:?}", out.0);

    commands::dispatch(&mut session, ":formula cancel", &mut out).expect("cancel");
    assert_eq!(
        database_text(session.as_ref().unwrap(), 1),
        original,
        "{:?}",
        out.0
    );
}

#[test]
fn a_rejected_verify_runs_veripb_twice() {
    require_veripb!();
    let mut session = load_formula(FORMULA);
    session.buffer.push("rup 1 x2 >= 1 ;".to_string());
    session.buffer.push("rup 1 x1 >= 1 ;".to_string());
    session.buffer.push("rup 1 x3 >= 1 ;".to_string());

    let before = invocation_count();
    let (_, rejection) = session.drive_forward().expect("verify");
    assert!(rejection.is_some());
    assert_eq!(session.checked_len, 1);
    // The batch, then the rejected line on its own for its message.
    assert_eq!(invocation_count() - before, 2);
}
