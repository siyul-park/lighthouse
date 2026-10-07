use std::{collections::BTreeMap, fs, path::Path};

use lighthouse_spec::Catalog;

use crate::Error;

/// Where a project keeps its own rules, relative to the project root.
const LOCAL_DIR: &str = ".lighthouse/rules";

/// Where a project keeps its own rules: `<root>/.lighthouse/rules`.
pub fn local_dir(root: &Path) -> std::path::PathBuf {
    root.join(LOCAL_DIR)
}

/// The catalog layer of `<root>/.lighthouse/rules/*.yaml`; `None` when the
/// directory does not exist.
pub fn load_local(root: &Path) -> Result<Option<Catalog>, Error> {
    match local_files(root)? {
        Some(files) => Ok(Some(Catalog::from_local(files)?)),
        None => Ok(None),
    }
}

/// The texts of `<root>/.lighthouse/rules/*.yaml` by file name; `None` when
/// the directory does not exist.
pub fn local_files(root: &Path) -> Result<Option<BTreeMap<String, String>>, Error> {
    let dir = local_dir(root);
    if !dir.is_dir() {
        return Ok(None);
    }
    let io = |path: &Path, e: std::io::Error| Error::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    };
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(&dir).map_err(|e| io(&dir, e))? {
        let path = entry.map_err(|e| io(&dir, e))?.path();
        if path.extension().is_some_and(|e| e == "yaml") {
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
