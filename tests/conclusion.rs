//! Tests for the intended conclusion: kept by `:source`, set with
//! `:conclusion`, tried first by bare `:check` and `:save`, restored by
//! `:undo`. The `veripb` runs need `$VERIPB_REPL_VERIPB_BIN` (those tests
//! skip otherwise).

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::checker::invoke::invocation_count;
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

const OBJECTIVE: &str = "min: 1 x1 1 x2 ;\n1 x1 1 x2 >= 1 ;\n";

fn file(suffix: &str, text: &str) -> NamedTempFile {
    let mut file = NamedTempFile::with_suffix(suffix).unwrap();
    file.write_all(text.as_bytes()).unwrap();
    file
}

fn load(formula: &NamedTempFile) -> Option<Session> {
    Some(Session::load(formula.path().to_str().unwrap()).unwrap())
}

/// A proof of `lines` with the closing sequence for `conclusion`.
fn proof(lines: &[&str], conclusion: &str) -> NamedTempFile {
    let mut text = String::from("pseudo-Boolean proof version 3.0\nf 1;\n");
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str(&format!(
        "output NONE;\nconclusion {conclusion};\nend pseudo-Boolean proof;\n"
    ));
    file(".pbp", &text)
}

/// Dispatches `command`, returning its output and the `veripb` runs it made.
fn run(session: &mut Option<Session>, command: &str) -> (String, usize) {
    let mut out = Capture(Vec::new());
    let before = invocation_count();
    commands::dispatch(session, command, &mut out).expect("dispatch");
    (out.0.join("\n"), invocation_count() - before)
}

fn intended(session: &Option<Session>) -> Option<String> {
    session.as_ref().unwrap().intended_conclusion.clone()
}

#[test]
fn conclusion_shows_sets_and_clears() {
    let formula = file(".opb", OBJECTIVE);
    let mut session = load(&formula);
    assert_eq!(
        run(&mut session, ":conclusion").0,
        "No intended conclusion."
    );
    run(&mut session, ":conclusion conclusion  BOUNDS 1   1 ;");
    assert_eq!(intended(&session).as_deref(), Some("BOUNDS 1 1"));
    assert_eq!(
        run(&mut session, ":conclusion").0,
        "Intended conclusion: BOUNDS 1 1"
    );
    run(&mut session, ":conclusion none");
    assert_eq!(intended(&session), None);

    run(&mut session, ":conclusion UNSAT");
    run(&mut session, ":conclusion SAT");
    run(&mut session, ":undo");
    assert_eq!(intended(&session).as_deref(), Some("UNSAT"));
    run(&mut session, ":redo");
    assert_eq!(intended(&session).as_deref(), Some("SAT"));
    session
        .as_mut()
        .unwrap()
        .buffer
        .push("rup 1 x1 1 x2 >= 1 ;".to_string());
    run(&mut session, ":reset");
    assert_eq!(intended(&session), None);
}

#[test]
fn help_shows_the_command_and_the_rule() {
    let formula = file(".opb", OBJECTIVE);
    let mut session = load(&formula);
    let (help, _) = run(&mut session, ":help conclusion");
    assert!(help.starts_with(":conclusion"), "{help}");
    assert!(help.contains("\nconclusion NONE|SAT|UNSAT"), "{help}");
}

#[test]
fn source_keeps_the_files_conclusion_and_check_tries_it_first() {
    require_veripb!();
    let formula = file(".opb", OBJECTIVE);
    let mut session = load(&formula);
    let proof = proof(&["soli x1 ~x2 ;", "rup 1 x1 1 x2 >= 1 ;"], "BOUNDS 1 1");
    let (loaded, _) = run(&mut session, &format!(":source {}", proof.path().display()));
    assert!(
        loaded.contains("Intended conclusion: BOUNDS 1 1"),
        "{loaded}"
    );
    assert_eq!(intended(&session).as_deref(), Some("BOUNDS 1 1"));
    run(&mut session, ":verify");

    let (output, runs) = run(&mut session, ":check");
    assert!(output.contains("VERIFIED BOUNDS 1"), "{output}");
    assert_eq!(runs, 1, "no run for the bounds");

    run(&mut session, ":undo 2");
    assert_eq!(
        intended(&session),
        None,
        "undoing the :verify and the :source"
    );
}

#[test]
fn a_failing_intended_conclusion_falls_back_to_the_others() {
    require_veripb!();
    let formula = file(".opb", OBJECTIVE);
    let mut session = load(&formula);
    let proof = proof(&["soli x1 ~x2 ;", "rup 1 x1 1 x2 >= 1 ;"], "BOUNDS 0 0");
    run(&mut session, &format!(":source {}", proof.path().display()));
    run(&mut session, ":verify");
    let (output, _) = run(&mut session, ":check");
    assert!(
        output.contains("intended conclusion BOUNDS 0 0 does not verify"),
        "{output}"
    );
    assert!(output.contains("VERIFIED BOUNDS 1"), "{output}");
}

#[test]
fn save_keeps_an_unverified_intended_conclusion() {
    require_veripb!();
    let formula = file(".opb", OBJECTIVE);
    let mut session = load(&formula);
    let proof = proof(&["rup 1 x1 1 x2 >= 1 ;"], "BOUNDS 1 1");
    run(&mut session, &format!(":source {}", proof.path().display()));
    run(&mut session, ":verify");
    let saved = NamedTempFile::with_suffix(".pbp").unwrap();
    let (output, _) = run(&mut session, &format!(":save {}", saved.path().display()));
    assert!(
        output.contains("saving with the intended conclusion"),
        "{output}"
    );
    let text = std::fs::read_to_string(saved.path()).unwrap();
    assert!(text.contains("conclusion BOUNDS 1 1;"), "{text}");
}
