//! `:debug` — stepping/breakpoint mode over the session's `checked_len`
//! boundary. `:step`/`:back` move it one line (forward re-checks, backward
//! only retracts), `:continue`/`:until <n>` run forward to a breakpoint, line
//! or rejection, `:break` manages breakpoints, and `:restart` retracts to the
//! start keeping the buffer. The `edit::READONLY_DURING_EDIT` commands also
//! work.
//!
//! All debug state lives on [`Session`]; [`DebugState`] is only the mode's
//! on/off marker. Line-moving commands push their own undo snapshots (see
//! `with_undo`) because `commands::dispatch` does not run inside a mode;
//! breakpoint changes are not undoable. An empty line is a no-op.

use crate::commands::edit::{self, READONLY_DURING_EDIT};
use crate::commands::help::{self, Topic};
use crate::output::{self, Output, outln};
use crate::session::Session;

/// Marker for an active `:debug` mode; all state lives on [`Session`].
pub struct DebugState;

/// Debug-only commands, in addition to `READONLY_DURING_EDIT` (which callers
/// chain on separately). Not registered in `help::COMMANDS`; uses [`Topic`]
/// for TUI completion. Order sets abbreviation priority (see
/// [`resolve_debug_command`]) and should match `:help debug`.
pub(crate) const MODE_VOCABULARY: &[Topic] = &[
    Topic {
        name: "step",
        usage: ":step [n]",
        summary: "re-check the next n unchecked lines (default 1)",
        details: &[],
    },
    Topic {
        name: "back",
        usage: ":back [n]",
        summary: "retract n checked lines (default 1) — nothing re-verified, just forgotten",
        details: &[],
    },
    Topic {
        name: "continue",
        usage: ":continue",
        summary: "run forward to the next breakpoint or a rejection",
        details: &[],
    },
    Topic {
        name: "until",
        usage: ":until <n>",
        summary: "run forward to line n — a one-off stop, not a standing breakpoint",
        details: &[],
    },
    Topic {
        name: "break",
        usage: ":break [<line>|clear]",
        summary: "list breakpoints, toggle one at line n, or clear them all",
        details: &[],
    },
    Topic {
        name: "restart",
        usage: ":restart",
        summary: "retract all the way back to the start, keeping every line",
        details: &[],
    },
    Topic {
        name: "done",
        usage: ":done",
        summary: "leave debug mode",
        details: &[],
    },
];

/// What the frontend should do after routing one line through [`handle`].
pub enum DebugFlow {
    /// Still debugging.
    Continue,
    /// The mode just ended (`:done`) — normal mode resumes.
    Ended,
}

/// `:debug` — enters debug mode. Returns `None` if the buffer is empty.
pub fn start(session: &Session, out: &mut dyn Output) -> Option<DebugState> {
    if session.buffer.is_empty() {
        outln!(out, "Error: no proof lines yet to debug.");
        return None;
    }
    outln!(
        out,
        "Debugging (prompt becomes debug>) — :step/:back move one line, :continue runs to \
         the next breakpoint or a rejection, :break <n> toggles one. :show/:list/:explain/\
         :objective/:preserved/:check still work. :done (or Esc in the TUI) to leave."
    );
    Some(DebugState)
}

/// Runs `f` and pushes an undo snapshot iff it bumped `session.generation`
/// (the same check `commands::dispatch` does).
fn with_undo<T>(
    session: &mut Session,
    f: impl FnOnce(&mut Session) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let before = (session.generation, session.snapshot());
    let result = f(session)?;
    if session.generation != before.0 {
        session.push_undo(before.1);
    }
    Ok(result)
}

/// Prints the captured checker output, then the rejection, the current
/// position, or "fully checked". Used by every forward-moving command.
fn report_position(
    session: &Session,
    captured: &str,
    rejection: Option<String>,
    out: &mut dyn Output,
) {
    output::text(out, captured);
    if let Some(err) = rejection {
        output::error(out, &err);
        help::print_rejection_hint(out, &session.buffer[session.checked_len]);
        let display_line = session.display_line(session.checked_len);
        outln!(out, "Stopped at line {display_line} — it fails.");
        return;
    }
    if session.checked_len >= session.buffer.len() {
        outln!(out, "Fully checked — nothing left to step through.");
    } else {
        let display_line = session.display_line(session.checked_len);
        outln!(out, "Now at line {display_line} (not yet checked).");
    }
}

