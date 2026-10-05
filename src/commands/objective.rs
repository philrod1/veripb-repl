//! `:objective` — show the current objective function and the best bounds
//! proven/logged for it so far, if any.

use crate::output::{Output, outln};
use crate::session::Session;

pub fn run(session: &Session, out: &mut dyn Output) {
    let Some(original) = &session.objective else {
        outln!(out, "No objective — this is a satisfaction problem.");
        return;
    };

    // `session.objective` is the formula's static text; the checker's
    // dump reflects any `obju` updates applied so far in the proof.
    let bounds = match session.objective_bounds() {
        Ok(bounds) => bounds,
        Err(err) => {
            outln!(out, "Original objective: {original}");
            outln!(out, "Error: {err:#}");
            return;
        }
    };
    match &bounds.objective {
        Some(current) => outln!(out, "Objective: {current}"),
        None => outln!(out, "Objective: {original}"),
    }
    match &bounds.best {
        Some(value) => outln!(out, "Best value found so far: {value}"),
        None => outln!(out, "Best value found so far: none logged yet"),
    }
    match &bounds.best_valid {
        Some(value) => outln!(out, "Best value with checked-deletion guarantees: {value}"),
        None => outln!(out, "Best value with checked-deletion guarantees: none yet"),
    }
}
