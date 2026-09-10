//! Plain data types for talking to the `veripb` subprocess.

/// The result of checking a candidate proof.
pub enum CheckOutcome {
    /// Every line was accepted. `trace` is the checker's captured
    /// output.
    Accepted { trace: String },
    /// Checking stopped at `line`.
    Rejected {
        line: usize,
        /// The checker's error text for `line`.
        message: String,
        /// Any output printed before the rejection.
        trace: String,
    },
}

impl CheckOutcome {
    pub fn is_accepted(&self) -> bool {
        matches!(self, CheckOutcome::Accepted { .. })
    }
}

/// One hint in a `rup` step's minimized hint list.
pub enum RupHint {
    /// A specific already-derived constraint.
    ConstraintId(usize),
    /// The rule's own negation.
    NegatedPremise,
}

/// One live (non-deleted) row of the checker's constraint database.
pub struct DatabaseEntry {
    pub id: usize,
    pub is_core: bool,
    pub text: String,
}

/// The checker's constraint database at some point in a replay.
pub struct Database {
    pub entries: Vec<DatabaseEntry>,
}

impl Database {
    /// Returns the entry with this `id`, if it's still live.
    pub fn get(&self, id: usize) -> Option<&DatabaseEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }
}

/// The checker's best-known objective bounds at some point in a replay.
/// Values are the checker's own arbitrary-precision integer text,
/// unparsed — this REPL only ever splices them into further proof text
/// (`BOUNDS v v`), never does arithmetic on them.
pub struct ObjectiveBounds {
    /// The best objective value logged so far, with or without
    /// checked-deletion guarantees.
    pub best: Option<String>,
    /// The best objective value logged while checked-deletion guarantees
    /// held — the value `conclusion BOUNDS`'s upper-bound check actually
    /// compares against.
    pub best_valid: Option<String>,
}
