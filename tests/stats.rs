//! Tests for `:stats` (the database's require `$VERIPB_REPL_VERIPB_BIN`;
//! those skip otherwise).

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::commands;
use veripb_repl::output::Output;
use veripb_repl::session::Session;

struct Capture(Vec<String>);

impl Output for Capture {
    fn line(&mut self, text: &str) {
        self.0.push(text.to_string());
    }
}

const FORMULA: &str = "min: 1 x1 2 x2 ;\n@f 1 x1 1 x2 >= 1 ;\n1 x2 1 x3 = 1 ;\n2 x1 3 x3 >= 3 ;\n";

fn session(proof: &[&str]) -> (NamedTempFile, Option<Session>) {
    let mut file = NamedTempFile::with_suffix(".opb").unwrap();
    file.write_all(FORMULA.as_bytes()).unwrap();
    let mut session = Session::load(file.path().to_str().unwrap()).unwrap();
    for line in proof {
        session.buffer.push(line.to_string());
    }
    (file, Some(session))
}

fn run(session: &mut Option<Session>, command: &str) -> Vec<String> {
    let mut out = Capture(Vec::new());
    commands::dispatch(session, command, &mut out).expect("dispatch");
    out.0
}

#[test]
fn formula_stats_count_lines_constraints_and_kinds() {
    let (_file, mut session) = session(&[]);
    assert_eq!(
        run(&mut session, ":stats formula"),
        [
            "Formula: 3 lines (53 B), 4 constraints (1 line loading as two), 3 variables",
            "  labelled lines: 1 (33.3%)",
            "  longest line: 19 B (ID 1)",
            "  objective: 2 terms",
            "  kinds: clause 1 (33.3%), general 1 (33.3%), equality 1 (33.3%)",
            "  terms per line: min 2, median 2, max 2, mean 2.0",
            "  largest coefficient: 3",
        ]
    );
}

#[test]
fn proof_stats_count_rules_and_checked_lines() {
    let (_file, mut session) = session(&["rup 1 x1 1 x2 1 x3 >= 1 ;", "% note", "@q rup >= 0 ;"]);
    session.as_mut().unwrap().checked_len = 2;
    assert_eq!(
        run(&mut session, ":stats proof"),
        [
            "Proof: 3 lines (47 B), 2 checked (66.7%)",
            "  labelled lines: 1 (33.3%); 0 breakpoints",
            "  longest line: 25 B (line 3)",
            "  rule     lines      %  bytes      %  checked",
            "  rup          2  66.7%   40 B  85.1%        1",
            "  comment      1  33.3%    7 B  14.9%        1",
        ]
    );
    assert_eq!(
        run(&mut session, ":stats nope"),
        ["Error: usage :stats [formula|database|proof]"]
    );
}

#[test]
fn database_stats_count_core_derived_and_kinds() {
    if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
        eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
        return;
    }
    let (_file, mut session) = session(&["@p rup 1 x1 1 x2 1 x3 >= 1 ;", "pol 1 2 + ;"]);
    session.as_mut().unwrap().drive_forward().unwrap();
    assert_eq!(
        run(&mut session, ":stats database"),
        [
            "Database: 6 constraints (102 B)",
            "  core: 4 (66.7%), derived: 2 (33.3%)",
            "  from the formula: 4 of its 4 still there (100.0%)",
            "  labelled: 2 (33.3%); highest ID 6",
            "  longest constraint: 19 B (ID 5)",
            "  kinds: clause 4 (66.7%), general 2 (33.3%)",
            "  terms per constraint: min 2, median 2, max 3, mean 2.3",
            "  largest coefficient: 3",
        ]
    );
}

#[test]
fn sizes_use_binary_units() {
    let (_file, mut session) = session(&[]);
    let long = format!("% {}", "x".repeat(3 * 1024 * 1024));
    session.as_mut().unwrap().buffer.push(long);
    let stats = run(&mut session, ":stats proof");
    assert_eq!(stats[0], "Proof: 1 line (3.0 MiB), 0 checked (0.0%)");
    assert_eq!(stats[2], "  longest line: 3.0 MiB (line 3)");
}
