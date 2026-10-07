//! `:source <file.pbp>` — replace the current proof with one loaded from a
//! file: the buffer is cleared first (formula kept, same as `:reset`),
//! then the file's derivation lines land in it **unchecked** — nothing is
//! verified on load. `:verify` checks them, on request, same as any other
//! unchecked buffer content (see `session.rs`'s module docs). This is
//! deliberate, not a missing feature: checking a whole file's worth of
//! lines the instant it loads is exactly what made a single typo deep in
//! a long file expensive to recover from before — the fix used to be
//! "stop, report how far it got, and require a fresh `:source` to try
//! again"; now the file's already-loaded content simply sits there,
//! editable in place, until `:verify` says otherwise.
//!
//! A complete `.pbp` file's *framing* is handled specially rather than fed
//! through: the session synthesizes its own preamble and closing sequence
//! on demand (`:check`, `:save`), so applying the file's copies would
//! duplicate the preamble and — worse — close the session for good,
//! leaving `:check` and further rules unusable. Instead the version header
//! and a matching `f N;` are skipped, and the closing
//! `output`/`conclusion`/`end` lines are lifted out and simply dropped —
//! checking a conclusion only means anything once the derivation leading
//! to it has actually verified, which `:source` alone no longer attempts.

use std::fs;

use crate::commands::expand_tilde;
use crate::output::{Output, outln};
use crate::session::Session;

/// Parse an `f N ;` formula-count-check rule, returning `N`. Anything
/// else — including other rules starting with the letter f — is `None`.
fn parse_f_check(line: &str) -> Option<usize> {
    let rest = line.strip_prefix('f')?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    rest.trim().trim_end_matches(';').trim().parse().ok()
}

/// Split a `.pbp` file's contents into its derivation lines and its
/// closing conclusion (the latter parsed but, for now, unused — see the
/// module docs on why a loaded file's conclusion isn't auto-checked).
/// `session` supplies the formula length the file's own `f N;` line is
/// compared against (see the `past_preamble` handling below); this never
/// mutates it. `out` is only for the one advisory message a non-`NONE`
/// `output` line prints — everything else here is silent.
pub(crate) fn parse_derivation<'a>(
    contents: &'a str,
    session: &Session,
    out: &mut dyn Output,
) -> (Vec<(usize, &'a str)>, Option<String>) {
    let mut past_preamble = false;
    // The conclusion named by the file's closing sequence, if any, held
    // back for a non-destructive check after the derivation is in.
    let mut pending_conclusion: Option<String> = None;
    // (0-based source-file line index, trimmed text) for everything that
    // isn't framing — what actually needs applying.
    let mut derivation: Vec<(usize, &str)> = Vec::new();

    for (i, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if !past_preamble {
            // The synthesized preamble already supplies both of these two
            // lines, so a complete file's own copies would be duplicates —
            // the version header an outright rejected one. An `f N;` whose
            // count *doesn't* match the loaded formula is deliberately fed
            // through instead of skipped: it means the file was written
            // for a different formula, and the checker's own error says so
            // far better than silently misapplying the proof would.
            if line.starts_with("pseudo-Boolean proof version") {
                continue;
            }
            past_preamble = true;
            if parse_f_check(line) == Some(session.formula.len()) {
                continue;
            }
        }

        // The closing sequence: never applied (see module docs). `output`
        // and `conclusion` are only valid as the tail in v3 syntax, so a
        // prefix match here can't shadow a real derivation rule.
        if let Some(rest) = line.strip_prefix("output")
            && rest.starts_with(|c: char| c.is_whitespace() || c == ';')
        {
            let what = rest.trim().trim_end_matches(';').trim();
            if what != "NONE" {
                outln!(out, "(skipping `{line}` — only `output NONE` is supported)");
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("conclusion")
            && rest.starts_with(char::is_whitespace)
        {
            let conclusion = rest.trim().trim_end_matches(';').trim();
            pending_conclusion = (!conclusion.is_empty()).then(|| conclusion.to_string());
            continue;
        }
        if line.starts_with("end pseudo-Boolean proof") {
            continue;
        }

        derivation.push((i, line));
    }

    (derivation, pending_conclusion)
}

pub fn run(session: &mut Session, args: &str, out: &mut dyn Output) -> anyhow::Result<()> {
    let path = args.trim();
    if path.is_empty() {
        outln!(out, "Error: usage :source <file.pbp>");
        return Ok(());
    }
    let path = &expand_tilde(path);

    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) => {
            outln!(out, "Error: failed to read {path}: {err}");
            return Ok(());
        }
    };

    let (derivation, _pending_conclusion) = parse_derivation(&contents, session, out);

    // Replace, not append — matches `:load`'s "this is now the X" model
    // for the formula, applied to the proof instead. Only announced when
    // there was actually something to clear, so the common case (sourcing
    // into a still-empty buffer) stays quiet.
    let existing = session.buffer.len();
    if existing > 0 {
        if let Err(err) = session.reset() {
            outln!(out, "Error: {err:#}");
            return Ok(());
        }
        outln!(out, "Cleared {existing} existing proof line(s).");
    }

    session.buffer = derivation
        .into_iter()
        .map(|(_, text)| text.to_string())
        .collect();
    // Marks the change for `dispatch`'s undo push (and the database cache)
    // even when the buffer was already empty and `reset` didn't run.
    session.generation += 1;
    outln!(
        out,
        "Loaded {} line(s) from {path} into the buffer — nothing checked yet. :verify when \
         ready.",
        session.buffer.len()
    );
    Ok(())
}
