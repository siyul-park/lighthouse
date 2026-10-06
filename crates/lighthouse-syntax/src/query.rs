use std::sync::Arc;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, QueryCursor};

use crate::Error;

/// A compiled query over one grammar.
pub struct Query {
    inner: tree_sitter::Query,
}

/// One pattern match with its captures.
pub struct Match<'t> {
    pub pattern: usize,
    captures: Vec<(u32, Node<'t>)>,
    names: Arc<[String]>,
}

impl Query {
    pub fn new(language: &Language, source: &str) -> Result<Self, Error> {
        tree_sitter::Query::new(language, source)
            .map(|inner| Self { inner })
            .map_err(|e| Error::Query(e.to_string()))
    }

    /// Matches under `root`, in source order.
    pub fn matches<'t>(&self, root: Node<'t>, source: &str) -> Vec<Match<'t>> {
        let names: Arc<[String]> = self
            .inner
            .capture_names()
            .iter()
            .map(|n| (*n).to_owned())
            .collect();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.inner, root, source.as_bytes());
        let mut out = Vec::new();
        while let Some(m) = matches.next() {
            out.push(Match {
                pattern: m.pattern_index,
                captures: m.captures().iter().map(|c| (c.index, c.node)).collect(),
                names: Arc::clone(&names),
            });
        }
        out
    }
}

impl<'t> Match<'t> {
    pub fn get(&self, name: &str) -> Option<Node<'t>> {
        self.all(name).next()
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = Node<'t>> + 'a {
        self.captures
            .iter()
            .filter(move |(i, _)| self.names[*i as usize] == name)
            .map(|(_, n)| *n)
    }
}
