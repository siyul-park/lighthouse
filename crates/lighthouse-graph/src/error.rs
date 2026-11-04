/// Why a pull failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A query that, through the queries it reads, reads itself. The path
    /// names the queries from the first one of the cycle.
    #[error("query cycle: {}", .0.join(" -> "))]
    Cycle(Vec<String>),
    /// An input that a query read was not set in this process.
    #[error("input {0} was never set")]
    MissingInput(String),
    /// Verify mode ran a query again and it gave another value: it reads
    /// something that is not an input.
    #[error("{0} gave another value when run again: it reads outside the graph")]
    Impure(String),
    /// A key or a value that cannot be encoded.
    #[error("cannot encode: {0}")]
    Encode(String),
    /// The store failed.
    #[error("store: {0}")]
    Store(#[from] rusqlite::Error),
}
