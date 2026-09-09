//! `:debug` — an interactive stepping/breakpoint mode on top of the same
//! `checked_len` boundary the ordinary prompt and `:verify` already
//! drive: `:step`/`:back` move it one line at a time (forward re-checks
//! the next line for real; backward just retracts — nothing is ever
//! re-verified going backward, there's nothing to check, only to
//! forget), `:continue` runs it forward to the next breakpoint or a
//! rejection, `:until <n>` does the same but to a one-off line instead
//! of a standing breakpoint, `:break` manages which lines are marked,
//! and `:restart` retracts all the way back to the top without touching
//! the buffer at all — the non-destructive sibling of `:reset`. Every
//! read-only inspection command (`:show`/`:list`/`:objective`/`:check`/
//! `:explain`/`:why`) keeps working exactly as it does at the ordinary
//! prompt, since they only ever read `checked_len`/the checker's current
//! state, never caring who moved it there — see
//! `edit::READONLY_DURING_EDIT`, reused as-is.
//!
//! Shaped like `commands::formula`'s mode (`start`/`handle`,
//! [`DebugState`]/[`DebugFlow`]), but with no queue at all: nothing here
//! is "the next thing to visit" the way a formula constraint or a
//! retyped line is — every debug-mode fact (`checked_len`,
//! `breakpoints`, `known_bad`) already lives on [`Session`] itself, so
//! [`DebugState`] exists only as the mode's on/off marker `plain.rs`/
//! `tui::App` hold, mirroring `edit::EditState`/
//! `formula::FormulaEditState`'s own shape.
//!
//! Every line-moving command here does its own undo-stack push (see
//! [`with_undo`]), the same manual bookkeeping the TUI's Vim-mode
//! mutators (`vim_delete_char`, etc.) already do for the identical
//! reason: `commands::dispatch`'s own undo wrapping never runs for
//! anything typed inside a mode (`edit`/`formula`'s own docs explain
//! why) — mirrored here rather than left silently un-undoable, since
//! stepping through a proof and wanting to back out of the last few
//! steps is exactly the kind of thing `:undo` is for. Breakpoints
//! themselves are the one exception, deliberately: see
//! `Session::breakpoints`'s own docs for why.
//!
//! An empty line here is a deliberate no-op, same as at the ordinary
//! prompt — it used to step forward, but that made Enter behave
//! differently on an empty `debug>` line than everywhere else empty
//! Enter is a no-op. In the TUI specifically, Shift+Down/Shift+Up step
//! forward/backward instead — see `tui::run`'s own comment at those key
//! arms — so stepping still doesn't require typing the command out every
//! time; the plain frontend has no equivalent single-keypress shortcut,
//! only the typed `:step`/`:back` commands themselves.

use crate::commands::edit::{self, READONLY_DURING_EDIT};
use crate::commands::help::Topic;
use crate::output::{self, Output, outln};
use crate::session::Session;

/// `:debug` mode's entire state — see the module docs for why there's
/// nothing to actually hold.
pub struct DebugState;

/// The `:`-commands debug mode recognizes beyond the shared read-only
/// set (`READONLY_DURING_EDIT` — `:show`/`:list`/`:objective`/`:check`/
/// `:explain`/`:why`, chained on separately by
/// `tui::App::refresh_candidates` rather than duplicated here, so that
/// allowlist stays the one place deciding which read-only commands work
/// mid-mode). None of these are real dispatchable commands outside
/// `:debug` (`:step`/`:back`/`:continue`/`:until`/`:restart`/`:break`
/// only mean anything while stepping; `:done` only while some mode is
/// active at all), so none are registered in `help::COMMANDS` either —
/// same `Topic` shape as that registry purely so the TUI's Tab
/// completion and suggestion strip can build candidates for these the
/// way they already do for everything else (see
/// `formula::MODE_VOCABULARY`, the direct precedent this mirrors).
/// Ordered to match this module's own doc comment and `:help debug`'s
/// own listing: the line-moving commands first, `:break`/`:restart`
/// after, `:done` last.
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

/// `:debug` — enter debug mode. Refuses only if there's no proof buffer
/// yet to step through at all (an empty buffer has nothing for
/// `checked_len` to even mean).
pub fn start(session: &Session, out: &mut dyn Output) -> Option<DebugState> {
    if session.buffer.is_empty() {
        outln!(out, "Error: no proof lines yet to debug.");
        return None;
    }
    outln!(
        out,
        "Debugging (prompt becomes debug>) — :step/:back move one line, :continue runs to \
         the next breakpoint or a rejection, :break <n> toggles one. :show/:list/:explain/\
         :why/:objective/:check still work. :done (or Esc in the TUI) to leave."
    );
    Some(DebugState)
}

/// Push an undo snapshot iff `f` actually changed something —
/// `commands::dispatch`'s own generation-diff bookkeeping, done by hand
/// here since dispatch never runs for anything typed inside this mode
/// (see the module docs).
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

