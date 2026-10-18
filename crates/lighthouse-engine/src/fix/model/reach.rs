//! The files a change reaches: besides the files an agent changed, those that
//! refer to a symbol of them whose signature changed. A change to a signature
//! breaks its callers, which the agent did not touch if it forgot them, so the
//! verdict must look there too.

use std::{collections::BTreeSet, path::PathBuf};

use lighthouse_model::ucm::{Project, Symbol};

use crate::Run;

/// `changed` and the files that call or refer to a symbol of them whose
/// signature differs between the state `before` and the one `after`. Where
/// the code model holds no signature for a symbol (a type, or a language that
/// reports none), any use of it counts: the files with an edge into it.
pub(super) fn reach(changed: &[PathBuf], before: &Run, after: &Run) -> Vec<PathBuf> {
    let mut files: BTreeSet<PathBuf> = changed.iter().cloned().collect();
    for path in changed {
        for symbol in before.project.symbols_in(path) {
            if signature_changed(symbol, before, after) {
                files.extend(users(symbol, &before.project));
                files.extend(users(symbol, &after.project));
            }
        }
    }
    files.into_iter().collect()
}

/// Whether the signature of `symbol` differs between the two states: it is
/// gone, or the summaries of both disagree, or one of them has none.
fn signature_changed(symbol: &Symbol, before: &Run, after: &Run) -> bool {
    match (
        before.project.function(&symbol.id),
        after.project.function(&symbol.id),
    ) {
        (Some(was), Some(is)) => was.signature != is.signature,
        _ => true,
    }
}

/// The files of the symbols that call or refer to `symbol` in `project`.
fn users<'a>(symbol: &'a Symbol, project: &'a Project) -> impl Iterator<Item = PathBuf> + 'a {
    project
        .callers(&symbol.id)
        .iter()
        .chain(project.references(&symbol.id))
        .filter_map(|id| Some(project.symbol(id)?.file.clone()))
}
