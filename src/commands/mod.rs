//! REPL `:`-commands (one module each; `:quit` is inline) and [`dispatch`],
//! the shared entry point both frontends feed submitted lines through.
//! Commands must emit only through the `Output` sink they are given, never
//! print directly.

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
pub mod preserved;
pub mod reset;
pub mod save;
pub mod show;
pub mod source;
pub mod theme;
pub mod undo;
pub mod verify;

use crate::output::{self, Output, outln};
use crate::session::{AppendOutcome, Session};

/// What the frontend's loop should do after a dispatched line.
pub enum Flow {
    Continue,
    Quit,
    /// `:edit`/`:deassert`/`:insert` started: route lines through
    /// `edit::handle` until it returns `EditFlow::Ended`.
    StartEdit(edit::EditState),
    /// `:formula` mode started: route lines through `formula::handle` until it
    /// returns `FormulaEditFlow::Ended`. The TUI intercepts `:formula` before
    /// `dispatch`, so only the plain frontend sees this.
    StartFormulaEdit(formula::FormulaEditState),
    /// `:debug` started: route lines through `debug::handle` until it returns
    /// `DebugFlow::Ended`. Both frontends receive this.
    StartDebug(debug::DebugState),
}

/// Expands a leading `~` or `~/` to `$HOME`. Other paths (including `~user`)
/// are returned unchanged, as is everything if `HOME` is unset.
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

/// Loads the startup formula, if given, and announces the result. A load
/// failure is returned as an error (unlike `:load`, which only reports it).
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

/// Resolves a possibly-abbreviated command name against `help::COMMANDS`: an
/// exact name wins, otherwise a unique prefix match. Errors on empty, unknown
/// or ambiguous input (listing candidates). Unregistered commands cannot be
/// dispatched.
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

/// Runs one non-empty input line (a `:`-command or a proof rule) against the
/// session, writing all output to `out`. Wraps `dispatch_inner` with undo
/// and `:formula cancel` bookkeeping.
pub fn dispatch(
    session: &mut Option<Session>,
    line: &str,
    out: &mut dyn Output,
) -> anyhow::Result<Flow> {
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

    // `:formula cancel` may only undo the most recent action. Any line other
    // than `:formula` itself (including a bare proof rule) invalidates its
    // recovery state.
    if resolved_cmd != Some("formula")
        && let Some(session) = session.as_mut()
    {
        session.last_formula_edit = None;
        session.recoverable_buffer = None;
    }

    // A change is detected by `Session::generation` differing afterwards.
    let before = session.as_ref().map(|s| (s.generation, s.snapshot()));

    let result = dispatch_inner(session, line, out);

    if let Some(session) = session.as_mut() {
        // Never push for `:undo` or `:formula cancel` (corrective actions),
        // or for `:load`/`:instance` (they replace the session, so the old
        // snapshot belongs to a different formula). `:reset` clears the stack.
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

/// The command logic [`dispatch`] wraps, kept separate so its early returns
/// don't bypass the undo bookkeeping.
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
            "check" => check::run(session, cmd_args, out)?,
            "objective" => objective::run(session, out),
            "preserved" => preserved::run(session, out),
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
