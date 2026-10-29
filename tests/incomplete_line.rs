//! Tests for proof lines missing their closing `;`: rejected as incomplete
//! rather than accepted with nothing derived (require
//! `$VERIPB_REPL_VERIPB_BIN`; skip otherwise).

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::checker::invoke::invocation_count;
use veripb_repl::session::{AppendOutcome, Session};

macro_rules! require_veripb {
    () => {
        if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
            eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
            return;
        }
    };
}

fn session() -> (NamedTempFile, Session) {
    let mut file = NamedTempFile::with_suffix(".opb").unwrap();
    file.write_all(b"1 x1 1 x2 >= 1 ;\n1 x2 1 x3 >= 1 ;\n")
        .unwrap();
    let session = Session::load(file.path().to_str().unwrap()).unwrap();
    (file, session)
}

#[test]
fn a_typed_line_without_its_semicolon_is_rejected() {
    require_veripb!();
    let (_file, mut session) = session();
    for line in ["pol 1 2 +", "rup 1 x1 1 x2 1 x3 >= 1", "del id 1"] {
        match session.append_line(line).unwrap() {
            AppendOutcome::Rejected { error, .. } => {
                assert!(
                    error.starts_with("the line is incomplete: expected "),
                    "{error}"
                );
            }
            _ => panic!("{line} was not rejected"),
        }
        assert!(session.buffer.is_empty());
    }
    assert!(matches!(
        session.append_line("pol 1 2 + ;").unwrap(),
        AppendOutcome::Verified { .. }
    ));
}

#[test]
fn verify_stops_at_the_incomplete_line_in_one_or_two_runs() {
    require_veripb!();
    let (_file, mut session) = session();
    let mut lines: Vec<String> = (0..200)
        .map(|_| "rup 1 x1 1 x2 >= 1 ;".to_string())
        .collect();
    lines.push("pol 1 2 +".to_string());
    lines.push(String::new());
    lines.push("% the end".to_string());
    for line in lines {
        session.buffer.push(line);
    }
    let before = invocation_count();
    let (_, rejection) = session.drive_forward().unwrap();
    let runs = invocation_count() - before;

    let error = rejection.expect("rejected");
    assert!(error.starts_with("the line is incomplete:"), "{error}");
    assert_eq!(session.checked_len, 200, "stopped at the pol line");
    assert!(runs <= 2, "{runs} runs");
}

#[test]
fn a_proof_without_closing_lines_is_still_accepted() {
    require_veripb!();
    let (_file, mut session) = session();
    for line in [
        "pol 1 2 + ;",
        "",
        "% done",
        "output NONE ;",
        "conclusion NONE ;",
    ] {
        session.buffer.push(line.to_string());
    }
    let (_, rejection) = session.drive_forward().unwrap();
    assert_eq!(rejection, None);
    assert_eq!(session.checked_len, 5);
}
