//! The files a change reaches: besides the files an agent changed, those with
//! an edge into or out of a symbol of them, from a symbol or from a module, in
//! the code model before the edit and after it. What a change does to a
//! neighbour is not known from the change itself: it breaks the callers of a
//! signature, and it takes the last use from a symbol that is then unused, in
//! a file the agent did not touch. The verdict must look at the findings there
//! too.

use std::{collections::BTreeSet, path::PathBuf};

use lighthouse_model::ucm::{Project, Symbol};

use crate::Run;

/// `changed` and the files of the symbols that call or refer to a symbol of
/// them, or that a symbol of them calls or refers to, in either state: the
/// one `before` the edit and the one `after` it.
pub(super) fn reach(changed: &[PathBuf], before: &Run, after: &Run) -> Vec<PathBuf> {
    let mut files: BTreeSet<PathBuf> = changed.iter().cloned().collect();
    for project in [&before.project, &after.project] {
        for path in changed {
            for symbol in project.symbols_in(path) {
                files.extend(neighbours(symbol, project));
            }
        }
    }
    files.into_iter().collect()
}

/// The files of the symbols with an edge into or out of `symbol` in `project`.
fn neighbours<'a>(symbol: &'a Symbol, project: &'a Project) -> impl Iterator<Item = PathBuf> + 'a {
    project
        .callers(&symbol.id)
        .iter()
        .chain(project.references(&symbol.id))
        .chain(project.uses(&symbol.id))
        .filter_map(|id| Some(project.symbol(id)?.file.clone()))
        // Edges that start at a module, such as an import or a call outside
        // any function, have no symbol to name: their sites do.
        .chain(
            project
                .sites(&symbol.id)
                .iter()
                .map(|site| site.file.clone()),
        )
}
