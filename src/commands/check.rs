//! `:check [<conclusion>]`:
//!
//! - With an argument, non-destructively tests whether the session would
//!   verify with that real conclusion (`UNSAT`, `SAT`, `BOUNDS 0 10`,
//!   `NONE`, ...), without closing it. See [`Session::dry_run_conclusion`].
//! - With no argument, auto-detects: tries every conclusion that needs no
//!   extra input from the user — `UNSAT`, `SAT`, and, once the session has
//!   an objective with a checked-deletion-safe best value already known,
//!   `BOUNDS v v` for that exact value — and shows whichever one the real
//!   checker actually accepts, so you see the genuine `s VERIFIED ...`
//!   rather than a REPL-guessed hint. Everything else — an unequal
//!   `BOUNDS lo hi`, `ENUMERATION_COMPLETE`/`ENUMERATION_PARTIAL`, ... —
//!   needs numbers only you can supply, so it's never auto-tried; ask for
//!   those explicitly. If nothing auto-detects, reports a REPL-native
//!   "not yet" status instead of running a real (and pointless)
//!   `conclusion NONE` check, which is a proof-format no-op that would
//!   verify trivially either way.

use crate::output::{self, Output, outln};
use crate::session::Session;

/// Conclusions safe to try without any extra input from the user: `UNSAT`
/// and `SAT` always (no witness or bound numbers needed), plus, once the
/// session has an objective and a best value with checked-deletion
/// guarantees is already known (`Context::best_valid_objective_value` —
/// the same field `:objective` reports as "checked-deletion guarantees",
/// and the exact one `conclusion BOUNDS`'s upper-bound check compares
/// against), `BOUNDS v v` for that value. The value is never guessed —
/// it's read straight out of `Context` — and the *lower*-bound half of
/// that conclusion still requires a genuinely derivable proof that the
/// objective can't do better, which `dry_run_conclusion` checks for real,
/// exactly like the other two candidates: this only ever adds a
/// candidate worth trying, never assumes success.
///
/// Shared with `:save`'s own auto-detection when no explicit conclusion
/// is given to save with.
pub(crate) fn auto_detect_candidates(session: &Session) -> Vec<String> {
    let mut candidates = vec!["UNSAT".to_string(), "SAT".to_string()];
    let context = &session.current_checker.context;
    if context.objective.is_some()
        && let Some(value) = &context.best_valid_objective_value
    {
        candidates.push(format!("BOUNDS {value} {value}"));
    }
    candidates
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
            output::error_chain(out, &err);
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
