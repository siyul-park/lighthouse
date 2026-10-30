//! Source spans of syntax nodes, in the bytes and lines the protocol uses.

use lighthouse_protocol::{Position, Span};
use proc_macro2::TokenTree;
use quote::ToTokens;

use crate::project::SourceFile;

/// Span of an item from its first token after the attributes to its last
/// token. proc-macro2 counts columns in characters; the protocol wants bytes.
pub fn span_of(file: &SourceFile, item: &impl ToTokens) -> Span {
    let trees: Vec<TokenTree> = item.to_token_stream().into_iter().collect();
    let mut at = 0;
    while at + 1 < trees.len() && is_attribute(&trees[at], &trees[at + 1]) {
        at += 2;
    }
    let (Some(first), Some(last)) = (trees.get(at), trees.last()) else {
        return Span {
            start: Position { line: 1, col: 1 },
            end: Position { line: 1, col: 1 },
        };
    };
    let (start, end) = (first.span().start(), last.span().end());
    let (line, col) = file.position(start.line, start.column);
    let (end_line, end_col) = file.position(end.line, end.column);
    Span {
        start: Position { line, col },
        end: Position {
            line: end_line,
            col: end_col,
        },
    }
}

/// Span of the whole item: from its first token, outer attributes and doc
/// comments included, to its last, with the plain `//` comment lines that lead
/// it with no blank line between, so a comment moves with its item.
pub fn extent_of(file: &SourceFile, item: &impl ToTokens) -> Span {
    let trees: Vec<TokenTree> = item.to_token_stream().into_iter().collect();
    let (Some(first), Some(last)) = (trees.first(), trees.last()) else {
        return Span {
            start: Position { line: 1, col: 1 },
            end: Position { line: 1, col: 1 },
        };
    };
    let mut extent = range(file, first.span().start(), last.span().end());
    if let Some((line, col)) = file.leading_comments(first.span().start().line) {
        extent.start = Position { line, col };
    }
    extent
}

/// Span of an identifier, which is where a reference to it sits.
pub fn ident_span(file: &SourceFile, ident: &proc_macro2::Ident) -> Span {
    let span = ident.span();
    range(file, span.start(), span.end())
}

fn range(file: &SourceFile, start: proc_macro2::LineColumn, end: proc_macro2::LineColumn) -> Span {
    let (line, col) = file.position(start.line, start.column);
    let (end_line, end_col) = file.position(end.line, end.column);
    Span {
        start: Position { line, col },
        end: Position {
            line: end_line,
            col: end_col,
        },
    }
}

fn is_attribute(hash: &TokenTree, group: &TokenTree) -> bool {
    matches!(hash, TokenTree::Punct(p) if p.as_char() == '#')
        && matches!(group, TokenTree::Group(g) if g.delimiter() == proc_macro2::Delimiter::Bracket)
}
