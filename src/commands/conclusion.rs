//! `:conclusion [<conclusion>|none]` — show, set or clear the session's
//! intended conclusion: the one bare `:check` and `:save` try first (see
//! [`Session::intended_conclusion`]). `:source` sets it from the file's own
//! `conclusion …;` line. Setting it checks nothing; `:check` does.

use crate::output::{Output, outln};
use crate::session::Session;

pub fn run(session: &mut Session, args: &str, out: &mut dyn Output) {
    let args = args.trim();
    if args.is_empty() {
        match &session.intended_conclusion {
            Some(conclusion) => outln!(out, "Intended conclusion: {conclusion}"),
            None => outln!(out, "No intended conclusion."),
        }
        return;
    }
    if args.eq_ignore_ascii_case("none") {
        session.set_intended_conclusion(None);
        outln!(out, "Intended conclusion cleared.");
        return;
    }
    session.set_intended_conclusion(Some(args));
    match &session.intended_conclusion {
        Some(conclusion) => outln!(
            out,
            "Intended conclusion: {conclusion} (:check tests it; bare :save uses it)."
        ),
        None => outln!(out, "Error: usage :conclusion [<conclusion>|none]"),
    }
}
