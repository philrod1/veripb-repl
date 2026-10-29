//! Tests for the limit on `veripb`'s elaboration, which `:explain` uses to
//! find the hints a `rup` line needed: on a long chain it grows with the
//! square of the proof's length (requires `$VERIPB_REPL_VERIPB_BIN`; skips
//! otherwise).

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::checker::{Elaboration, elaborate_rup_within};

/// A chain `x1 -> x2 -> ... -> xn` with `~xn`, and a proof deriving `~xk`
/// downwards whose elaboration grows with the square of `n`: about 9.6 MB
/// for 2000.
fn chain(n: usize) -> (NamedTempFile, String) {
    let mut formula = NamedTempFile::with_suffix(".opb").unwrap();
    for i in 1..n {
        writeln!(formula, "@c{i} 1 ~x{i} 1 x{} >= 1 ;", i + 1).unwrap();
    }
    writeln!(formula, "1 ~x{n} >= 1 ;").unwrap();
    let mut proof = format!("pseudo-Boolean proof version 3.0\nf {n} ;\n");
    for k in (1..n).rev() {
        proof.push_str(&format!("rup 1 ~x{k} >= 1 ;\n"));
        if k + 1 < n {
            proof.push_str(&format!("pol @c{k} @c{} + ;\n", k + 1));
        }
    }
    (formula, proof)
}

/// The temp directory's entries.
fn temp_entries() -> std::collections::BTreeSet<std::ffi::OsString> {
    std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(|entry| Some(entry.ok()?.file_name()))
        .collect()
}

#[test]
fn an_elaboration_past_its_limit_is_stopped_and_removed() {
    if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
        eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
        return;
    }
    let (formula, proof) = chain(2000);
    let before = temp_entries();

    let started = std::time::Instant::now();
    let stopped = elaborate_rup_within(formula.path(), proof.as_str(), 1024 * 1024).unwrap();
    assert_eq!(stopped, Elaboration::TooLarge);
    let stopped_after = started.elapsed();

    let full = elaborate_rup_within(formula.path(), proof.as_str(), u64::MAX).unwrap();
    assert!(
        matches!(&full, Elaboration::Hints(Some(hints)) if !hints.is_empty()),
        "{full:?}"
    );
    assert!(
        stopped_after < started.elapsed() - stopped_after,
        "stopping early saves time: {stopped_after:?} of {:?}",
        started.elapsed()
    );
    assert_eq!(temp_entries(), before, "no temp files left behind");
}
