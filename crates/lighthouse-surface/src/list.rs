//! `--limit` on listings: the first N items and how many were left out.

/// A cut listing.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// How many items `limit` left out.
    pub omitted: usize,
}

/// The first `limit` of `items`, or all of them.
pub fn page<T>(mut items: Vec<T>, limit: Option<usize>) -> Page<T> {
    let omitted = limit.map_or(0, |limit| items.len().saturating_sub(limit));
    items.truncate(items.len() - omitted);
    Page { items, omitted }
}

/// The line of a text listing that says what `--limit` left out; empty when
/// nothing was.
pub fn omitted_line(omitted: usize) -> String {
    if omitted == 0 {
        return String::new();
    }
    format!("... {omitted} more not shown (raise --limit)\n")
}
