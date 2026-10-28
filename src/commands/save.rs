//! `:save <file.pbp> [<conclusion>]` — write the session out as a `.pbp`
//! file: the synthesized preamble, **the entire buffer, checked or not**,
//! and a closing `output NONE; conclusion <conclusion>; end pseudo-Boolean
//! proof;` sequence. Always writes, regardless of check status — unlike a
//! completed proof's own closing sequence, the buffer is meant to be
//! freely saved and reloaded mid-work, not just once it's fully verified.
//! An unverified conclusion, an unchecked tail, or a known-bad line are
//! all reported as warnings rather than blocking the write.

use std::fs;

use crate::commands::check::detect;
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
        match (detect(session, out)?, &session.intended_conclusion) {
            (Some((conclusion, captured)), _) => {
                output::text(out, &captured);
                conclusion
            }
            (None, Some(intended)) => {
                outln!(
                    out,
                    "Warning: no conclusion verifies; saving with the intended conclusion \
                     `{intended}` anyway."
                );
                intended.clone()
            }
            (None, None) => {
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

    let written = fs::File::create(path).and_then(|file| {
        let mut writer = std::io::BufWriter::new(file);
        session.write_listing(&conclusion, &mut writer)?;
        std::io::Write::flush(&mut writer)
    });
    match written {
        Ok(()) => outln!(
            out,
            "Saved {} line(s) to {path} (conclusion {conclusion}).",
            session.buffer.len()
        ),
        Err(err) => outln!(out, "Error: failed to write {path}: {err}"),
    }
    Ok(())
}
