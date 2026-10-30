//! A fence's fragment, read from the text the compiler reads, put where the
//! host file shows it: positions are the host's, and what the host does not
//! show (a hidden line, the wrapper) is analysed and never reported.

use std::collections::BTreeSet;

use lighthouse_protocol::{FileInfo, Fragment, Node, Position, Span};

use super::text::Text;
use crate::project::SourceFile;

/// The lines of a host file, to turn the byte offsets of a mapping into
/// positions.
pub(super) struct Lines {
    starts: Vec<usize>,
}

impl Lines {
    pub(super) fn of(text: &str) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        Self { starts }
    }

    pub(super) fn position(&self, offset: usize) -> Position {
        let line = self.starts.partition_point(|start| *start <= offset);
        let start = self.starts.get(line.wrapping_sub(1)).copied().unwrap_or(0);
        Position {
            line: u32::try_from(line).unwrap_or(u32::MAX),
            col: u32::try_from(offset - start + 1).unwrap_or(u32::MAX),
        }
    }
}

/// Where the spans of one fence's file lie in its host file.
pub(super) struct Remap<'a> {
    pub(super) text: &'a Text,
    pub(super) source: &'a SourceFile,
    pub(super) lines: &'a Lines,
}

impl Remap<'_> {
    /// The host span of a span of the fence's file; `None` where the host shows
    /// nothing of it.
    fn span(&self, span: Span) -> Option<Span> {
        let [from, to] = [span.start, span.end].map(|at| self.offset(at));
        let [start, end] = self.text.host(from?, to?)?;
        Some(Span {
            start: self.lines.position(start),
            end: self.lines.position(end),
        })
    }

    fn offset(&self, at: Position) -> Option<usize> {
        let line = self
            .source
            .line_starts
            .get((at.line as usize).checked_sub(1)?)?;
        Some(line + (at.col as usize).checked_sub(1)?)
    }

    /// `fragment` as the host file shows it, with the file `path` as its own.
    /// A symbol the host does not show is dropped, with what is about it; what
    /// it declared or called is the module's, and a call, a use or a
    /// declaration keeps its place only where the host shows it.
    pub(super) fn fragment(&self, mut fragment: Fragment, path: &str) -> Fragment {
        let module = fragment
            .modules
            .first()
            .map(|module| module.path.clone())
            .unwrap_or_default();
        let mut dropped = BTreeSet::new();
        let mut symbols = Vec::new();
        for mut symbol in std::mem::take(&mut fragment.symbols) {
            match self.span(symbol.span) {
                Some(span) => {
                    symbol.span = span;
                    symbol.file = path.to_owned();
                    symbol.extent = symbol.extent.and_then(|extent| self.span(extent));
                    symbols.push(symbol);
                }
                None => {
                    dropped.insert(symbol.id);
                }
            }
        }
        for symbol in &mut symbols {
            if symbol
                .owner
                .as_ref()
                .is_some_and(|owner| dropped.contains(owner))
            {
                symbol.owner = None;
            }
        }
        fragment.symbols = symbols;
        fragment.functions.retain_mut(|function| {
            if dropped.contains(&function.symbol) {
                return false;
            }
            function.events.retain_mut(|event| {
                self.span(event.span)
                    .map(|span| event.span = span)
                    .is_some()
            });
            true
        });
        fragment
            .tests
            .retain(|test| !dropped.contains(&test.symbol));
        for edge in &mut fragment.edges {
            if matches!(&edge.from, Node::Symbol(id) if dropped.contains(id)) {
                edge.from = Node::Module(module.clone());
            }
            edge.site = edge.site.and_then(|site| self.span(site));
        }
        fragment.edges.retain(|edge| !dropped.contains(&edge.to));
        // The comments the host shows, at their place; the text is the
        // fence's, without the indentation of a list item.
        fragment.comments.retain_mut(|comment| {
            let Some(span) = self.span(comment.span) else {
                return false;
            };
            comment.span = span;
            if comment
                .attached_to
                .as_ref()
                .is_some_and(|id| dropped.contains(id))
            {
                comment.attached_to = None;
            }
            true
        });
        fragment.file = FileInfo {
            path: path.to_owned(),
            generated: false,
            facts: Default::default(),
        };
        fragment
    }
}
