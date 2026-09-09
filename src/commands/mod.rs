//! One module per `:`-prefixed REPL command with real logic, plus the
//! [`dispatch`] entry point every frontend feeds submitted lines through —
//! shared so the plain CLI and the TUI can't drift apart on command
//! behavior. Only `:quit` stays as an inline arm in `dispatch` rather than
//! earning a file of its own. Commands never print — they emit through the
//! `Output` sink they're handed, so the same command code drives every
//! frontend.

pub mod check;
pub mod debug;
pub mod edit;
pub mod explain;
pub mod formula;
pub mod help;
pub mod instance;
pub mod list;
pub mod load;
pub mod objective;
pub mod reset;
pub mod save;
pub mod show;
pub mod source;
pub mod theme;
pub mod undo;
pub mod verify;
pub mod why;

use crate::output::{self, Output, outln};
use crate::session::{AppendOutcome, Session};

/// What the frontend's loop should do after a dispatched line.
pub enum Flow {
    Continue,
    Quit,
    /// `:edit` was invoked successfully — the frontend should switch to
    /// routing subsequent lines through `edit::handle` instead of
    /// `dispatch`, until it reports `EditFlow::Ended`.
    StartEdit(edit::EditState),
    /// `:formula` (mode entry, not `:formula cancel`) was invoked
    /// successfully — route subsequent lines through `formula::handle`
    /// instead of `dispatch`, until it reports `FormulaEditFlow::Ended`.
    /// The TUI never actually receives this: it intercepts `:formula`
    /// ahead of `dispatch`, exactly like `:edit` (see `tui::App::execute`).
    StartFormulaEdit(formula::FormulaEditState),
    /// `:debug` was invoked successfully — route subsequent lines through
    /// `debug::handle` instead of `dispatch`, until it reports
    /// `DebugFlow::Ended`. Unlike `:edit`/`:formula`, the TUI genuinely
    /// receives this one — it has no custom browse UI of its own for
    /// debug mode, just the same `debug>` prompt the plain frontend
    /// shows (see `tui::App::execute`).
    StartDebug(debug::DebugState),
}

/// `~` or `~/rest` → the user's home directory: the shell isn't around to
/// do this inside the REPL, so every path-taking command (and the TUI's
/// path completion) shares this one expansion. Other forms (`~user`) and
/// tilde-free paths pass through untouched.
pub(crate) fn expand_tilde(path: &str) -> String {
    let Ok(home) = std::env::var("HOME") else {
        return path.to_string();
    };
    if path == "~" {
        return home;
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return format!("{home}/{rest}");
    }
    path.to_string()
}

/// Load the startup formula (if a path was given) and announce the result
/// through `out` — shared by both frontends so their opening lines match.
/// A bad startup path is a hard error (the user asked for exactly this
/// file), unlike a failing `:load` mid-session, which is recoverable.
pub fn startup(
    formula_path: Option<&str>,
    out: &mut dyn Output,
) -> anyhow::Result<Option<Session>> {
    match formula_path {
        Some(path) => {
            let path = &expand_tilde(path);
            let session = Session::load(path)?;
            outln!(
                out,
                "Loaded {} constraints from {path}",
                session.formula.len()
            );
            Ok(Some(session))
        }
        None => {
            outln!(
                out,
                "No formula loaded — use :load <formula.opb> to get started."
            );
            Ok(None)
        }
    }
}

/// Resolve a possibly-abbreviated command name against `help::COMMANDS`:
/// an exact name always wins; otherwise a prefix matching exactly one
/// command is that command (`:q` → `:quit`). An ambiguous prefix errors
/// listing its candidates rather than guessing. The registry is the
/// gatekeeper: a command that isn't in it can't be dispatched at all.
/// `pub(crate)` so the TUI can check "does this resolve to `:edit`?"
/// itself, ahead of `dispatch`, without duplicating the abbreviation
/// logic — see `tui::App::execute`.
pub(crate) fn resolve_command(cmd: &str) -> Result<&'static str, String> {
    if cmd.is_empty() {
        return Err("Type :help for the list of commands.".to_string());
    }
    if let Some(topic) = help::COMMANDS.iter().find(|t| t.name == cmd) {
        return Ok(topic.name);
    }
    let matches = help::command_matches(cmd);
    match matches.len() {
        1 => Ok(matches[0].name),
        0 => Err(format!("Unknown command ':{cmd}'.")),
        _ => {
            let names: Vec<String> = matches.iter().map(|t| format!(":{}", t.name)).collect();
            Err(format!(
                "Ambiguous command ':{cmd}' — could be {}.",
                names.join(", ")
            ))
        }
    }
}

