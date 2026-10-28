//! Tests for bare `:check`, `:save` and `:objective`: which conclusions are
//! tried, and how many `veripb` runs they make (require
//! `$VERIPB_REPL_VERIPB_BIN`; skip otherwise).

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::checker::invoke::invocation_count;
use veripb_repl::checker::parse::is_contradiction;
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

/// A session on `formula` with `lines` verified.
fn verified(formula: &str, lines: &[&str]) -> (NamedTempFile, Option<Session>) {
    let mut file = NamedTempFile::with_suffix(".opb").unwrap();
    file.write_all(formula.as_bytes()).unwrap();
    let mut session = Session::load(file.path().to_str().unwrap()).unwrap();
    for line in lines {
        session.buffer.push(line.to_string());
    }
    let (_, rejection) = session.drive_forward().unwrap();
    assert!(rejection.is_none(), "{rejection:?}");
    (file, Some(session))
}

/// Dispatches `command`, returning its output and the `veripb` runs it made.
fn run(session: &mut Option<Session>, command: &str) -> (String, usize) {
    let mut out = Capture(Vec::new());
    let before = invocation_count();
    commands::dispatch(session, command, &mut out).expect("dispatch");
    (out.0.join("\n"), invocation_count() - before)
}

const OBJECTIVE: &str = "min: 1 x1 1 x2 ;\n1 x1 1 x2 >= 1 ;\n";

#[test]
fn with_an_objective_only_bounds_is_tried_once() {
    require_veripb!();
    let (_file, mut session) = verified(OBJECTIVE, &["soli x1 ~x2 ;", "rup 1 x1 1 x2 >= 1 ;"]);

    let (output, runs) = run(&mut session, ":check");
    assert!(output.contains("VERIFIED BOUNDS 1"), "{output}");
    assert_eq!(runs, 2, "the bounds, then BOUNDS 1 1");

    let (again, runs) = run(&mut session, ":check");
    assert_eq!((again.as_str(), runs), (output.as_str(), 0));
    let (objective, runs) = run(&mut session, ":objective");
    assert!(objective.contains("guarantees: 1"), "{objective}");
    assert_eq!(runs, 0);

    let saved = NamedTempFile::with_suffix(".pbp").unwrap();
    let (_, runs) = run(&mut session, &format!(":save {}", saved.path().display()));
    assert_eq!(runs, 0);
    let text = std::fs::read_to_string(saved.path()).unwrap();
    assert!(text.contains("conclusion BOUNDS 1 1;"), "{text}");
}

#[test]
fn with_an_objective_and_no_solution_nothing_is_tried() {
    require_veripb!();
    let (_file, mut session) = verified(OBJECTIVE, &["rup 1 x1 1 x2 >= 1 ;"]);
    let (output, runs) = run(&mut session, ":check");
    assert_eq!(output, "NOT YET CONCLUDED");
    assert_eq!(runs, 1, "the bounds only");
}

#[test]
fn a_cached_database_puts_the_likely_conclusion_first() {
    require_veripb!();
    let (_file, mut session) = verified("1 x1 >= 1 ;\n1 ~x1 >= 1 ;\n", &["rup >= 1 ;"]);
    run(&mut session, ":show");
    let (output, runs) = run(&mut session, ":check");
    assert!(output.contains("VERIFIED UNSAT"), "{output}");
    assert_eq!(runs, 1);

    let (_file, mut session) = verified("1 x1 1 x2 >= 1 ;\n", &["sol x1 ~x2 ;"]);
    run(&mut session, ":show");
    let (output, runs) = run(&mut session, ":check");
    assert!(output.contains("VERIFIED SAT"), "{output}");
    assert_eq!(runs, 1);
}

#[test]
fn a_change_reruns_the_check() {
    require_veripb!();
    let (_file, mut session) = verified("1 x1 1 x2 >= 1 ;\n", &["sol x1 ~x2 ;"]);
    run(&mut session, ":check");
    run(&mut session, ":delete 3");
    let (output, runs) = run(&mut session, ":check");
    assert_eq!(output, "NOT YET CONCLUDED");
    assert!(runs > 0);
}

#[test]
fn contradictions_are_constraints_that_can_never_hold() {
    assert!(is_contradiction(">= 1"));
    assert!(is_contradiction("1 x1 1 ~x2 >= 3"));
    assert!(!is_contradiction("1 x1 1 ~x2 >= 2"));
    assert!(!is_contradiction(">= 0"));
    assert!(!is_contradiction(
        "1 x1 >= 99999999999999999999999999999999999999999"
    ));
    assert!(!is_contradiction("1 x1 = 2"));
}

#[test]
fn a_verify_while_the_database_is_in_use_fills_the_bounds_too() {
    require_veripb!();
    let (_file, mut session) = verified(OBJECTIVE, &["rup 1 x1 1 x2 >= 1 ;"]);
    run(&mut session, ":show");
    let (_, runs) = run(&mut session, "soli x1 ~x2 ;");
    assert_eq!(runs, 1);
    let (objective, runs) = run(&mut session, ":objective");
    assert!(objective.contains("guarantees: 1"), "{objective}");
    assert_eq!(runs, 0);
    let (output, runs) = run(&mut session, ":check");
    assert!(output.contains("VERIFIED BOUNDS 1"), "{output}");
    assert_eq!(runs, 1);
}
