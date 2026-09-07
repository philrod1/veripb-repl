//! VeriPB REPL entry point: parse the arguments and hand off to a
//! frontend — the crossterm multi-panel TUI by default (`tui/`), or the
//! plain line-based loop (`plain.rs`) with `--plain` or whenever stdin or
//! stdout isn't a real terminal, so piping and scripting keep working.
//!
//! The core mechanic lives in `session.rs`: the proof buffer is a live,
//! only-partly-checked list of lines (see that module's own docs for the
//! full model), and checking one more of them means re-running the whole
//! checked prefix plus the candidate through a fresh checker, accepted
//! iff the parser's error lands exactly at the end (meaning "checked
//! fine, just wants more input") rather than partway through. The
//! individual `:`-commands built on top of it live in `commands/`, all
//! emitting through the `Output` sink in `output.rs` so frontends decide
//! where text lands.

mod commands;
mod output;
mod plain;
mod session;
mod tui;

use std::{env, io::IsTerminal};

use anyhow::bail;

fn main() -> anyhow::Result<()> {
    let mut plain = false;
    let mut formula_path: Option<String> = None;
    for arg in env::args().skip(1) {
        if arg == "--plain" {
            plain = true;
        } else if arg.starts_with('-') {
            bail!("unknown option {arg}; usage: veripb-repl [--plain] [<formula.opb>]");
        } else if formula_path.is_none() {
            formula_path = Some(arg);
        } else {
            bail!("unexpected extra argument {arg}; usage: veripb-repl [--plain] [<formula.opb>]");
        }
    }

    if plain || !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        plain::run(formula_path.as_deref())
    } else {
        tui::run(formula_path.as_deref())
    }
}
