//! Progress of `veripb` runs, for a frontend to show while one is under way.
//!
//! A frontend installs a reporter with [`set_reporter`]. While one is
//! installed (on Unix), each run hands the proof to `veripb` through a named
//! pipe instead of a file, counting the lines written, and calls the
//! reporter every [`INTERVAL`] from another thread until `veripb` exits.
//! `veripb` checks each line as it reads it, and the pipe holds little, so
//! the count follows the line it has reached.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

/// How far one `veripb` run has got.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    /// This is the run's first report.
    pub started: bool,
    /// Proof lines handed to `veripb` so far, preamble included.
    pub line: usize,
    /// Proof lines in all, preamble included.
    pub total: usize,
}

/// Called with each report, from a thread of its own.
pub type Reporter = Arc<dyn Fn(Progress) + Send + Sync>;

/// How often a run reports.
pub const INTERVAL: Duration = Duration::from_millis(100);

thread_local! {
    static REPORTER: RefCell<Option<Reporter>> = const { RefCell::new(None) };
}

/// Installs `reporter` for the `veripb` runs this thread makes (`None`
/// removes it).
pub fn set_reporter(reporter: Option<Reporter>) {
    REPORTER.with(|current| *current.borrow_mut() = reporter);
}

/// The reporter installed on this thread, if any. Only Unix runs report.
#[cfg(unix)]
pub(crate) fn reporter() -> Option<Reporter> {
    REPORTER.with(|current| current.borrow().clone())
}
