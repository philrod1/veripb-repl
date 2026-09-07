//! `:undo [n]` — undo the last `n` actions (default 1), restoring the
//! session to exactly the state from `n` actions ago. "Action" here means
//! whatever `commands::dispatch` pushed onto `Session::undo_stack` — one
//! bare proof line, one `:delete`, a whole `:source`'d file, an entire
//! `:edit`/`:formula` session, one TUI browse-mode commit, and so on; see
//! `dispatch`'s own docs for exactly what gets pushed and when. Popping
//! more than one entry (`n > 1`) discards the intermediate ones and
//! restores only the state from the final pop — jumping back `n` actions,
//! not replaying through each one.
//!
//! This used to mean something narrower — "remove the last n accepted
//! *lines*" — before the undo stack existed. `:delete <n>-<m>` remains
//! the precise, line-numbered way to remove specific lines; this command
//! is now about undoing recent actions, not addressing lines by count.

use crate::output::{Output, outln};
use crate::session::Session;

pub fn run(session: &mut Session, args: &str, out: &mut dyn Output) {
    let args = args.trim();
    let n = if args.is_empty() {
        1
    } else {
        match args.parse::<usize>() {
            Ok(n) => n,
            Err(_) => {
                outln!(
                    out,
                    "Error: usage :undo [n] (n must be a non-negative integer)"
                );
                return;
            }
        }
    };

    let mut last = None;
    let mut undone = 0;
    for _ in 0..n {
        match session.pop_undo() {
            Some(snapshot) => {
                last = Some(snapshot);
                undone += 1;
            }
            None => break,
        }
    }
    let Some(snapshot) = last else {
        outln!(out, "(nothing to undo)");
        return;
    };

    match session.restore_snapshot(snapshot) {
        Ok(()) => {
            outln!(
                out,
                "Undid {undone} action(s); {} proof line(s) remain.",
                session.buffer.len()
            );
        }
        Err(err) => outln!(out, "Error: {err:#}"),
    }
}
