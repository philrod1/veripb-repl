//! `:theme [<name>]` — switch the TUI's color palette. Purely a TUI
//! concern: the TUI intercepts it ahead of `commands::dispatch` entirely
//! (see `tui::App::start_theme`), where it actually switches
//! `theme::ThemeName` and takes effect on the next redraw. This module is
//! only ever reached from the plain frontend, then — which has no color
//! at all — so it just says so rather than doing (or pretending to do)
//! anything.

use crate::output::{Output, outln};

pub fn run(out: &mut dyn Output) {
    outln!(
        out,
        "Theming only applies to the TUI frontend — this is --plain, which has no color at all."
    );
}
