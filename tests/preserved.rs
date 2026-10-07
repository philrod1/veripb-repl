//! End-to-end tests for `:preserved`, using veripb's
//! `add_preserved_var`/`remove_preserved_var` test instances. Requires
//! `$VERIPB_REPL_VERIPB_BIN`; tests skip otherwise.

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::commands::preserved;
use veripb_repl::output::Output;
use veripb_repl::session::{AppendOutcome, Session};

const CONSTRAINTS: &str = "\
1 x1 +1 x2 >= 1 ;
1 x2 +1 x3 >= 1 ;
1 x1 +1 x3 +1 x4 >= 1 ;
1 x1 +1 x3 -1 x4 >= 0 ;
";

/// `x5 <-> x1`, already in the formula — what `preserved_rm x5` needs.
const X5_DEFINITION: &str = "\
1 x1 +1 ~x5 >= 1 ;
1 ~x1 +1 x5 >= 1 ;
";

fn veripb_available() -> bool {
    std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_some()
}

macro_rules! require_veripb {
    () => {
        if !veripb_available() {
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

fn load(formula: &str) -> Session {
    let mut file = NamedTempFile::with_suffix(".opb").expect("failed to create temp formula file");
    file.write_all(formula.as_bytes())
        .expect("failed to write temp formula file");
    Session::load(file.path().to_str().expect("temp path is valid UTF-8"))
        .expect("failed to load session")
}

fn append_ok(session: &mut Session, line: &str) {
    match session
        .append_line(line)
        .expect("append should not fail to invoke")
    {
        AppendOutcome::Verified { .. } => {}
        AppendOutcome::Rejected { error, .. } => panic!("{line:?} was rejected: {error}"),
        AppendOutcome::Deferred => panic!("{line:?} was deferred"),
    }
}

fn run(session: &Session) -> String {
    let mut out = Capture(Vec::new());
    preserved::run(session, &mut out);
    out.0.join("\n")
}

#[test]
fn no_preserved_line() {
    require_veripb!();
    let session = load(CONSTRAINTS);
    assert_eq!(
        run(&session),
        "No preserved set — the formula has no `preserved:` line."
    );
}

#[test]
fn declared_and_unchanged() {
    require_veripb!();
    let session = load(&format!("preserved: x3 x1;\n{CONSTRAINTS}"));
    assert_eq!(
        run(&session),
        "Preserved set (2): x1 x3\n  as declared by the formula"
    );
}

#[test]
fn after_preserved_add() {
    require_veripb!();
    let mut session = load(&format!("preserved: x1 x3;\n{CONSTRAINTS}"));
    append_ok(&mut session, "red 1 ~x5 1 x1 >= 1 : x5 0 ;");
    append_ok(&mut session, "core id -1;");
    append_ok(&mut session, "red 1 x5 1 ~x1 >= 1 : x5 1 ;");
    append_ok(&mut session, "core id -1;");
    append_ok(&mut session, "preserved_add x5 : 1 x1 >= 1 ;");
    assert_eq!(
        run(&session),
        "Preserved set (3): x1 x3 x5\n  last changed by line 7; the formula declared: x1 x3"
    );
}

#[test]
fn after_preserved_rm() {
    require_veripb!();
    let mut session = load(&format!(
        "preserved: x1 x3 x5;\n{CONSTRAINTS}{X5_DEFINITION}"
    ));
    append_ok(&mut session, "preserved_rm x5 : 1 x1 >= 1 ;");
    append_ok(&mut session, "rup 1 x1 1 x2 >= 1 ;");
    assert_eq!(
        run(&session),
        "Preserved set (2): x1 x3\n  last changed by line 3; the formula declared: x1 x3 x5"
    );
}
