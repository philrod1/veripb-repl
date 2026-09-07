//! Entry point: parse the arguments and hand off to a frontend — the
//! crossterm multi-panel TUI by default (`veripb_repl::tui`), or the
//! plain line-based loop (`veripb_repl::plain`) with `--plain` or
//! whenever stdin or stdout isn't a real terminal, so piping and
//! scripting keep working. See `lib.rs` for the crate's own docs — this
//! binary is a thin wrapper over it, split out so `tests/*.rs` has a
//! library to link against.

use std::{env, io::IsTerminal};

use anyhow::bail;
use veripb_repl::{plain, tui};

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