/// Report where `:step`/`:continue`/`:until` left off: the checker's own
/// captured trace, then either the rejection `:verify` would show, or a
/// plain "now at line N" position report, or "fully checked" once
/// nothing is left. Shared by every forward-moving command below so they
/// all describe their outcome the same way.
fn report_position(session: &Session, captured: &str, rejection: Option<String>, out: &mut dyn Output) {
    output::text(out, captured);
    if let Some(err) = rejection {
        output::error(out, &err);
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

/// Parse `:step`/`:back`'s optional count argument — a positive integer,
/// defaulting to 1 when nothing's given. Prints its own usage error and
/// returns `None` on anything else, so callers can just bail on `None`
/// rather than threading a `Result` through for what's always a REPL-
/// native message, never a real error.
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

/// `:step [n]` (default 1): advance the checked prefix by re-checking
/// the next `n` unchecked lines in turn, the same one-line-at-a-time
/// engine `:verify` uses — stopping early at the first rejection, same
/// as it does, or once nothing is left to step onto.
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

/// `:back [n]` (default 1): retract the checked prefix by `n` lines —
/// nothing is re-verified going backward, only forgotten (see
/// `Session::step_back`) — stopping early once nothing is left to step
/// back from (the very start of the buffer).
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

/// `:continue`: run forward to the next breakpoint or the first
/// rejection, whichever comes first — `Session::continue_run` with no
/// target, so only a real breakpoint or the buffer's own end stops it.
/// Reachable by any abbreviation too, including a bare `:c` (which
/// `resolve_debug_command` prefers over `:check`) — see that function's
/// own docs.
fn continue_cmd(session: &mut Session, out: &mut dyn Output) -> anyhow::Result<()> {
    with_undo(session, |session| {
        let (captured, rejection) = session.continue_run(None)?;
        report_position(session, &captured, rejection, out);
        Ok(())
    })
}

/// `:until <n>`: run forward to display line `n` — a one-off
/// destination, not a permanent breakpoint — same early-stop-at-a-real-
/// breakpoint behavior as `:continue` along the way.
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

/// `:restart`: retract the checked prefix all the way back to the
/// start, keeping every buffer line exactly as it is — the non-
/// destructive sibling of `:reset` (which also clears the buffer).
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

/// `:break` (list every breakpoint), `:break <n>` (toggle one at display
/// line `n`), `:break clear` (drop every breakpoint at once). Never
/// touches `checked_len`/`undo_stack` — a breakpoint isn't proof state
/// (see `Session::breakpoints`'s own docs), so there's nothing here for
/// `:undo` to need to know about.
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

/// Every name debug mode recognizes, in priority order — `MODE_VOCABULARY`'s
/// own line-moving/mode commands first (in that const's own order), then
/// the shared `READONLY_DURING_EDIT` inspection set — for
/// [`resolve_debug_command`] to resolve abbreviations against, and for
/// `tui::App::refresh_candidates`' suggestion strip to list in the same
/// order (see its call site), so the two can never disagree about either
/// what's recognized or what's "first". That shared order is exactly
/// what makes `resolve_debug_command`'s tie-break below correspond to
/// "whatever the suggestion strip already shows on top" rather than an
/// arbitrary pick.
fn debug_command_names() -> impl Iterator<Item = &'static str> {
    MODE_VOCABULARY
        .iter()
        .map(|t| t.name)
        .chain(READONLY_DURING_EDIT.iter().copied())
}

/// Resolve a possibly-abbreviated command name against everything
/// `:debug` mode recognizes: an exact name always wins; otherwise the
/// *first* name in [`debug_command_names`]'s own order that the prefix
/// matches. Unlike `commands::resolve_command` at the ordinary prompt
/// (which errors on an ambiguous prefix rather than guessing), a
/// stepping session is a tight, repetitive loop — `:c` for `:continue`
/// over and over — where a keystroke saved matters more than a
/// vanishingly unlikely wrong guess, and the "guess" is never arbitrary:
/// it's always the line-moving/mode command the prefix could mean, over
/// a read-only inspection one, exactly the priority `MODE_VOCABULARY`
/// coming first in `debug_command_names` already encodes (so `:c` →
/// `:continue`, not `:check`; `:b` → `:back`, not `:break`) — the same
/// preference the suggestion strip's own ordering already shows, so
/// there's no surprise about *which* command a short prefix lands on,
/// just no error stopping it from landing on one at all. Subsumes the
/// old bare `"cont"` alias for `:continue`: `"cont"` is itself just a
/// prefix of it under this same logic, nothing special-cased.
fn resolve_debug_command(cmd: &str) -> Result<&'static str, String> {
    if let Some(name) = debug_command_names().find(|&n| n == cmd) {
        return Ok(name);
    }
    debug_command_names().find(|n| n.starts_with(cmd)).ok_or_else(|| {
        format!("Unknown command ':{cmd}' in debug mode — :help debug for the list.")
    })
}

/// Route one input line while debug mode is active — the sole entry
/// point either frontend needs during one: `commands::dispatch` isn't
/// consulted at all until `DebugFlow::Ended` comes back (mirrors
/// `formula::handle` exactly).
pub fn handle(
    session: &mut Session,
    _state: &mut DebugState,
    line: &str,
    out: &mut dyn Output,
) -> anyhow::Result<DebugFlow> {
    let line = line.trim();
    // A blank line is a deliberate no-op — same as at the ordinary
    // prompt, and unlike `:edit`/`:formula`'s own blank-line handling
    // (see `plain.rs`'s debug-mode branch). It used to step forward
    // instead, but that made Enter behave differently on an empty
    // `debug>` line than everywhere else in the app; the TUI's
    // Shift+Down/Shift+Up (see `tui::run`) are the dedicated shortcuts
    // now, typing `:step`/`:back` out for anyone/anywhere else.
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
