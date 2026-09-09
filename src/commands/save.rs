//! `:save <file.pbp> [<conclusion>]` — write the session out as a `.pbp`
//! file: the synthesized preamble, **the entire buffer, checked or not**,
//! and a closing `output NONE; conclusion <conclusion>; end pseudo-Boolean
//! proof;` sequence. Always writes, regardless of check status — unlike a
//! completed proof's own closing sequence, the buffer is meant to be
//! freely saved and reloaded mid-work, not just once it's fully verified.
//! An unverified conclusion, an unchecked tail, or a known-bad line are
//! all reported as warnings rather than blocking the write.

use std::fs;

use crate::commands::check::auto_detect_candidates;
use crate::commands::expand_tilde;
use crate::output::{self, Output, outln};
use crate::session::Session;

pub fn run(session: &Session, args: &str, out: &mut dyn Output) -> anyhow::Result<()> {
    let args = args.trim();
    let (path, conclusion_arg) = match args.split_once(char::is_whitespace) {
        Some((path, rest)) => (path, rest.trim().trim_end_matches(';').trim()),
        None => (args, ""),
    };
    if path.is_empty() {
        outln!(out, "Error: usage :save <file.pbp> [<conclusion>]");
        return Ok(());
    }
    let path = &expand_tilde(path);

    let pending = session.buffer.len() - session.checked_len;
    if pending > 0 {
        outln!(
            out,
            "Warning: {pending} line(s) in the buffer haven't been checked yet — saving as-is."
        );
    }
    if let Some(reason) = &session.known_bad {
        outln!(
            out,
            "Warning: line {} does not verify: {reason}",
            session.display_line(session.checked_len)
        );
    }

    let conclusion = if !conclusion_arg.is_empty() {
        let (captured, result) = session.dry_run_conclusion(conclusion_arg)?;
        // On success this is the checker's own "s VERIFIED ..." line.
        output::text(out, &captured);
        if let Err(err) = result {
            output::error(out, &err);
            outln!(
                out,
                "Warning: conclusion `{conclusion_arg}` does not verify — saving anyway."
            );
        }
        conclusion_arg.to_string()
    } else {
        let mut detected = None;
        for candidate in auto_detect_candidates(session) {
            let (captured, result) = session.dry_run_conclusion(&candidate)?;
            if result.is_ok() {
                output::text(out, &captured);
                detected = Some(candidate);
                break;
            }
        }
        match detected {
            Some(conclusion) => conclusion,
            None => {
                outln!(
                    out,
                    "No conclusion auto-detected; saving with `conclusion NONE;` (a real \
                     no-op — see `conclusion NONE` in the guide for why that isn't a verified \
                     result). Pass an explicit conclusion to :save to claim one."
                );
                "NONE".to_string()
            }
        }
    };

    let text = session.listing(&conclusion);
    match fs::write(path, text) {
        Ok(()) => outln!(
            out,
            "Saved {} line(s) to {path} (conclusion {conclusion}).",
            session.buffer.len()
        ),
        Err(err) => outln!(out, "Error: failed to write {path}: {err}"),
    }
    Ok(())
}
