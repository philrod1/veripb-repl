//! `:reset` — drop every buffer line, checked or not, keeping the
//! currently loaded formula. Unlike `:load`, which also swaps (or
//! discards) the formula, this only clears the buffer built on top of it.

use crate::output::{Output, outln};
use crate::session::Session;

pub fn run(session: &mut Session, out: &mut dyn Output) {
    let n = session.buffer.len();
    if n == 0 {
        outln!(out, "(no lines to reset)");
        return;
    }

    match session.reset() {
        Ok(()) => outln!(out, "Reset: cleared {n} line(s), formula kept."),
        Err(err) => outln!(out, "Error: {err:#}"),
    }
}
