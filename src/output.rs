//! Where user-visible text goes. Commands never print directly — everything
//! they emit flows through the [`Output`] trait so each frontend can route
//! it: the plain CLI prints lines to stdout immediately, and the crossterm
//! TUI (to come) will append them to its scrollback pane instead.

use std::error::Error as StdError;

use veripb_checker::error::ForwardsCheckerError;

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

/// `ForwardsCheckerError::Parse`'s `Display` is just a generic wrapper
/// ("Syntax error while parsing proof file!") — the actual message, line,
/// column, and code snippet live on the wrapped `ParserError`, reachable
/// only via `.source()`. Walk the whole chain so nothing useful is hidden.
///
/// `ForwardsCheckerError::Checking`'s own `Display` is the opposite
/// problem: a pure passthrough (`"{0}"`) to its wrapped `CheckingLineError`,
/// which already bakes its own "Checking error at F:L / Caused by: ..."
/// text into *its* `Display` — and thiserror's `#[from]` makes `.source()`
/// return that identical value right back. Walking `.source()`
/// unconditionally would print that same fully-formatted block twice in a
/// row (once as the top-level message, once again as its own "Caused
/// by:"), roughly doubling an already-multi-line message for no new
/// information. Comparing each level's formatted text against the last
/// catches this generically, without needing to know which error types
/// happen to be pure passthroughs — a genuinely new level always formats
/// differently.
///
/// `CheckingLineError`'s baked-in message is itself multiple lines —
/// literal `\n`/`\t` embedded in one `Display` string, not separate
/// `Output::line` calls. Routing it through `outln!` (a single `line`
/// call) rather than [`text`] worked by accident in the plain frontend —
/// `println!` just prints embedded newlines as newlines — but broke the
/// TUI outright: its scrollback writes each line at an absolute cursor
/// position, and an embedded `\n` mid-write makes the real terminal jump
/// lines on its own, corrupting every row after it, silently swallowing
/// the actual explanation (see `Session`/`commands` — no code path here
/// was dropping it; the bytes just landed somewhere the renderer never
/// looked). Splitting through `text` — exactly like the checker's own
/// captured trace output already does — fixes both frontends at once.
pub fn error_chain(out: &mut dyn Output, err: &ForwardsCheckerError) {
    let mut last = err.to_string();
    text(out, &format!("Error: {last}"));
    let mut source = StdError::source(err);
    while let Some(err) = source {
        let formatted = err.to_string();
        if formatted != last {
            text(out, &format!("  Caused by: {formatted}"));
            last = formatted;
        }
        source = err.source();
    }
}
