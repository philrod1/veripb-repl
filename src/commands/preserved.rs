//! `:preserved` — prints the current preserved variable set (the formula's
//! declaration as modified by checked `preserved_add`/`preserved_rm` steps;
//! see [`Session::preserved_set`]).

use crate::output::{Output, outln};
use crate::session::Session;

pub fn run(session: &Session, out: &mut dyn Output) {
    let preserved = match session.preserved_set() {
        Ok(preserved) => preserved,
        Err(err) => {
            if let Some(line) = &session.preserved {
                outln!(out, "Declared preserved set: {line}");
            }
            outln!(out, "Error: {err:#}");
            return;
        }
    };

    let Some(current) = &preserved.current else {
        outln!(
            out,
            "No preserved set — the formula has no `preserved:` line."
        );
        return;
    };
    outln!(out, "Preserved set ({}): {}", current.len(), names(current));
    match (preserved.last_change, &preserved.declared) {
        (None, _) => outln!(out, "  as declared by the formula"),
        (Some(line), Some(declared)) => outln!(
            out,
            "  last changed by line {line}; the formula declared: {}",
            names(declared)
        ),
        (Some(line), None) => outln!(out, "  last changed by line {line}"),
    }
}

fn names(names: &[String]) -> String {
    if names.is_empty() {
        "(empty)".to_string()
    } else {
        names.join(" ")
    }
}
