//! `:instance <file.opb>` / `:instance <file.pbp>` / `:instance <stem>` —
//! load a formula and its matching proof together in one step. Name either
//! half of the pair and the other is found by swapping the extension; name
//! neither (a bare stem, no `.opb`/`.pbp` on the end) and both are found by
//! appending it — same stem either way, `.opb` <-> `.pbp`, the convention
//! `tests/instances/` already follows throughout this repo, and most of
//! the time how a formula and its proof actually sit on disk. Both files
//! are confirmed to exist up front, before anything is touched, rather
//! than letting a typo leave the formula loaded with no proof to follow
//! it. Wraps `:load` and `:source` as-is rather than reimplementing
//! either: a bad formula leaves the session untouched exactly like a bad
//! `:load` does, and a bad or partial proof is reported exactly the way
//! `:source` already reports one — this only saves the second command,
//! not its behavior.

use std::path::Path;

use crate::commands::{expand_tilde, source};
use crate::output::{Output, outln};
use crate::session::Session;

pub fn run(session: &mut Option<Session>, args: &str, out: &mut dyn Output) -> anyhow::Result<()> {
    let path = args.trim();
    if path.is_empty() {
        outln!(out, "Error: usage :instance <file.opb|file.pbp|stem>");
        return Ok(());
    }
    let path = expand_tilde(path);
    let (opb_path, pbp_path) = sibling_paths(&path);

    match (
        Path::new(&opb_path).is_file(),
        Path::new(&pbp_path).is_file(),
    ) {
        (true, true) => {}
        (false, true) => {
            outln!(
                out,
                "Error: no {opb_path} found to pair with {pbp_path} — nothing loaded."
            );
            return Ok(());
        }
        (true, false) => {
            outln!(
                out,
                "Error: no {pbp_path} found to pair with {opb_path} — :load it alone if \
                 that's all you meant."
            );
            return Ok(());
        }
        (false, false) => {
            outln!(
                out,
                "Error: neither {opb_path} nor {pbp_path} exists — nothing loaded."
            );
            return Ok(());
        }
    }

    match Session::load(&opb_path) {
        Ok(new_session) => {
            outln!(
                out,
                "Loaded {} constraints from {opb_path}",
                new_session.formula.len()
            );
            *session = Some(new_session);
        }
        Err(err) => {
            outln!(out, "Error: {err:#}");
            return Ok(());
        }
    }

    // Just set above, and nothing between here and there can unload it.
    let session = session
        .as_mut()
        .expect("instance load just set the session");
    source::run(session, &pbp_path, out)
}

/// The `.opb`/`.pbp` pair implied by `path`: whichever extension it has
/// (case-insensitively), the other with the same stem; with neither —
/// including any other extension, e.g. a compressed formula's — `path`
/// itself is treated as the stem and both suffixes are appended fresh.
/// Never rejects a shape outright; a nonsensical result just won't exist,
/// and `run` checks that before touching anything.
fn sibling_paths(path: &str) -> (String, String) {
    let p = Path::new(path);
    match p.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("opb") => (
            path.to_string(),
            p.with_extension("pbp").to_string_lossy().into_owned(),
        ),
        Some(ext) if ext.eq_ignore_ascii_case("pbp") => (
            p.with_extension("opb").to_string_lossy().into_owned(),
            path.to_string(),
        ),
        _ => (format!("{path}.opb"), format!("{path}.pbp")),
    }
}
