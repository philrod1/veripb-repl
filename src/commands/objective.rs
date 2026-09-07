//! `:objective` — show the current objective function and the best bounds
//! proven/logged for it so far, if any.

use crate::output::{Output, outln};
use crate::session::Session;

pub fn run(session: &Session, out: &mut dyn Output) {
    let context = &session.current_checker.context;
    let var_names = &context.var_names;

    let Some(objective) = &context.objective else {
        outln!(out, "No objective — this is a satisfaction problem.");
        return;
    };
    outln!(out, "Objective: {}", objective.to_pretty_string(var_names));

    match &context.best_objective_value {
        Some(value) => outln!(out, "Best value found so far: {value}"),
        None => outln!(out, "Best value found so far: none logged yet"),
    }
    match &context.best_valid_objective_value {
        Some(value) => outln!(out, "Best value with checked-deletion guarantees: {value}"),
        None => outln!(out, "Best value with checked-deletion guarantees: none yet"),
    }
}
