//! End-to-end tests for `:explain` against a real `veripb` binary. Requires
//! `$VERIPB_REPL_VERIPB_BIN` to point at one; skipped (not failed)
//! otherwise, same as `tests/checker.rs`.

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::commands::explain;
use veripb_repl::output::Output;
use veripb_repl::session::{AppendOutcome, Session};

const FORMULA: &str = "\
* #variable= 2 #constraint= 3
1 x1 1 x2 >= 1 ;
1 ~x1 1 x2 >= 1 ;
1 ~x2 >= 1 ;
";

fn veripb_available() -> bool {
    std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_some()
}

struct Capture(Vec<String>);

impl Output for Capture {
    fn line(&mut self, text: &str) {
        self.0.push(text.to_string());
    }
}

/// Satisfiable (x2 = 1), so unlike [`FORMULA`] not everything is
/// RUP-implied: `1 x2 >= 1` is, `1 x1 >= 1` isn't.
const SAT_FORMULA: &str = "\
* #variable= 2 #constraint= 2
1 x1 1 x2 >= 1 ;
1 ~x1 1 x2 >= 1 ;
";

fn load() -> Session {
    load_formula(FORMULA)
}

fn load_formula(formula: &str) -> Session {
    let mut file = NamedTempFile::with_suffix(".opb").expect("failed to create temp formula file");
    file.write_all(formula.as_bytes())
        .expect("failed to write temp formula file");
    Session::load(file.path().to_str().expect("temp path is valid UTF-8"))
        .expect("failed to load session")
}

fn append_ok(session: &mut Session, line: &str) {
    match session.append_line(line).expect("append should not fail to invoke") {
        AppendOutcome::Verified { .. } => {}
        AppendOutcome::Rejected { error, .. } => panic!("{line:?} was rejected: {error}"),
        AppendOutcome::Deferred => panic!("{line:?} was deferred"),
    }
}

fn explain(session: &Session, args: &str) -> String {
    let mut out = Capture(Vec::new());
    explain::run(session, args, &mut out);
    out.0.join("\n")
}

macro_rules! require_veripb {
    () => {
        if !veripb_available() {
            eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
            return;
        }
    };
}

/// Queues `line` unchecked at the end of the buffer and steps onto it,
/// leaving it rejected and pending (`known_bad` set) — unlike
/// `append_line`, which drops a rejected line.
fn reject_pending(session: &mut Session, line: &str) {
    session
        .insert_line(session.buffer.len(), line)
        .expect("insert should not fail");
    session.step().expect("step should not fail to invoke");
    assert!(session.known_bad.is_some(), "{line:?} was unexpectedly accepted");
}

#[test]
fn bare_explain_with_nothing_checked() {
    require_veripb!();
    let session = load();
    assert_eq!(explain(&session, ""), "Nothing checked yet — nothing to explain.");
}

#[test]
fn bare_explain_targets_the_last_checked_line() {
    require_veripb!();
    let mut session = load();
    append_ok(&mut session, "rup 1 x2 >= 1 ;");
    let text = explain(&session, "");
    assert!(text.contains("Line 3 needed:"), "{text}");
}

#[test]
fn rup_line_lists_the_needed_hints() {
    require_veripb!();
    let mut session = load();
    append_ok(&mut session, "rup 1 x2 >= 1 ;");
    append_ok(&mut session, "pol 3 4 + ;");
    let text = explain(&session, "3");
    assert!(text.contains("Line 3 needed:"), "{text}");
    assert!(text.contains("ConstraintId 3: 1 ~x2 >= 1"), "{text}");
}

#[test]
fn non_rup_line_has_no_needed_hints() {
    require_veripb!();
    let mut session = load();
    append_ok(&mut session, "rup 1 x2 >= 1 ;");
    append_ok(&mut session, "pol 3 4 + ;");
    let text = explain(&session, "4");
    assert!(!text.contains("needed:"), "{text}");
}

#[test]
fn rejected_line_with_a_bad_hint_checks_without_it() {
    require_veripb!();
    let mut session = load();
    reject_pending(&mut session, "rup 1 x2 >= 1 : 99 ;");
    let text = explain(&session, "");
    assert!(
        text.starts_with("Line 3 is rejected: Accessing the database out of bound with index 99."),
        "{text}"
    );
    assert!(!text.contains("Running VeriPB"), "{text}");
    assert!(text.contains("Typed hints: 99"), "{text}");
    assert!(text.contains("ConstraintId 99 is not in the database"), "{text}");
    assert!(text.contains("Without hints it DOES check"), "{text}");
    // `:explain 3` names the rejected line explicitly — same diagnosis.
    assert_eq!(explain(&session, "3"), text);
}

#[test]
fn rejected_line_that_is_not_rup_implied_fails_without_hints_too() {
    require_veripb!();
    let mut session = load_formula(SAT_FORMULA);
    reject_pending(&mut session, "rup 1 x1 >= 1 : 1 ;");
    let text = explain(&session, "");
    assert!(text.contains("Without hints it still fails"), "{text}");
}

#[test]
fn explicit_line_number_wins_over_a_pending_rejection() {
    require_veripb!();
    let mut session = load_formula(SAT_FORMULA);
    append_ok(&mut session, "rup 1 x2 >= 1 ;");
    reject_pending(&mut session, "rup 1 x1 >= 1 : 1 ;");
    let text = explain(&session, "3");
    assert!(text.contains("Line 3 needed:"), "{text}");
    assert!(!text.contains("rejected"), "{text}");
}

#[test]
fn bad_argument_shows_usage() {
    require_veripb!();
    let session = load();
    assert_eq!(explain(&session, "all"), "Error: usage :explain [<n>]");
}
