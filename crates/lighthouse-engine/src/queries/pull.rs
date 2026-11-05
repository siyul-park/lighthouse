//! Pulling what a run needs of the files.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use lighthouse_graph::{Db, par_map};

use super::{CONTENT, Content, DIR, FILES, Hash, key};
use crate::Error;

/// What a pull of the files gives.
pub(super) struct Pulled {
    pub files: Vec<String>,
    pub root: Hash,
    pub contents: BTreeMap<PathBuf, Content>,
}

/// Pulls the file list, the root hash and the content of the claimed files:
/// fanned out, on the pool, by the graph's own `par_map`.
pub(super) fn pull(db: &Db, wanted: &(impl Fn(&Path) -> bool + Sync)) -> Result<Pulled, Error> {
    let files = db.query(&FILES, &())?;
    let root_hash = *db.query(&DIR, &String::new())?;
    let claimed: Vec<PathBuf> = files
        .iter()
        .map(PathBuf::from)
        .filter(|rel| wanted(rel))
        .collect();
    let pulled = par_map(db, claimed.clone(), |db, rel| {
        db.query(&CONTENT, &key(rel))
            .map(|content| content.as_ref().clone())
    });
    let contents: Result<BTreeMap<PathBuf, Content>, Error> = claimed
        .into_iter()
        .zip(pulled)
        .map(|(rel, content)| Ok((rel, content?)))
        .collect();
    Ok(Pulled {
        files: files.as_ref().clone(),
        root: root_hash,
        contents: contents?,
    })
}
