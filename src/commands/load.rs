//! `:load <file>` — replaces the session with a new one for an OPB formula
//! (see `Session::load_checked`); `session` is `None` until the first
//! successful load. An unreadable file, or a formula veripb rejects, leaves
//! the current session unchanged.

use crate::commands::expand_tilde;
use crate::output::{Output, outln};
use crate::session::Session;

pub fn run(session: &mut Option<Session>, args: &str, out: &mut dyn Output) {
    let path = args.trim();
    if path.is_empty() {
        outln!(out, "Error: usage :load <formula.opb>");
        return;
    }
    let path = &expand_tilde(path);

    match Session::load_checked(path) {
        Ok(new_session) => {
            outln!(
                out,
                "Loaded {} constraints from {path}",
                new_session.formula.len()
            );
            *session = Some(new_session);
        }
        Err(err) => outln!(out, "Error: {err:#}"),
    }
}
