//! `:load <file>` — load a new formula (OPB only, matching `Session::load`),
//! resetting the whole session: buffer cleared, checker rebuilt from
//! scratch. Also how the very first formula gets loaded if the REPL was
//! started with no formula path on the command line — `session` is `None`
//! until the first successful `:load`. A bad path or parse error leaves the
//! current session (or lack of one) untouched rather than crashing the REPL
//! or half-initializing — same "state unchanged on failure" guarantee as
//! everything else here.

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

    match Session::load(path) {
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
