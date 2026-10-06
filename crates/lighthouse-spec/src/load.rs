use std::{collections::BTreeMap, fs, path::Path};

use include_dir::{Dir, DirEntry, include_dir};

use crate::Error;

static BUNDLED: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../patterns");

/// Catalog files keyed by `/`-separated path relative to the catalog root.
pub type Files = BTreeMap<String, String>;

pub fn embedded() -> Files {
    let mut files = Files::new();
    collect_embedded(&BUNDLED, &mut files);
    files
}

pub fn read_dir(root: &Path) -> Result<Files, Error> {
    let mut files = Files::new();
    collect_dir(root, root, &mut files)?;
    Ok(files)
}

fn collect_embedded(dir: &Dir, files: &mut Files) {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(sub) => collect_embedded(sub, files),
            DirEntry::File(file) => {
                let text = file.contents_utf8().expect("bundled catalog is UTF-8");
                files.insert(
                    file.path().to_string_lossy().replace('\\', "/"),
                    text.to_owned(),
                );
            }
        }
    }
}

fn collect_dir(root: &Path, dir: &Path, files: &mut Files) -> Result<(), Error> {
    let io = |path: &Path| {
        let path = path.display().to_string();
        move |source| Error::Io { path, source }
    };
    for entry in fs::read_dir(dir).map_err(io(dir))? {
        let path = entry.map_err(io(dir))?.path();
        if path.is_dir() {
            collect_dir(root, &path, files)?;
            continue;
        }
        let text = fs::read_to_string(&path).map_err(io(&path))?;
        let key = path.strip_prefix(root).unwrap_or(&path);
        files.insert(key.to_string_lossy().replace('\\', "/"), text);
    }
    Ok(())
}
