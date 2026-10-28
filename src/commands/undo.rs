//! `:undo [n]` — undo the last `n` actions (default 1), and `:redo [n]` —
//! redo the last `n` actions `:undo` undid. An action is whatever
//! `commands::dispatch` makes one undo point for: one bare proof line, one
//! `:delete`, a whole `:source`'d file, an entire `:edit`/`:formula`
//! session, one TUI browse-mode commit, and so on. `:delete <n>-<m>` is the
//! line-numbered way to remove specific lines.

use crate::output::{Output, outln};
use crate::session::Session;

/// Parses the optional count (default 1), printing a usage error for
/// `command` if it isn't a non-negative integer.
fn count(command: &str, args: &str, out: &mut dyn Output) -> Option<usize> {
    let args = args.trim();
    if args.is_empty() {
        return Some(1);
    }
    match args.parse::<usize>() {
        Ok(n) => Some(n),
        Err(_) => {
            outln!(
                out,
                "Error: usage :{command} [n] (n must be a non-negative integer)"
            );
            None
        }
    }
}

pub fn run(session: &mut Session, args: &str, out: &mut dyn Output) {
    let Some(n) = count("undo", args, out) else {
        return;
    };
    match session.undo(n) {
        0 => outln!(out, "(nothing to undo)"),
        undone => outln!(
            out,
            "Undid {undone} action(s); {} proof line(s) remain.",
            session.buffer.len()
        ),
    }
}

pub fn run_redo(session: &mut Session, args: &str, out: &mut dyn Output) {
    let Some(n) = count("redo", args, out) else {
        return;
    };
    match session.redo(n) {
        0 => outln!(out, "(nothing to redo)"),
        redone => outln!(
            out,
            "Redid {redone} action(s); {} proof line(s) now.",
            session.buffer.len()
        ),
    }
}
