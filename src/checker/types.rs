//! Plain data types for talking to the `veripb` subprocess.

/// The result of checking a candidate proof.
pub enum CheckOutcome {
    /// Every line was accepted. `trace` is the checker's stdout.
    Accepted { trace: String },
    /// Checking stopped at `line`.
    Rejected {
        line: usize,
        /// One-line rejection reason (see
        /// [`super::parse::rejection_reason`]).
        message: String,
        /// The checker's full raw output, rejection included.
        trace: String,
    },
}

impl CheckOutcome {
    pub fn is_accepted(&self) -> bool {
        matches!(self, CheckOutcome::Accepted { .. })
    }
}

/// One hint in a `rup` step's hint list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RupHint {
    /// A specific already-derived constraint.
    ConstraintId(usize),
    /// A constraint named by label (`@name`), as typed. veripb's own
    /// elaborated hint lists always use IDs.
    Label(String),
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
/// Values are unparsed arbitrary-precision integer text, only ever spliced
/// into proof text.
pub struct ObjectiveBounds {
    /// The current objective (`<coeff> <lit> ... <constant>`, unparsed),
    /// reflecting any `obju` updates so far; `None` if there is none.
    pub objective: Option<String>,
    /// The best objective value logged so far, with or without
    /// checked-deletion guarantees.
    pub best: Option<String>,
    /// The best objective value logged while checked-deletion guarantees
    /// held; what `conclusion BOUNDS`'s upper bound is checked against.
    pub best_valid: Option<String>,
}
