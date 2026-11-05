//! What an importer sees of a unit: the digest of its files without the bodies
//! of their functions. A body edit leaves it as it was, which is the early
//! cutoff; anything else of a file is in it.

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
/// the project can see of it.
fn skeleton(file: &SourceFile) -> String {
    let mut bodies = Vec::new();
    collect_bodies(file, &file.ast.items, &mut bodies);
    let mut kept = String::with_capacity(file.text.len());
    let mut at = 0;
    for (start, end) in bodies {
        if start < at
            || end > file.text.len()
            || !file.text.is_char_boundary(start)
            || !file.text.is_char_boundary(end)
        {
            return digest(&[&file.text]);
        }
        kept.push_str(&file.text[at..start]);
        at = end;
    }
    kept.push_str(&file.text[at..]);
    digest(&[&kept])
}

fn collect_bodies(file: &SourceFile, items: &[Item], out: &mut Vec<(usize, usize)>) {
    for item in items {
        match item {
            Item::Fn(f) => out.extend(span_of(file, &f.block)),
            Item::Impl(i) => {
                for member in &i.items {
                    if let ImplItem::Fn(f) = member {
                        out.extend(span_of(file, &f.block));
                    }
                }
            }
            Item::Trait(t) => {
                for member in &t.items {
                    if let TraitItem::Fn(f) = member
                        && let Some(block) = &f.default
                    {
                        out.extend(span_of(file, block));
                    }
                }
            }
            Item::Mod(m) => {
                if let Some((_, nested)) = &m.content {
                    collect_bodies(file, nested, out);
                }
            }
            _ => {}
        }
    }
}

/// The byte range of a block, braces included.
fn span_of(file: &SourceFile, block: &syn::Block) -> Option<(usize, usize)> {
    let open = block.brace_token.span.open().start();
    let close = block.brace_token.span.close().end();
    Some((
        file.offset(open.line, open.column)?,
        file.offset(close.line, close.column)?,
    ))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
