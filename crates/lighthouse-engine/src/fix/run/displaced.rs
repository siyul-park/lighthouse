//! What a move may not change: the symbols of a file. A fix that only moves
//! declarations keeps their text, so the line guard of the edits cannot see a
//! declaration that changes scope with it: in a language that brackets nothing,
//! a `def` put in a class is a method, and one put after a `return` is dead
//! code. The check of the changed texts has re-indexed them already, so the
//! symbols of a file before and after are compared there.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use lighthouse_model::ucm::{LoadPoint, Project, SymbolId, Visibility};

use super::super::propose::Proposal;

/// A symbol as far as moving it may not change it. Its id is not part of it:
/// some providers put the line of a symbol in its id (a Go `init`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Shape {
    name: String,
    kind: &'static str,
    owner: Option<SymbolId>,
    visibility: u8,
    test: bool,
}

/// The files whose edits in `kept` are all of fixes that only move
/// declarations.
pub(super) fn moved_files(kept: &[Proposal]) -> BTreeSet<&PathBuf> {
    let mut only: BTreeMap<&PathBuf, bool> = BTreeMap::new();
    for p in kept {
        for edit in &p.edits {
            *only.entry(&edit.file).or_insert(true) &= p.moves_only;
        }
    }
    only.into_iter()
        .filter(|(_, moves)| *moves)
        .map(|(file, _)| file)
        .collect()
}

/// The files among `files` whose symbols or test cases are not the same in
/// `before` and `after`, with the first change.
pub(super) fn displaced(
    files: &BTreeSet<&PathBuf>,
    (before, after): (&Project, &Project),
) -> BTreeMap<PathBuf, String> {
    files
        .iter()
        .filter(|file| before.file(file).is_some())
        .filter_map(|file| {
            let why = first_change(before, after, file)?;
            Some(((*file).clone(), why))
        })
        .collect()
}

/// The files among `files` whose code, as it loads, needs a name bound below
/// it after the fix and did not before, with the first such need: a fix of any
/// kind may put a declaration where the language evaluates it too early.
pub(super) fn unordered(
    files: &[PathBuf],
    (before, after): (&Project, &Project),
) -> BTreeMap<PathBuf, String> {
    files
        .iter()
        .filter_map(|file| {
            let was: BTreeSet<_> = before.load_breaks(file).into_iter().collect();
            let broken = after
                .load_breaks(file)
                .into_iter()
                .find(|need| !was.iter().any(|w| w.key() == need.key()))?;
            let name = |id: &SymbolId| after.symbol(id).map_or_else(|| id.as_str().to_owned(), |s| s.name.clone());
            let who = match &broken.user {
                LoadPoint::Declaration(id) => format!("`{}`", name(id)),
                LoadPoint::Site(site) => format!("the code of the file on line {}", site.start.line),
            };
            Some((
                file.clone(),
                format!(
                    "the fix would break the load order of the file: {who} needs `{}` bound above it as the file loads",
                    name(&broken.binding)
                ),
            ))
        })
        .collect()
}

/// The first symbol the file has before and not after, or after and not
/// before, as the sentence that says what the move would do.
fn first_change(before: &Project, after: &Project, file: &Path) -> Option<String> {
    let (was, is) = (shapes(before, file), shapes(after, file));
    let mut left: Vec<&Shape> = is.iter().collect();
    let mut gone: Vec<&Shape> = Vec::new();
    for shape in &was {
        match left.iter().position(|s| *s == shape) {
            Some(at) => {
                left.remove(at);
            }
            None => gone.push(shape),
        }
    }
    let Some(gone_shape) = gone.first() else {
        return left
            .first()
            .map(|added| format!("the move would add `{}`", added.name));
    };
    let arrived = left
        .iter()
        .find(|s| s.name == gone_shape.name && s.kind == gone_shape.kind);
    Some(match arrived {
        Some(now) if now.owner != gone_shape.owner => owner_change(
            &gone_shape.name,
            (before, gone_shape.owner.as_ref()),
            (after, now.owner.as_ref()),
        ),
        Some(_) => format!(
            "the move would change `{}`: its visibility or test case",
            gone_shape.name
        ),
        None => format!("the move would drop `{}`", gone_shape.name),
    })
}

/// The shapes of the symbols of a file, in a fixed order.
fn shapes(project: &Project, file: &Path) -> Vec<Shape> {
    let mut found: Vec<Shape> = project
        .symbols_in(file)
        .map(|s| Shape {
            name: s.name.clone(),
            kind: s.kind.as_str(),
            owner: s.owner.clone(),
            visibility: match s.visibility {
                Visibility::Public => 0,
                Visibility::Private => 1,
                Visibility::Internal => 2,
            },
            test: project.test(&s.id).is_some(),
        })
        .collect();
    found.sort();
    found
}

fn owner_change(
    name: &str,
    was: (&Project, Option<&SymbolId>),
    now: (&Project, Option<&SymbolId>),
) -> String {
    format!(
        "the move would change `{name}`: owner {} → {}",
        owner_name(was.0, was.1),
        owner_name(now.0, now.1)
    )
}

/// The name of an owner, or `the file`.
fn owner_name(project: &Project, owner: Option<&SymbolId>) -> String {
    owner
        .and_then(|id| project.symbol(id))
        .map_or_else(|| "the file".to_owned(), |s| format!("`{}`", s.name))
}

#[cfg(test)]
mod tests;
