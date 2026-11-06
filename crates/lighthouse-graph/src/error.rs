/// Why a pull failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A query that, through the queries it reads, reads itself. The path
    /// names the queries from the first one of the cycle.
    #[error("query cycle: {}", .0.join(" -> "))]
    Cycle(Vec<String>),
    /// Verify mode ran a query again and it gave another value: it reads
    /// something that is not an input.
    #[error("{0} gave another value when run again: it reads outside the graph")]
    Impure(String),
    /// A key or a value that cannot be encoded.
    #[error("cannot encode: {0}")]
    Encode(String),
    /// A kind name registered as one thing and again as another: the name
    /// identifies the nodes of the kind in the store.
    #[error("kind `{name}` is registered as {had} and as {now}")]
    Kind {
        name: String,
        had: String,
        now: String,
    },
    /// A query that could not finish what it was asked: it is not stored, and
    /// the next pull runs it again.
    #[error("{0}")]
    Incomplete(String),
    /// The store failed.
    #[error("store: {0}")]
    Store(#[from] rusqlite::Error),
}
