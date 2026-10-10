//! Where a project keeps its own decisions and how they are read: the
//! project-local layer of the catalog, next to [`Catalog::load`].

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use lighthouse_resource::Format;

use crate::{Catalog, Error};

/// Where a project keeps its own decisions, relative to the project root.
const LOCAL_DIR: &str = ".lighthouse/decisions";

/// Where a project keeps its own decisions: `<root>/.lighthouse/decisions`.
pub fn local_dir(root: &Path) -> PathBuf {
    root.join(LOCAL_DIR)
}

/// The catalog layer of `<root>/.lighthouse/decisions/*.yaml`; `None` when the
/// directory does not exist.
pub fn load_local(root: &Path) -> Result<Option<Catalog>, Error> {
    match local_files(root)? {
        Some(files) => Ok(Some(Catalog::from_local(files)?)),
        None => Ok(None),
    }
}

/// The texts of `<root>/.lighthouse/decisions/*` (YAML, TOML or JSON) by file name; `None` when
/// the directory does not exist.
pub fn local_files(root: &Path) -> Result<Option<BTreeMap<String, String>>, Error> {
    let dir = local_dir(root);
    if !dir.is_dir() {
        return Ok(None);
    }
    let io = |path: &Path, source: std::io::Error| Error::Io {
        path: path.display().to_string(),
        source,
    };
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(&dir).map_err(|e| io(&dir, e))? {
        let path = entry.map_err(|e| io(&dir, e))?.path();
        if Format::of_path(&path).is_some() {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let text = fs::read_to_string(&path).map_err(|e| io(&path, e))?;
            files.insert(name, text);
        }
    }
    Ok(Some(files))
}
