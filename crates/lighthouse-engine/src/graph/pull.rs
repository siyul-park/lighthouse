//! Pulling what a run needs of the files.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use lighthouse_graph::Graph;

use super::{CONTENT, Content, FILES, Hash, TREE, key};
use crate::Error;

/// What a pull of the files gives.
pub(super) struct Pulled {
    pub files: Vec<String>,
    pub tree: Hash,
    pub contents: BTreeMap<PathBuf, Content>,
}

/// Pulls the file list, the content of the claimed files and the root's
/// `tree`, from the bottom up so that every kind a stored node depends on is
/// in use before it is validated: fanned out, on the pool, by the graph's own
/// `par_map`.
pub(super) fn pull(
    graph: &Graph,
    wanted: &(impl Fn(&Path) -> bool + Sync),
) -> Result<Pulled, Error> {
    let files = graph.query(&FILES, &())?;
    let claimed: Vec<PathBuf> = files
        .iter()
        .map(PathBuf::from)
        .filter(|rel| wanted(rel))
        .collect();
    let pulled = graph.par_map(claimed.clone(), |graph, rel| {
        graph
            .query(&CONTENT, &key(rel))
            .map(|content| content.as_ref().clone())
    });
    let contents: Result<BTreeMap<PathBuf, Content>, Error> = claimed
        .into_iter()
        .zip(pulled)
        .map(|(rel, content)| Ok((rel, content?)))
        .collect();
    let tree = *graph.query(&TREE, &String::new())?;
    Ok(Pulled {
        files: files.as_ref().clone(),
        tree,
        contents: contents?,
    })
}
