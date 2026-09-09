//! `:verify` — check the buffer's unchecked tail, one line at a time,
//! committing each as it passes and stopping at the first rejection.
//! Nothing is ever discarded: a rejected line stays exactly where it is,
//! along with everything still unchecked after it — [`Session::drive_forward`]
//! does all the real work here; this is just its dispatch-facing wrapper.

use crate::output::{self, Output, outln};
use crate::session::Session;

pub fn run(session: &mut Session, out: &mut dyn Output) -> anyhow::Result<()> {
    let pending = session.buffer.len() - session.checked_len;
    if pending == 0 {
        outln!(out, "Nothing unchecked — the whole buffer is already verified.");
        return Ok(());
    }

    let (captured, rejection) = session.drive_forward()?;
    output::text(out, &captured);

    match rejection {
        None => outln!(out, "Verified {pending} line(s)."),
        Some(err) => {
            output::error(out, &err);
            let display_line = session.display_line(session.checked_len);
            let remaining = session.buffer.len() - session.checked_len - 1;
            outln!(
                out,
                "(stopped at line {display_line}; {remaining} line(s) after it remain \
                 unchecked. Retype it (:edit {display_line}), then :verify again.)"
            );
        }
    }
    Ok(())
}
