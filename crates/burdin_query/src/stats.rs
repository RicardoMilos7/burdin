//! What the database did: per-revision and cumulative counters. They are the evidence for the incremental claims
//! (`spec/query.md`, QL5): a test edits one function and asserts that exactly one body was checked.

/// What one analysis did.
#[derive(Clone, Debug, Default)]
pub struct RevisionStats {
    /// Files parsed from scratch, by path.
    pub files_parsed: Vec<String>,
    /// Files whose syntax tree was updated by reparsing one function body, by path.
    pub bodies_reparsed: Vec<String>,
    /// Files whose syntax tree was reused as it was.
    pub files_reused: usize,
    /// The functions and constants checked from scratch, each once, in the order they were first checked.
    pub bodies_checked: Vec<String>,
    /// How many body checks ran in all (a generic body is checked once per pass that needs it).
    pub body_checks: usize,
    /// How many bodies were taken from the memo.
    pub bodies_reused: usize,
    /// How many passes over the bodies the checker made.
    pub passes: usize,
    /// The functions whose move and borrow checks ran from scratch, by name.
    pub memory_checked: Vec<String>,
    /// How many functions' move and borrow results were taken from the memo.
    pub memory_reused: usize,
}

/// What every analysis of a database did, summed.
#[derive(Clone, Debug, Default)]
pub struct Counters {
    pub analyses: usize,
    pub parses: usize,
    pub body_reparses: usize,
    pub bodies_checked: usize,
    pub bodies_reused: usize,
    pub memory_checked: usize,
    pub memory_reused: usize,
}
