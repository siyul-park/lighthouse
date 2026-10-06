use std::{collections::BTreeMap, fs, path::Path};

use lighthouse_spec::Catalog;

use crate::Error;

/// Where a project keeps its own rules, relative to the project root.
pub const LOCAL_DIR: &str = ".lighthouse/rules";

/// The catalog layer of `<root>/.lighthouse/rules/*.yaml`; `None` when the
/// directory does not exist.
pub fn load_local(root: &Path) -> Result<Option<Catalog>, Error> {
    let dir = root.join(LOCAL_DIR);
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
    Ok(Some(Catalog::from_local(files)?))
}
