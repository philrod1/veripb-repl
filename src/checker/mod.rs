//! The only module in this crate that knows a `veripb` binary exists.
//! Everything else — `session`, `commands::*`, `tui::*` — talks to a
//! formula/proof exclusively through what this module exposes.
//!
//! - [`types`] — plain result/data types ([`CheckOutcome`], [`RupHint`])
//!   that cross this boundary; no VeriPB library type anywhere.
//! - [`invoke`] — the actual subprocess plumbing: temp files, spawning
//!   `veripb`, capturing stdout/stderr/exit status, completely
//!   unparsed.
//! - `parse` (not yet written) — scraping `invoke`'s raw output back
//!   into [`types`]'s plain shapes (a rejected line's number and
//!   message, the last `rup` line out of a scratch `--elaborate` file,
//!   ...). This module's own public API (`check`/`explain_line`/
//!   `why_rup`/...) — composing `invoke` + `parse` into what the rest of
//!   the crate actually calls — waits until `parse` exists to take
//!   final shape.

pub mod invoke;
pub mod parse;
pub mod types;

pub use types::{CheckOutcome, RupHint};
