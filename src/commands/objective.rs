//! `:objective` — show the current objective function and the best bounds
//! proven/logged for it so far, if any.

use crate::output::{Output, outln};
use crate::session::Session;

pub fn run(session: &Session, out: &mut dyn Output) {
    let Some(objective) = &session.objective else {
        outln!(out, "No objective — this is a satisfaction problem.");
        return;
    };
    outln!(out, "Objective: {objective}");

    let bounds = match session.objective_bounds() {
        Ok(bounds) => bounds,
        Err(err) => {
            outln!(out, "Error: {err:#}");
            return;
        }
    };
    match &bounds.best {
        Some(value) => outln!(out, "Best value found so far: {value}"),
        None => outln!(out, "Best value found so far: none logged yet"),
    }
    match &bounds.best_valid {
        Some(value) => outln!(out, "Best value with checked-deletion guarantees: {value}"),
        None => outln!(out, "Best value with checked-deletion guarantees: none yet"),
    }
}
