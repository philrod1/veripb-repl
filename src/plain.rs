//! The plain line-based frontend: a bare `stdin` read loop with no raw-mode
//! terminal handling, no history, no completion. Kept alongside the
//! crossterm TUI — it's the right frontend for piping, scripted tests, and
//! dumb terminals (and is selected automatically for them), and it's also
//! the reference for what every command's output should look like, since
//! commands emit the same lines through the same `Output` sink in both
//! frontends.

use std::io::{BufRead, Write, stdin, stdout};

use crate::commands::{self, Flow, debug, edit, formula};
use crate::output::Stdout;

/// Whether `line` resolves (through the same abbreviation logic
/// `dispatch` uses) to `cmd` — used here just for `:source`'s "working"
/// acknowledgment, ahead of the blocking call that follows.
fn resolves_to(line: &str, cmd: &str) -> bool {
    line.strip_prefix(':')
        .and_then(|rest| rest.split_whitespace().next())
        .is_some_and(|first| commands::resolve_command(first) == Ok(cmd))
}

pub fn run(formula_path: Option<&str>) -> anyhow::Result<()> {
    let mut out = Stdout;
    let mut session = commands::startup(formula_path, &mut out)?;
    let mut edit_state: Option<edit::EditState> = None;
    let mut formula_state: Option<formula::FormulaEditState> = None;
    let mut debug_state: Option<debug::DebugState> = None;

    let stdin = stdin();

    loop {
        // While `:deassert` is active, the assertion it's replacing is
        // folded into the prompt label itself — the only thing printed
        // on every single line, so it's the one place that stays visible
        // for the whole edit, not just the intro message printed once at
        // the start (which has long since scrolled past by the time
        // you're a few derivation steps in).
        let prompt = if debug_state.is_some() {
            "debug> ".to_string()
        } else if formula_state.is_some() {
            "opb> ".to_string()
        } else if let Some(state) = edit_state.as_ref() {
            match state.deassert_source() {
                Some(source) => format!("edit (deasserting {source})> "),
                None => "edit> ".to_string(),
            }
        } else {
            "pbp> ".to_string()
        };
        print!("{prompt}");
        stdout().flush()?;

        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            println!();
            break;
        }
        let line = line.trim_end();

        if let Some(state) = edit_state.as_mut() {
            // `:edit` only ever starts with a session loaded (see
            // dispatch), and nothing during an edit can unload it. Unlike
            // the normal loop below, a blank line is deliberately let
            // through here — `edit::handle` gives it a meaning ("keep
            // the shown line unchanged") instead of the usual no-op.
            let session = session
                .as_mut()
                .expect("edit mode implies a loaded session");
            match edit::handle(session, state, line, &mut out)? {
                edit::EditFlow::Continue => {}
                edit::EditFlow::Ended => edit_state = None,
            }
            continue;
        }

        if let Some(state) = formula_state.as_mut() {
            // Same reasoning as the `:edit` block above — a blank line
            // means "keep this constraint unchanged" to `formula::handle`,
            // not "do nothing".
            let session = session
                .as_mut()
                .expect("formula mode implies a loaded session");
            match formula::handle(session, state, line, &mut out)? {
                formula::FormulaEditFlow::Continue => {}
                formula::FormulaEditFlow::Ended => formula_state = None,
            }
            continue;
        }

        if let Some(state) = debug_state.as_mut() {
            // Unlike the `:edit`/`:formula` blocks above, a blank line
            // isn't "keep this unchanged" here — `debug::handle` treats
            // it as a plain no-op, the same meaning it has everywhere
            // else, so there's nothing special to intercept; letting it
            // through is just simpler than duplicating that check here.
            let session = session
                .as_mut()
                .expect("debug mode implies a loaded session");
            match debug::handle(session, state, line, &mut out)? {
                debug::DebugFlow::Continue => {}
                debug::DebugFlow::Ended => debug_state = None,
            }
            continue;
        }

        if line.is_empty() {
            continue;
        }

        // `:source` (and `:instance`, which calls straight into it after
        // loading the formula) can process hundreds of lines in one
        // blocking call; print an immediate acknowledgment (flushed,
        // since the terminal otherwise wouldn't see it until the whole
        // call returns) rather than leaving the prompt looking
        // unresponsive for however long it takes. Not an animated
        // spinner — there's nowhere to interleave one in a
        // single-threaded, synchronous read loop.
        if resolves_to(line, "source") || resolves_to(line, "instance") {
            println!("Working…");
            stdout().flush()?;
        }

        match commands::dispatch(&mut session, line, &mut out)? {
            Flow::Quit => break,
            Flow::Continue => {}
            // `edit::start` already announced everything worth saying
            // (which line(s), what to type, how to cancel) through `out`.
            Flow::StartEdit(state) => edit_state = Some(state),
            // `formula::start` already announced everything worth saying,
            // same as above.
            Flow::StartFormulaEdit(state) => formula_state = Some(state),
            // `debug::start` already announced everything worth saying,
            // same as above.
            Flow::StartDebug(state) => debug_state = Some(state),
        }
    }

    Ok(())
}
