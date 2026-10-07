//! Spans, doc comments and token counts, the small facts every symbol needs.

use lighthouse_protocol::{Position, Span};
use proc_macro2::{Spacing, TokenStream, TokenTree};
use quote::ToTokens;
use syn::{Attribute, Expr, ExprLit, Lit, Meta};

use crate::tree::SourceFile;

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
/// comments included, to its last.
pub fn extent_of(file: &SourceFile, item: &impl ToTokens) -> Span {
    let trees: Vec<TokenTree> = item.to_token_stream().into_iter().collect();
    let (Some(first), Some(last)) = (trees.first(), trees.last()) else {
        return Span {
            start: Position { line: 1, col: 1 },
            end: Position { line: 1, col: 1 },
        };
    };
    range(file, first.span().start(), last.span().end())
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

/// Doc comment text: `///` and `#[doc = "..."]` lines, one leading space of
/// each removed, trimmed.
pub fn doc_of(attrs: &[Attribute]) -> Option<String> {
    let lines: Vec<String> = attrs
        .iter()
        .filter_map(|a| {
            let Meta::NameValue(nv) = &a.meta else {
                return None;
            };
            let Expr::Lit(ExprLit {
                lit: Lit::Str(text),
                ..
            }) = &nv.value
            else {
                return None;
            };
            nv.path.is_ident("doc").then(|| text.value())
        })
        .collect();
    let text = lines
        .iter()
        .map(|l| l.strip_prefix(' ').unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Leaf tokens: identifiers, literals, operators (a run of joint punctuation
/// is one operator) and each delimiter.
pub fn count_tokens(stream: TokenStream) -> u32 {
    let mut count = 0u32;
    let mut joined = false;
    for tree in stream {
        match tree {
            TokenTree::Group(group) => {
                joined = false;
                let delimiters = if group.delimiter() == proc_macro2::Delimiter::None {
                    0
                } else {
                    2
                };
                count = count.saturating_add(delimiters + count_tokens(group.stream()));
            }
            TokenTree::Punct(punct) => {
                if !joined {
                    count = count.saturating_add(1);
                }
                joined = punct.spacing() == Spacing::Joint;
            }
            _ => {
                joined = false;
                count = count.saturating_add(1);
            }
        }
    }
    count
}

/// Parameters other than the receiver, and results (a tuple counts each
/// element, `()` and no return type count none).
pub fn signature_counts(sig: &syn::Signature) -> (u32, u32) {
    let params = sig
        .inputs
        .iter()
        .filter(|a| matches!(a, syn::FnArg::Typed(_)))
        .count();
    let returns = match &sig.output {
        syn::ReturnType::Default => 0,
        syn::ReturnType::Type(_, ty) => match &**ty {
            syn::Type::Tuple(t) => t.elems.len(),
            _ => 1,
        },
    };
    (
        u32::try_from(params).unwrap_or(u32::MAX),
        u32::try_from(returns).unwrap_or(u32::MAX),
    )
}

/// An attribute's path as written, `tokio::test`.
pub fn attr_path(attr: &Attribute) -> String {
    attr.path()
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}
