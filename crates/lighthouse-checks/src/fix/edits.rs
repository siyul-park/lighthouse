//! The edits a `cel` fix returns, read and lowered to the edit IR: the six
//! constructors are the whole vocabulary, and what each means over the code
//! model is decided here, never in a decision.

use std::path::{Path, PathBuf};

use lighthouse_model::{
    Position, Span, eol_of,
    ucm::{Anchor, EditOp, Node, Owner, Project, SymbolId},
};
use serde::Deserialize;
use serde_json::Value;

/// The result of lowering: edit operations, or why the edits cannot be made.
pub(crate) type Lowered = Result<Vec<EditOp>, String>;

/// A symbol, as its id or as the node that carries it.
#[derive(Clone, Deserialize)]
#[serde(untagged)]
enum Symbol {
    Id(String),
    Node { id: String },
}

/// A range of text, in the file it names or else the file of the finding.
#[derive(Deserialize)]
struct Range {
    #[serde(default)]
    file: Option<PathBuf>,
    start: Position,
    end: Position,
}

/// What an edit changes: a span of text, else a declaration.
#[derive(Deserialize)]
#[serde(untagged)]
enum Target {
    Range(Range),
    Symbol(Symbol),
}

/// A file, as its path or as the value that carries it.
#[derive(Deserialize)]
#[serde(untagged)]
enum File {
    Path(PathBuf),
    Value { path: PathBuf },
}

/// Where `add` and `move` put their text or declaration.
#[derive(Deserialize)]
#[serde(tag = "edit", rename_all = "lowercase")]
enum Where {
    Before { node: Symbol },
    After { node: Symbol },
    Start { file: File },
    End { file: File },
}

#[derive(Deserialize)]
#[serde(tag = "edit", rename_all = "lowercase")]
enum Edit {
    Remove { target: Target },
    Add { anchor: Where, text: String },
    Replace { target: Target, text: String },
    Move { node: Symbol, anchor: Where },
    Rename { symbol: Symbol, name: String },
    Reorder { nodes: Vec<Symbol> },
}

/// What the lowering reads: the project, the file of the finding and its text.
pub(crate) struct Place<'a> {
    pub project: &'a Project,
    pub file: &'a Path,
    pub text: &'a str,
}

/// The IR operations for the edit values `edits` returned, in order. An
/// empty list is no edit.
pub(crate) fn lower(edits: &[Value], place: &Place) -> Lowered {
    let mut ops = Vec::new();
    for value in edits {
        let edit: Edit = serde_json::from_value(value.clone())
            .map_err(|e| format!("`edits` holds {value}, which is no edit: {e}"))?;
        ops.extend(one(edit, place)?);
    }
    Ok(ops)
}

fn one(edit: Edit, place: &Place) -> Lowered {
    Ok(match edit {
        Edit::Remove {
            target: Target::Symbol(symbol),
        } => vec![EditOp::Delete {
            node: node(&symbol)?,
        }],
        Edit::Remove {
            target: Target::Range(range),
        } => vec![EditOp::DeleteRange {
            span: span(&range),
            file: range.file.unwrap_or_else(|| place.file.to_owned()),
        }],
        Edit::Replace { target, text } => {
            let (file, span) = region(&target, place)?;
            vec![EditOp::Replace { file, span, text }]
        }
        Edit::Add { anchor, text } => {
            let (file, at) = insertion(&anchor, place)?;
            vec![EditOp::Replace {
                file,
                span: Span { start: at, end: at },
                text: whole_lines(text, place.text),
            }]
        }
        Edit::Move {
            node: moved,
            anchor,
        } => {
            let anchor = match anchor {
                Where::Before { node: next } => Anchor::Before(node(&next)?),
                Where::After { node: prev } => Anchor::After(node(&prev)?),
                Where::Start { .. } | Where::End { .. } => {
                    return Err("`move` goes before or after a declaration".to_owned());
                }
            };
            vec![EditOp::Move {
                node: node(&moved)?,
                anchor,
            }]
        }
        Edit::Rename { symbol, name } => {
            let Node::Symbol(symbol) = node(&symbol)? else {
                return Err("`rename` needs a symbol".to_owned());
            };
            vec![EditOp::Rename { symbol, name }]
        }
        Edit::Reorder { nodes } => reorder(&nodes, place)?,
    })
}

/// `text` ending in the line terminator of the file, as an insertion of whole
/// lines does.
fn whole_lines(mut text: String, file: &str) -> String {
    if !text.ends_with('\n') {
        text.push_str(eol_of(file));
    }
    text
}