/// Parses an optional positive count (default 1). Prints a usage error and
/// returns `None` on invalid input.
fn parse_count(cmd: &str, args: &str, out: &mut dyn Output) -> Option<usize> {
    if args.is_empty() {
        return Some(1);
    }
    match args.parse::<usize>() {
        Ok(n) if n > 0 => Some(n),
        _ => {
            outln!(out, "Error: usage :{cmd} [n] (n a positive number)");
            None
        }
    }
}

/// `:step [n]` — checks up to `n` more lines, stopping at the first rejection
/// or the end of the buffer.
fn step_forward(session: &mut Session, n: usize, out: &mut dyn Output) -> anyhow::Result<()> {
    with_undo(session, |session| {
        let mut captured_all = String::new();
        for _ in 0..n {
            let Some((captured, rejection)) = session.step()? else {
                break;
            };
            captured_all.push_str(&captured);
            if rejection.is_some() {
                report_position(session, &captured_all, rejection, out);
                return Ok(());
            }
        }
        report_position(session, &captured_all, None, out);
        Ok(())
    })
}

/// `:back [n]` — retracts the checked prefix by up to `n` lines without
/// re-verification (see `Session::step_back`).
fn step_backward(session: &mut Session, n: usize, out: &mut dyn Output) -> anyhow::Result<()> {
    with_undo(session, |session| {
        let mut moved = 0;
        for _ in 0..n {
            if !session.step_back()? {
                break;
            }
            moved += 1;
        }
        if moved == 0 {
            outln!(
                out,
                "Already at the very start — nothing to step back from."
            );
        } else if session.checked_len == 0 {
            outln!(
                out,
                "Stepped back {moved} line(s) — back at the very start, nothing \
                          checked yet."
            );
        } else {
            let display_line = session.display_line(session.checked_len);
            outln!(
                out,
                "Stepped back {moved} line(s) — now at line {display_line} (not yet checked)."
            );
        }
        Ok(())
    })
}

/// `:continue` — runs forward to the next breakpoint, the first rejection, or
/// the end of the buffer.
fn continue_cmd(session: &mut Session, out: &mut dyn Output) -> anyhow::Result<()> {
    with_undo(session, |session| {
        let (captured, rejection) = session.continue_run(None)?;
        report_position(session, &captured, rejection, out);
        Ok(())
    })
}

/// `:until <n>` — like `:continue`, but also stops at display line `n`.
fn until_cmd(session: &mut Session, args: &str, out: &mut dyn Output) -> anyhow::Result<()> {
    let n: usize = match args.trim().parse() {
        Ok(n) => n,
        Err(_) => {
            outln!(out, "Error: usage :until <line>");
            return Ok(());
        }
    };
    let Some(idx) = session.buffer_index(n) else {
        outln!(
            out,
            "Error: line {n} is the synthesized preamble or past the end of the proof."
        );
        return Ok(());
    };
    with_undo(session, |session| {
        let (captured, rejection) = session.continue_run(Some(idx))?;
        report_position(session, &captured, rejection, out);
        Ok(())
    })
}

/// `:restart` — retracts the checked prefix to zero, keeping the buffer
/// (unlike `:reset`).
fn restart_cmd(session: &mut Session, out: &mut dyn Output) -> anyhow::Result<()> {
    with_undo(session, |session| {
        session.restart()?;
        outln!(
            out,
            "Restarted — nothing checked; {} line(s) waiting.",
            session.buffer.len()
        );
        Ok(())
    })
}

