//! Tests for `veripb` runs with a progress reporter installed, which hand
//! the proof over through a named pipe (requires `$VERIPB_REPL_VERIPB_BIN`;
//! skips otherwise).

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::NamedTempFile;
use veripb_repl::checker::progress::{self, Progress};
use veripb_repl::checker::{self, CheckOutcome, invoke};

macro_rules! require_veripb {
    () => {
        if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
            eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
            return;
        }
    };
}

/// A chain x1 -> x2 -> ... -> x<n> with ~x<n>, so `rup 1 ~x<i> >= 1 ;` holds
/// for every i, derived from n down.
fn chain_formula(n: usize) -> NamedTempFile {
    let mut file = NamedTempFile::with_suffix(".opb").unwrap();
    for i in 1..n {
        writeln!(file, "1 ~x{i} 1 x{} >= 1 ;", i + 1).unwrap();
    }
    writeln!(file, "1 ~x{n} >= 1 ;").unwrap();
    file
}

fn chain_proof(n: usize, bad_at: Option<usize>) -> String {
    let mut text = format!("pseudo-Boolean proof version 3.0\nf {n};\n");
    for (k, i) in (1..n).rev().enumerate() {
        if bad_at == Some(k) {
            text.push_str("rup 1 x1 >= 1 ;\n");
        } else {
            text.push_str(&format!("rup 1 ~x{i} >= 1 ;\n"));
        }
    }
    text
}

/// Installs a reporter recording every report, runs `f`, and removes it.
fn with_reports<T>(f: impl FnOnce() -> T) -> (T, Vec<Progress>) {
    let reports = Arc::new(Mutex::new(Vec::new()));
    let sink = reports.clone();
    progress::set_reporter(Some(Arc::new(move |p| sink.lock().unwrap().push(p))));
    let result = f();
    progress::set_reporter(None);
    let reports = reports.lock().unwrap().clone();
    (result, reports)
}

fn describe(outcome: &CheckOutcome) -> String {
    match outcome {
        CheckOutcome::Accepted { .. } => "accepted".to_string(),
        CheckOutcome::Rejected { line, message, .. } => format!("rejected at {line}: {message}"),
    }
}

#[test]
fn a_piped_run_matches_a_file_run() {
    require_veripb!();
    let formula = chain_formula(3000);
    for bad_at in [None, Some(2500)] {
        let proof = chain_proof(3000, bad_at);
        let plain = checker::check(formula.path(), &proof, None).unwrap();
        let (piped, reports) =
            with_reports(|| checker::check(formula.path(), &proof, None).unwrap());
        assert_eq!(describe(&piped), describe(&plain), "bad line {bad_at:?}");
        assert!(reports.first().is_some_and(|p| p.started), "{reports:?}");
        assert_eq!(reports.iter().filter(|p| p.started).count(), 1);
        let total = proof.lines().count();
        assert!(reports.iter().all(|p| p.total == total && p.line <= total));
        assert!(reports.windows(2).all(|w| w[0].line <= w[1].line));
    }
}

#[test]
fn a_piped_run_dumps_the_same_database() {
    require_veripb!();
    let formula = chain_formula(50);
    let proof = chain_proof(50, None);
    let entries = |db: &checker::Database| {
        db.entries
            .iter()
            .map(|e| (e.id, e.text.clone()))
            .collect::<Vec<_>>()
    };
    let plain = checker::show_database(formula.path(), &proof).unwrap();
    let (piped, _) = with_reports(|| checker::show_database(formula.path(), &proof).unwrap());
    assert_eq!(entries(&piped), entries(&plain));
}

#[test]
fn a_formula_error_is_reported_through_a_pipe_too() {
    require_veripb!();
    let mut formula = NamedTempFile::with_suffix(".opb").unwrap();
    writeln!(formula, "1 x1 x2 >= ;").unwrap();
    let proof = "pseudo-Boolean proof version 3.0\nf 1;\n";
    let plain = checker::check(formula.path(), proof, None)
        .err()
        .map(|e| format!("{e:#}"));
    let (piped, _) = with_reports(|| {
        checker::check(formula.path(), proof, None)
            .err()
            .map(|e| format!("{e:#}"))
    });
    assert!(plain.is_some());
    assert_eq!(piped, plain);
}

/// Runs `f` on another thread with a reporter installed there, failing if
/// it takes longer than `limit`.
fn finishes_within<T: Send + 'static>(
    limit: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (result, _) = with_reports(f);
        let _ = tx.send(result);
    });
    rx.recv_timeout(limit).expect("the run hung")
}

#[test]
fn veripb_exiting_before_it_opens_the_pipe_does_not_hang() {
    require_veripb!();
    let formula = chain_formula(3000);
    let path = formula.path().to_path_buf();
    let proof = chain_proof(3000, None);
    let raw = finishes_within(Duration::from_secs(30), move || {
        invoke::run(&path, &proof, ["--no-such-flag"]).unwrap()
    });
    assert!(!raw.success);
}

#[test]
fn veripb_stopping_early_does_not_hang() {
    require_veripb!();
    let formula = chain_formula(20_000);
    let path = formula.path().to_path_buf();
    // Rejected at its first line, with most of the proof still to send.
    let proof = chain_proof(20_000, Some(0));
    let outcome = finishes_within(Duration::from_secs(30), move || {
        checker::check(&path, &proof, None).unwrap()
    });
    assert!(
        matches!(outcome, CheckOutcome::Rejected { line: 3, .. }),
        "{}",
        describe(&outcome)
    );
}

#[test]
fn a_session_verifies_the_same_through_a_pipe() {
    require_veripb!();
    let formula = chain_formula(3000);
    let proof = chain_proof(3000, Some(2000));
    let load = || {
        let mut session =
            veripb_repl::session::Session::load(formula.path().to_str().unwrap()).unwrap();
        for line in proof.lines().skip(2) {
            session.buffer.push(line.to_string());
        }
        session
    };
    let mut plain = load();
    let plain_rejection = plain.drive_forward().unwrap().1;
    let mut piped = load();
    let (piped_rejection, reports) = with_reports(|| piped.drive_forward().unwrap().1);
    assert!(plain_rejection.is_some());
    assert_eq!(piped_rejection, plain_rejection);
    assert_eq!(piped.checked_len, plain.checked_len);
    assert!(reports.iter().any(|p| p.started && p.total == 2 + 2999));
}
