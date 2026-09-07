//! Plain data types for talking to the `veripb` subprocess.


/// The result of checking a candidate proof — some prefix of lines
/// accepted, then either everything supplied got through cleanly or one
/// specific line broke it. This is the result of a *successful* invocation
/// of the checker.
pub enum CheckOutcome {
    /// Every supplied line was accepted. `trace` is whatever the checker
    /// printed along the way captured verbatim, not reformatted.
    Accepted { trace: String },
    Rejected {
        line: usize,
        message: String,
        trace: String,
    },
}

impl CheckOutcome {
    pub fn is_accepted(&self) -> bool {
        matches!(self, CheckOutcome::Accepted { .. })
    }
}

/// One hint in a `rup` step's minimised hint list — either a specific
/// already-derived constraint, or the rule's own negation.
pub enum RupHint {
    ConstraintId(usize),
    NegatedPremise,
}
