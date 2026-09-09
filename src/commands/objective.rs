//! `:objective` — show the current objective function.

use crate::output::{Output, outln};
use crate::session::Session;

pub fn run(session: &Session, out: &mut dyn Output) {
    let Some(objective) = &session.objective else {
        outln!(out, "No objective — this is a satisfaction problem.");
        return;
    };
    outln!(out, "Objective: {objective}");
    // Best-value tracking needs live checker state (Context::
    // best_objective_value/best_valid_objective_value) that
    // --dump-database doesn't report. Not implemented.
    outln!(
        out,
        "Best-value tracking isn't available yet — querying it needs a core-binary change \
         this REPL doesn't have yet."
    );
}
