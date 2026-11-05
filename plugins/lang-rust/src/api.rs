//! What an importer sees of a unit: the digest of its files without the bodies
//! of their functions. A body edit leaves it as it was, which is the early
//! cutoff; anything else of a file is in it.

use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};
use quote::ToTokens;
use sha2::{Digest, Sha256};
use syn::{ImplItem, Item, TraitItem};

use crate::project::SourceFile;

/// The `api` of a unit: the skeletons of its files, in order of path.
pub(crate) fn of(files: &[&SourceFile]) -> String {
    let mut shapes: Vec<(&str, String)> = files
        .iter()
        .map(|f| (f.rel.as_str(), skeleton(f)))
        .collect();
    shapes.sort();
    let parts: Vec<&str> = shapes
        .iter()
        .flat_map(|(rel, shape)| [*rel, shape.as_str()])
        .collect();
    digest(&parts)
}

pub(crate) fn digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.len().to_string().as_bytes());
        hasher.update(b":");
        hasher.update(part.as_bytes());
    }
    hex(&hasher.finalize())
}

/// The digest of a file without the bodies of its functions: what the rest of
/// the project can see of it. It is the token stream of the declarations, so
/// comments and whitespace are not in it, and the doc comments, which are
/// facts of the fragment, are taken out.
fn skeleton(file: &SourceFile) -> String {
    let mut items = file.ast.items.clone();
    strip_bodies(&mut items);
    let mut tokens = TokenStream::new();
    for attr in file.ast.attrs.iter().filter(|a| !a.path().is_ident("doc")) {
        attr.to_tokens(&mut tokens);
    }
    for item in &items {
        item.to_tokens(&mut tokens);
    }
    digest(&[&undocumented(tokens).to_string()])
}

/// Empties the bodies of the functions of `items`, nested modules included.
fn strip_bodies(items: &mut [Item]) {
    for item in items {
        match item {
            Item::Fn(f) => f.block.stmts.clear(),
            Item::Impl(i) => {
                for member in &mut i.items {
                    if let ImplItem::Fn(f) = member {
                        f.block.stmts.clear();
                    }
                }
            }
            Item::Trait(t) => {
                for member in &mut t.items {
                    if let TraitItem::Fn(f) = member
                        && let Some(block) = &mut f.default
                    {
                        block.stmts.clear();
                    }
                }
            }
            Item::Mod(m) => {
                if let Some((_, nested)) = &mut m.content {
                    strip_bodies(nested);
                }
            }
            _ => {}
        }
    }
}

/// The tokens without the `#[doc = ...]` and `#![doc = ...]` attributes, which
/// is what a doc comment is made into.
fn undocumented(tokens: TokenStream) -> TokenStream {
    let mut out = Vec::new();
    let mut iter = tokens.into_iter().peekable();
    while let Some(token) = iter.next() {
        let TokenTree::Punct(hash) = &token else {
            out.push(match token {
                TokenTree::Group(group) => {
                    let mut kept = Group::new(group.delimiter(), undocumented(group.stream()));
                    kept.set_span(group.span());
                    TokenTree::Group(kept)
                }
                other => other,
            });
            continue;
        };
        if hash.as_char() != '#' {
            out.push(token);
            continue;
        }
        let mut ahead = iter.clone();
        let inner = matches!(ahead.peek(), Some(TokenTree::Punct(p)) if p.as_char() == '!');
        if inner {
            ahead.next();
        }
        let doc = matches!(
            ahead.peek(),
            Some(TokenTree::Group(group))
                if group.delimiter() == Delimiter::Bracket
                    && matches!(group.stream().into_iter().next(),
                        Some(TokenTree::Ident(name)) if name == "doc")
        );
        if doc {
            iter = ahead;
            iter.next();
        } else {
            out.push(token);
        }
    }
    out.into_iter().collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests;
