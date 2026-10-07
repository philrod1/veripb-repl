//! Tests for `veripb_repl::commands::help::rejection_hint` (no binary
//! needed), plus an end-to-end check that typing a rejected line prints it
//! (requires `$VERIPB_REPL_VERIPB_BIN`; skips otherwise).

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::commands::{self, help::rejection_hint};
use veripb_repl::output::Output;
use veripb_repl::session::Session;

const SHRUNK_HINT: &str =
    "Hint: *<variable> (a shrunk variable) is only allowed in solx — see :help solx.";

#[test]
fn shrunk_variable_outside_solx() {
    assert_eq!(rejection_hint("soli x1 *x2;"), Some(SHRUNK_HINT));
    assert_eq!(rejection_hint("sol *x1 ;"), Some(SHRUNK_HINT));
    assert_eq!(rejection_hint("@l sol x1 *x2 ;"), Some(SHRUNK_HINT));
}

#[test]
fn no_hint_otherwise() {
    assert_eq!(rejection_hint("solx *x1 x2 ;"), None);
    assert_eq!(rejection_hint("soli x1 ~x2 ;"), None);
    assert_eq!(rejection_hint("rup >= 1 ;"), None);
    assert_eq!(rejection_hint(""), None);
}

struct Capture(Vec<String>);

impl Output for Capture {
    fn line(&mut self, text: &str) {
        self.0.push(text.to_string());
    }
}

/// veripb's `incorrect/version3/solution_shrunk_soli` instance.
#[test]
fn typed_rejected_line_prints_the_hint() {
    if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
        eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
        return;
    }
    let mut file = NamedTempFile::with_suffix(".opb").expect("failed to create temp formula file");
    file.write_all(b"* #variable= 2 #constraint= 1\nmin: 1 x2 ;\n1 x1 >= 1 ;\n")
        .expect("failed to write temp formula file");
    let mut session = Some(
        Session::load(file.path().to_str().expect("temp path is valid UTF-8"))
            .expect("failed to load session"),
    );

    let mut out = Capture(Vec::new());
    commands::dispatch(&mut session, "soli x1 *x2;", &mut out).expect("dispatch failed");
    let text = out.0.join("\n");
    assert!(text.contains("found `*`"), "{text}");
    assert!(text.contains(SHRUNK_HINT), "{text}");
    assert!(text.contains("(line rejected, state unchanged)"), "{text}");
}