/// Dispatch one non-empty input line — a `:`-command or a bare proof rule —
/// against the session, emitting all user-visible text through `out`.
/// Tracks undo-stack bookkeeping around [`dispatch_inner`], which holds
/// the actual command logic (unchanged behavior, just renamed) — see that
/// function's own docs.
pub fn dispatch(
    session: &mut Option<Session>,
    line: &str,
    out: &mut dyn Output,
) -> anyhow::Result<Flow> {
    // Resolved once, shared below by both the existing `last_formula_edit`/
    // `recoverable_buffer` invalidation and the undo-push decision after
    // `dispatch_inner` runs, rather than each recomputing it separately.
    let (resolved_cmd, cmd_args_trimmed): (Option<&'static str>, &str) =
        match line.strip_prefix(':') {
            Some(rest) => {
                let mut parts = rest.splitn(2, char::is_whitespace);
                let first = parts.next().unwrap_or("");
                (
                    resolve_command(first).ok(),
                    parts.next().unwrap_or("").trim(),
                )
            }
            None => (None, ""),
        };

    // `:formula cancel`, and the recovery it reads (`last_formula_edit`/
    // `recoverable_buffer` — see `session.rs`), only ever refer to the one
    // formula edit that's still the most recent thing that happened.
    // Everything else dispatched in between invalidates both here, once,
    // ahead of every other branch below — including a bare proof-rule
    // line (the fall-through case at the very bottom of `dispatch_inner`,
    // outside the `:`-command branch entirely, which a narrower check
    // scoped to that branch would miss) — so neither can ever silently
    // let unrelated work get discarded later, or let an old, already
    // -abandoned tail reappear. `:formula` itself (mode entry or
    // `cancel`) is excluded, so it can both read a snapshot (`cancel`)
    // and set a fresh one (an edit) without immediately clearing what
    // it's about to use.
    if resolved_cmd != Some("formula")
        && let Some(session) = session.as_mut()
    {
        session.last_formula_edit = None;
        session.recoverable_buffer = None;
    }

    // Captured before `dispatch_inner` runs, so a genuine change can be
    // detected afterward via `Session::generation` — the only signal
    // needed, since neither `Formula` nor `VarNameManager` implements
    // `PartialEq`. `None` if no session is loaded yet (nothing to undo to
    // in that case regardless of what `:load` does).
    let before = session.as_ref().map(|s| (s.generation, s.snapshot()));

    let result = dispatch_inner(session, line, out);

    if let Some(session) = session.as_mut() {
        // `:undo` must never push (no accidental redo-via-repeated-undo
        // loop). `:load`/`:instance` replace the whole `Session` object
        // (`*session = Some(new_session)` — see `load.rs`/`instance.rs`),
        // so the new session already has its own empty `undo_stack`;
        // pushing the *old* session's snapshot onto it would try to
        // restore a proof buffer from one formula onto a different one.
        // `:reset` mutates in place instead of replacing, so it needs an
        // explicit clear rather than relying on that. `:formula cancel`
        // is itself a corrective action, not a new one worth remembering
        // — same reasoning as excluding `:undo`.
        let no_push = matches!(resolved_cmd, Some("undo") | Some("load") | Some("instance"))
            || (resolved_cmd == Some("formula") && cmd_args_trimmed == "cancel");
        if resolved_cmd == Some("reset") {
            session.undo_stack.clear();
        } else if !no_push
            && let Some((gen_before, snap_before)) = before
            && session.generation != gen_before
        {
            session.push_undo(snap_before);
        }
    }

    result
}

/// The actual command logic `dispatch` wraps — a `:`-command or a bare
/// proof rule against the session, emitting all user-visible text through
/// `out`. Split out from `dispatch` purely so the undo-stack bookkeeping
/// there can wrap a single call to this, rather than threading through
/// every early return below.
fn dispatch_inner(
    session: &mut Option<Session>,
    line: &str,
    out: &mut dyn Output,
) -> anyhow::Result<Flow> {
    if let Some(rest) = line.strip_prefix(':') {
        let mut parts = rest.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        let cmd_args = parts.next().unwrap_or("").trim();

        let cmd = match resolve_command(cmd) {
            Ok(name) => name,
            Err(message) => {
                outln!(out, "{message}");
                return Ok(Flow::Continue);
            }
        };

        if cmd == "quit" {
            return Ok(Flow::Quit);
        }
        if cmd == "help" {
            help::run(cmd_args, out);
            return Ok(Flow::Continue);
        }
        if cmd == "theme" {
            theme::run(out);
            return Ok(Flow::Continue);
        }
        if cmd == "load" {
            load::run(session, cmd_args, out);
            return Ok(Flow::Continue);
        }
        if cmd == "instance" {
            instance::run(session, cmd_args, out)?;
            return Ok(Flow::Continue);
        }
        if cmd == "edit" {
            let Some(session) = session.as_mut() else {
                outln!(out, "No formula loaded — use :load <formula.opb> first.");
                return Ok(Flow::Continue);
            };
            return Ok(match edit::start(session, cmd_args, out) {
                Some(state) => Flow::StartEdit(state),
                None => Flow::Continue,
            });
        }
        if cmd == "deassert" {
            let Some(session) = session.as_mut() else {
                outln!(out, "No formula loaded — use :load <formula.opb> first.");
                return Ok(Flow::Continue);
            };
            return Ok(match edit::start_deassert(session, cmd_args, out) {
                Some(state) => Flow::StartEdit(state),
                None => Flow::Continue,
            });
        }
        if cmd == "insert" {
            let Some(session) = session.as_mut() else {
                outln!(out, "No formula loaded — use :load <formula.opb> first.");
                return Ok(Flow::Continue);
            };
            return Ok(match edit::start_insert(session, cmd_args, out) {
                Some(state) => Flow::StartEdit(state),
                None => Flow::Continue,
            });
        }
        if cmd == "formula" {
            let Some(session) = session.as_mut() else {
                outln!(out, "No formula loaded — use :load <formula.opb> first.");
                return Ok(Flow::Continue);
            };
            if cmd_args.trim() == "cancel" {
                formula::cancel(session, out)?;
                return Ok(Flow::Continue);
            }
            return Ok(match formula::start(session, cmd_args, out) {
                Some(state) => Flow::StartFormulaEdit(state),
                None => Flow::Continue,
            });
        }
        if cmd == "debug" {
            let Some(session) = session.as_ref() else {
                outln!(out, "No formula loaded — use :load <formula.opb> first.");
                return Ok(Flow::Continue);
            };
            return Ok(match debug::start(session, out) {
                Some(state) => Flow::StartDebug(state),
                None => Flow::Continue,
            });
        }
        let Some(session) = session.as_mut() else {
            outln!(out, "No formula loaded — use :load <formula.opb> first.");
            return Ok(Flow::Continue);
        };
        match cmd {
            "show" => show::run(session, cmd_args, out),
            "explain" => explain::run(session, cmd_args, out),
            "why" => why::run(session, cmd_args, out),
            "check" => check::run(session, cmd_args, out)?,
            "objective" => objective::run(session, out),
            "list" => list::run(session, out),
            "undo" => undo::run(session, cmd_args, out),
            "delete" => edit::start_delete(session, cmd_args, out)?,
            "source" => source::run(session, cmd_args, out)?,
            "verify" => verify::run(session, out)?,
            "reset" => reset::run(session, out),
            "save" => save::run(session, cmd_args, out)?,
            // Every registry entry must have an arm above; reaching this
            // means the registry and dispatch have drifted apart.
            _ => outln!(out, "BUG: ':{cmd}' is registered but not wired up."),
        }
        return Ok(Flow::Continue);
    }

    let Some(session) = session.as_mut() else {
        outln!(out, "No formula loaded — use :load <formula.opb> first.");
        return Ok(Flow::Continue);
    };
    match session.append_line(line)? {
        AppendOutcome::Verified { captured } => output::text(out, &captured),
        AppendOutcome::Rejected { captured, error } => {
            output::text(out, &captured);
            output::error(out, &error);
            outln!(out, "(line rejected, state unchanged)");
        }
        AppendOutcome::Deferred => {
            let pending = session.buffer.len() - session.checked_len;
            outln!(
                out,
                "(added to the buffer, unchecked — {pending} line(s) now pending; :verify \
                 when ready)"
            );
        }
    }
    Ok(Flow::Continue)
}
