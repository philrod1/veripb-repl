//! Plain data types for talking to the `veripb` subprocess.

/// A candidate proof for `veripb`: text as is, or lines written out one at
/// a time (each followed by a newline) without first joining them.
#[derive(Clone, Copy)]
pub enum ProofText<'a> {
    Text(&'a str),
    Lines {
        preamble: &'a [String],
        body: &'a [String],
        tail: &'a [String],
    },
}

impl<'a> From<&'a str> for ProofText<'a> {
    fn from(text: &'a str) -> Self {
        ProofText::Text(text)
    }
}

impl<'a> From<&'a String> for ProofText<'a> {
    fn from(text: &'a String) -> Self {
        ProofText::Text(text)
    }
}

impl ProofText<'_> {
    /// Lines in all (a last line without a newline counts).
    pub fn line_count(&self) -> usize {
        match self {
            ProofText::Text(text) => text.lines().count(),
            ProofText::Lines {
                preamble,
                body,
                tail,
            } => preamble.len() + body.len() + tail.len(),
        }
    }

    /// Writes the proof to `w`, calling `after_line` after each line; it
    /// returns `false` to stop early.
    pub fn write_to(
        &self,
        w: &mut impl std::io::Write,
        mut after_line: impl FnMut() -> bool,
    ) -> std::io::Result<()> {
        match self {
            ProofText::Text(text) => {
                for line in text.split_inclusive('\n') {
                    w.write_all(line.as_bytes())?;
                    if !after_line() {
                        break;
                    }
                }
            }
            ProofText::Lines {
                preamble,
                body,
                tail,
            } => {
                for line in preamble.iter().chain(body.iter()).chain(tail.iter()) {
                    w.write_all(line.as_bytes())?;
                    w.write_all(b"\n")?;
                    if !after_line() {
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}

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
#[derive(Clone)]
pub struct DatabaseEntry {
    pub id: usize,
    pub is_core: bool,
    pub text: String,
}

/// The checker's constraint database at some point in a replay.
#[derive(Clone)]
pub struct Database {
    /// In ascending `id` order, as `--dump-database` writes them.
    pub entries: Vec<DatabaseEntry>,
}

impl Database {
    /// Returns the entry with this `id`, if it's still live.
    pub fn get(&self, id: usize) -> Option<&DatabaseEntry> {
        let idx = self
            .entries
            .binary_search_by_key(&id, |entry| entry.id)
            .ok()?;
        Some(&self.entries[idx])
    }
}

/// The checker's best-known objective bounds at some point in a replay.
/// Values are unparsed arbitrary-precision integer text, only ever spliced
/// into proof text.
#[derive(Clone, Debug, PartialEq)]
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
