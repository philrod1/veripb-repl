//! Where user-visible text goes. Commands never print directly — everything
//! they emit flows through the [`Output`] trait so each frontend can route
//! it: the plain CLI prints lines to stdout immediately, and the crossterm
//! TUI (to come) will append them to its scrollback pane instead.

/// `outln!(out, "...")` is `println!` for an [`Output`] sink — same
/// formatting syntax, one line per call, so converting a `println!` call
/// site is mechanical.
macro_rules! outln {
    ($out:expr) => {
        $out.line("")
    };
    ($out:expr, $($arg:tt)*) => {
        $out.line(&format!($($arg)*))
    };
}
pub(crate) use outln;

pub trait Output {
    /// Emit one line of user-visible text (without a trailing newline —
    /// the sink supplies its own line separation).
    fn line(&mut self, text: &str);
}

/// Plain-CLI sink: each line goes straight to stdout.
pub struct Stdout;

impl Output for Stdout {
    fn line(&mut self, text: &str) {
        println!("{text}");
    }
}

/// Emit a block of already-formatted, possibly multi-line text — e.g. the
/// checker output captured during a replay — one line at a time. Empty
/// text emits nothing at all (not even a blank line).
pub fn text(out: &mut dyn Output, text: &str) {
    for line in text.lines() {
        out.line(line);
    }
}

/// Prints an error message (e.g. a `CheckOutcome::Rejected`'s `message`,
/// or an invocation failure's text).
///
/// Routed through [`text`] rather than a single `outln!` call: the
/// message can be multiple lines, and the TUI's scrollback writes each
/// line at an absolute cursor position, so an embedded `\n` mid-write
/// would corrupt every row after it.
pub fn error(out: &mut dyn Output, message: &str) {
    text(out, &format!("Error: {message}"));
}
