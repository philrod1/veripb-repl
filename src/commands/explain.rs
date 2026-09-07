//! `:explain <n>` — show an expanded derivation for proof line `n`: `pol`'s
//! full step-by-step reverse-polish table, `red`'s substitution witness and
//! proofgoal listing, or (every other rule) the same baseline
//! `"ConstraintId N: ..."` line every rule prints when traced. See
//! [`Session::explain_line`] for what's already available per rule and why.
//! For a `rup` line specifically, the needed-hints data `:why` shows is
//! folded in automatically underneath the baseline line — `:explain` is
//! "show me everything already knowable about this line," and that data is
//! knowable now (see [`Session::rup_needed_hints`]), so there's no reason
//! to make `:why` the only way to see it. Never touches the live session —
//! both replay just this one line's worth of work through a fresh,
//! throwaway checker, the same non-destructive pattern `:check` already
//! uses.

use crate::commands::why;
use crate::output::{self, Output, outln};
use crate::session::Session;

pub fn run(session: &Session, args: &str, out: &mut dyn Output) {
    let n: usize = match args.trim().parse() {
        Ok(n) => n,
        Err(_) => {
            outln!(out, "Error: usage :explain <n>");
            return;
        }
    };

    let explanation = match session.explain_line(n) {
        Ok(Ok(text)) => text,
        Ok(Err(msg)) => {
            outln!(out, "Error: {msg}");
            return;
        }
        Err(err) => {
            outln!(out, "Error: {err:#}");
            return;
        }
    };

    let is_rup = session
        .checked_index(n)
        .is_some_and(|idx| why::is_rup_line(&session.buffer[idx]));

    if explanation.trim().is_empty() && !is_rup {
        outln!(
            out,
            "(no trace output for line {n} — nothing more to show beyond :list.)"
        );
        return;
    }
    if !explanation.trim().is_empty() {
        output::text(out, &explanation);
    }

    if is_rup {
        match session.rup_needed_hints(n) {
            Ok(Ok(hints)) => why::print_needed(session, n, &hints, out),
            Ok(Err(msg)) => outln!(out, "Error: {msg}"),
            Err(err) => outln!(out, "Error: {err:#}"),
        }
    }
}
