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
