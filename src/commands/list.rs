//! `:list` — print the synthesized preamble (tagged, since it isn't yours
//! to edit) followed by every buffer line, numbered as one sequence so
//! the numbers here, in the TUI's proof panel, and in the checker's
//! `line N:` trace output all agree. Doesn't touch the checker at all —
//! just `Session::preamble_lines` and the buffer. Lines past `checked_len`
//! are tagged `[unchecked]`; the one `known_bad` names (if any) also gets
//! its failure reason, the plain-mode equivalent of the TUI's Proof-pane
//! error highlight. Any line in `Session::breakpoints` also gets a
//! `[breakpoint]` tag of its own — the plain-mode equivalent of the
//! TUI gutter's ● marker.

use crate::output::{Output, outln};
use crate::session::Session;

pub fn run(session: &Session, out: &mut dyn Output) {
    let mut n = 0;
    for text in session.preamble_lines() {
        n += 1;
        outln!(out, "{n}: {text} [preamble]");
    }
    if session.buffer.is_empty() {
        outln!(out, "(no proof lines yet)");
        return;
    }
    for (idx, line) in session.buffer.iter().enumerate() {
        n += 1;
        let bp = if session.breakpoints.contains(&idx) {
            " [breakpoint]"
        } else {
            ""
        };
        if idx < session.checked_len {
            outln!(out, "{n}: {line}{bp}");
        } else if idx == session.checked_len && session.known_bad.is_some() {
            outln!(
                out,
                "{n}: {line} [unchecked, failed: {}]{bp}",
                session.known_bad.as_deref().unwrap_or_default()
            );
        } else {
            outln!(out, "{n}: {line} [unchecked]{bp}");
        }
    }
}