fn node(symbol: &Symbol) -> Result<Node, String> {
    let (Symbol::Id(id) | Symbol::Node { id }) = symbol;
    SymbolId::parse(id)
        .map(Node::Symbol)
        .ok_or_else(|| format!("`{id}` is not a symbol id"))
}

fn span(range: &Range) -> Span {
    Span {
        start: range.start,
        end: range.end,
    }
}

/// The file and the span a `replace` changes: the span itself, or the extent
/// of a declaration.
fn region(target: &Target, place: &Place) -> Result<(PathBuf, Span), String> {
    match target {
        Target::Range(range) => Ok((
            range.file.clone().unwrap_or_else(|| place.file.to_owned()),
            span(range),
        )),
        Target::Symbol(symbol) => {
            let Node::Symbol(id) = node(symbol)? else {
                return Err("`replace` needs a symbol".to_owned());
            };
            let found = place
                .project
                .symbol(&id)
                .ok_or_else(|| format!("`{}` is not in the project", id.as_str()))?;
            // Without its extent only the name is known, not the declaration.
            let extent = found.extent.ok_or_else(|| {
                format!(
                    "`{}` has no extent: its language provider does not report declaration extents",
                    found.name
                )
            })?;
            Ok((found.file.clone(), extent))
        }
    }
}

/// The file and the position an `add` inserts at: the start of the line of a
/// declaration, the start of the line after it, or either end of the finding's
/// file.
fn insertion(anchor: &Where, place: &Place) -> Result<(PathBuf, Position), String> {
    let line_start = |line: u32| Position { line, col: 1 };
    match anchor {
        Where::Before { node: next } => {
            let (file, span) = region(&Target::Symbol(next.clone()), place)?;
            Ok((file, line_start(span.start.line)))
        }
        Where::After { node: prev } => {
            let (file, span) = region(&Target::Symbol(prev.clone()), place)?;
            Ok((file, line_start(span.end.line + 1)))
        }
        Where::Start { file } => Ok((path(file, place)?, line_start(1))),
        Where::End { file } => {
            let file = path(file, place)?;
            let lines = place.text.lines().count();
            let last = place.text.lines().last().unwrap_or("");
            let at = if place.text.ends_with('\n') || place.text.is_empty() {
                line_start(u32::try_from(lines + 1).unwrap_or(u32::MAX))
            } else {
                Position {
                    line: u32::try_from(lines).unwrap_or(u32::MAX),
                    col: u32::try_from(last.len() + 1).unwrap_or(u32::MAX),
                }
            };
            Ok((file, at))
        }
    }
}

/// The path of a file the finding's file is, the only one whose text is read.
fn path(file: &File, place: &Place) -> Result<PathBuf, String> {
    let (File::Path(path) | File::Value { path }) = file;
    if path == place.file {
        Ok(path.clone())
    } else {
        Err("`add` at the start or end of a file works on the file of the finding".to_owned())
    }
}

/// Puts the nodes in the order given, in the places they hold. Nothing is
/// changed when they already are in it.
fn reorder(nodes: &[Symbol], place: &Place) -> Lowered {
    let ids: Vec<SymbolId> = nodes
        .iter()
        .map(|symbol| match node(symbol)? {
            Node::Symbol(id) => Ok(id),
            Node::Module(_) => Err("`reorder` needs symbols".to_owned()),
        })
        .collect::<Result<_, String>>()?;
    let found: Vec<_> = ids
        .iter()
        .map(|id| {
            place
                .project
                .symbol(id)
                .ok_or_else(|| format!("`{}` is not in the project", id.as_str()))
        })
        .collect::<Result<_, _>>()?;
    let Some(first) = found.first() else {
        return Ok(Vec::new());
    };
    let mut places: Vec<_> = found.iter().map(|s| (s.span.start, &s.id)).collect();
    places.sort();
    let held: Vec<&SymbolId> = places.into_iter().map(|(_, id)| id).collect();
    if held.iter().zip(&ids).all(|(held, wanted)| *held == wanted) {
        return Ok(Vec::new());
    }
    let owner = match &first.owner {
        Some(owner) if found.iter().all(|s| s.owner.as_ref() == Some(owner)) => {
            Owner::Symbol(owner.clone())
        }
        _ => Owner::File(first.file.clone()),
    };
    Ok(vec![EditOp::Reorder {
        owner,
        order: ids.into_iter().map(Node::Symbol).collect(),
    }])
}