/// `:break` lists breakpoints, `:break <n>` toggles one at display line `n`,
/// `:break clear` removes all. Not undoable (breakpoints are not proof state).
fn break_cmd(session: &mut Session, args: &str, out: &mut dyn Output) {
    let args = args.trim();
    if args.is_empty() {
        if session.breakpoints.is_empty() {
            outln!(out, "No breakpoints set.");
            return;
        }
        let lines: Vec<String> = session
            .breakpoints
            .iter()
            .map(|&idx| session.display_line(idx).to_string())
            .collect();
        outln!(out, "Breakpoint(s) at line(s): {}.", lines.join(", "));
        return;
    }
    if args == "clear" {
        let n = session.breakpoints.len();
        session.breakpoints.clear();
        outln!(out, "Cleared {n} breakpoint(s).");
        return;
    }
    let n: usize = match args.parse() {
        Ok(n) => n,
        Err(_) => {
            outln!(out, "Error: usage :break [<line>|clear]");
            return;
        }
    };
    let Some(idx) = session.buffer_index(n) else {
        outln!(
            out,
            "Error: line {n} is the synthesized preamble or past the end of the proof."
        );
        return;
    };
    session.toggle_breakpoint(idx);
    if session.breakpoints.contains(&idx) {
        outln!(out, "Breakpoint set at line {n}.");
    } else {
        outln!(out, "Breakpoint cleared at line {n}.");
    }
}

/// Every command name debug mode recognizes, in priority order:
/// `MODE_VOCABULARY` then `READONLY_DURING_EDIT`. The TUI suggestion strip
/// must list them in this same order.
fn debug_command_names() -> impl Iterator<Item = &'static str> {
    MODE_VOCABULARY
        .iter()
        .map(|t| t.name)
        .chain(READONLY_DURING_EDIT.iter().copied())
}

/// Resolves a possibly-abbreviated debug-mode command: an exact match wins,
/// otherwise the first prefix match in [`debug_command_names`] order (so `:c`
/// is `:continue`, `:b` is `:back`). Unlike `commands::resolve_command`,
/// ambiguity is not an error. Errors only if nothing matches.
fn resolve_debug_command(cmd: &str) -> Result<&'static str, String> {
    if let Some(name) = debug_command_names().find(|&n| n == cmd) {
        return Ok(name);
    }
    debug_command_names()
        .find(|n| n.starts_with(cmd))
        .ok_or_else(|| {
            format!("Unknown command ':{cmd}' in debug mode — :help debug for the list.")
        })
}

/// Routes one input line in debug mode; the frontend must not call
/// `commands::dispatch` until this returns `DebugFlow::Ended`.
pub fn handle(
    session: &mut Session,
    _state: &mut DebugState,
    line: &str,
    out: &mut dyn Output,
) -> anyhow::Result<DebugFlow> {
    let line = line.trim();
    // Blank line is a no-op, as at the ordinary prompt.
    if line.is_empty() {
        return Ok(DebugFlow::Continue);
    }
    let Some(rest) = line.strip_prefix(':') else {
        outln!(
            out,
            "Only `:`-commands work in debug mode — :help debug for the list, or :done to \
             leave."
        );
        return Ok(DebugFlow::Continue);
    };
    let mut parts = rest.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let args = parts.next().unwrap_or("").trim();

    let cmd = match resolve_debug_command(cmd) {
        Ok(cmd) => cmd,
        Err(msg) => {
            outln!(out, "{msg}");
            return Ok(DebugFlow::Continue);
        }
    };

    if cmd == "done" {
        outln!(out, "Left debug mode.");
        return Ok(DebugFlow::Ended);
    }
    if READONLY_DURING_EDIT.contains(&cmd) {
        edit::run_readonly(session, cmd, args, out)?;
        return Ok(DebugFlow::Continue);
    }
    match cmd {
        "step" => {
            if let Some(n) = parse_count("step", args, out) {
                step_forward(session, n, out)?;
            }
        }
        "back" => {
            if let Some(n) = parse_count("back", args, out) {
                step_backward(session, n, out)?;
            }
        }
        "continue" => continue_cmd(session, out)?,
        "until" => until_cmd(session, args, out)?,
        "restart" => restart_cmd(session, out)?,
        "break" => break_cmd(session, args, out),
        _ => unreachable!("resolve_debug_command only ever returns a debug_command_names() entry"),
    }
    Ok(DebugFlow::Continue)
}
