//! `:check [<conclusion>]`:
//!
//! - With an argument, non-destructively tests whether the session would
//!   verify with that real conclusion (`UNSAT`, `SAT`, `BOUNDS 0 10`,
//!   `NONE`, ...), without closing it. See [`Session::dry_run_conclusion`].
//! - With no argument, auto-detects: tries every conclusion that needs no
//!   extra input from the user and can hold — `UNSAT` and `SAT` without an
//!   objective; with one, `BOUNDS v v` for the checked-deletion-safe best
//!   value once it is known — and shows whichever one the real checker
//!   actually accepts, so you see the genuine `s VERIFIED ...` rather than a
//!   REPL-guessed hint. Everything else — an unequal
//!   `BOUNDS lo hi`, `ENUMERATION_COMPLETE`/`ENUMERATION_PARTIAL`, ... —
//!   needs numbers only you can supply, so it's never auto-tried; ask for
//!   those explicitly. If nothing auto-detects, reports a REPL-native
//!   "not yet" status instead of running a real (and pointless)
//!   `conclusion NONE` check, which is a proof-format no-op that would
//!   verify trivially either way.

use crate::checker;
use crate::output::{self, Output, outln};
use crate::session::Session;

/// Returns the conclusions to try without extra input from the user, most
/// likely first.
///
/// - With an objective, veripb rejects `UNSAT` and `SAT`, so the only one is
///   `BOUNDS v v` once a checked-deletion-safe best value is known
///   (`ObjectiveBounds::best_valid` — the same value `:objective` reports),
///   read straight from the checker, never guessed. A failed
///   `objective_bounds` query (e.g. the resolved `veripb` doesn't support
///   `--dump-objective`) means no candidate — `BOUNDS lo hi` is still
///   reachable by typing it explicitly to `:check`.
/// - Without one, `UNSAT` and `SAT`; `SAT` first if the database is already
///   at hand and holds no constraint that can never hold (`UNSAT` needs
///   one).
///
/// Shared with `:save`'s own auto-detection when no explicit conclusion
/// is given to save with.
pub(crate) fn auto_detect_candidates(session: &Session) -> Vec<String> {
    if session.objective.is_some() {
        return match session.objective_bounds() {
            Ok(bounds) => bounds
                .best_valid
                .map(|value| format!("BOUNDS {value} {value}"))
                .into_iter()
                .collect(),
            Err(_) => Vec::new(),
        };
    }
    let no_contradiction = session.cached_database().is_some_and(|database| {
        !database
            .entries
            .iter()
            .any(|entry| checker::parse::is_contradiction(&entry.text))
    });
    let order = if no_contradiction {
        ["SAT", "UNSAT"]
    } else {
        ["UNSAT", "SAT"]
    };
    order.map(String::from).into()
}

pub fn run(session: &Session, args: &str, out: &mut dyn Output) -> anyhow::Result<()> {
    let args = args.trim().trim_end_matches(';').trim();

    if !args.is_empty() {
        let (captured, result) = session.dry_run_conclusion(args)?;
        // On success the captured text is the checker's own
        // "s VERIFIED ..." line; on failure it's normally empty, but
        // anything the checker did print belongs with the error.
        output::text(out, &captured);
        if let Err(err) = result {
            output::error(out, &err);
        }
        return Ok(());
    }

    for candidate in auto_detect_candidates(session) {
        let (captured, result) = session.dry_run_conclusion(&candidate)?;
        if result.is_ok() {
            output::text(out, &captured);
            return Ok(());
        }
        // A failed probe captures nothing (tracing is scoped off), so
        // dropping it and silently moving on to the next candidate is safe
        // — the user never sees two "it's not this" errors dumped on them.
    }

    outln!(out, "NOT YET CONCLUDED");
    Ok(())
}
