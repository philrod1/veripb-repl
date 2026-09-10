//! VeriPB REPL: an interactive REPL/TUI for exploring and building VeriPB
//! proofs — talks to a `veripb` binary on `$PATH` rather than linking any
//! VeriPB library crate directly (see `checker/`).  `src/main.rs` is a
//! thin binary wrapper over this library.
//! The core mechanic lives in `session`: the proof buffer is a live,
//! only-partly-checked list of lines (see that module's own docs for the
//! full model), and checking one more of them means re-running the whole
//! checked prefix plus the candidate through a fresh checker, accepted
//! iff the parser's error lands exactly at the end.

pub mod checker;
pub mod commands;
pub mod output;
pub mod plain;
pub mod session;
pub mod tui;
pub mod varnames;
