use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use include_dir::{Dir, DirEntry, include_dir};
use lighthouse_model::{Format, Resource, STATE_DIR, Spec, documents, kind_of, resource};

use crate::{
    Catalog, Decision, DecisionSpec, Env, Error, PackSpec, ProjectSpec,
    sources::{SourceLine, SourceMapSpec},
};

/// The catalogs of the bundled plugins, each keyed from its own root.
static BUNDLED: [Dir; 3] = [
    include_dir!("$CARGO_MANIFEST_DIR/../../plugins/core/decisions"),
    include_dir!("$CARGO_MANIFEST_DIR/../../plugins/design/decisions"),
    include_dir!("$CARGO_MANIFEST_DIR/../../plugins/testing/decisions"),
];

/// Catalog files keyed by `/`-separated path relative to the catalog root.
pub type Files = BTreeMap<String, String>;

/// What a catalog file holds, with the directory it lies in.
pub(super) struct Loaded {
    pub(super) dir: String,
    pub(super) packs: Vec<Resource<PackSpec>>,
    pub(super) decisions: Vec<Decision>,
    pub(super) projects: Vec<Resource<ProjectSpec>>,
    pub(super) sources: Vec<SourceLine>,
}

pub(crate) fn embedded() -> Files {
    let mut files = Files::new();
    for root in &BUNDLED {
        collect_embedded(root, &mut files);
    }
    files
}

pub(super) fn read_dir(root: &Path) -> Result<Files, Error> {
    let mut files = Files::new();
    collect_dir(root, root, &mut files)?;
    Ok(files)
}

/// Where a project keeps its own decisions: `<root>/.lighthouse/decisions`.
pub fn local_dir(root: &Path) -> PathBuf {
    root.join(STATE_DIR).join("decisions")
}

/// The catalog layer of `<root>/.lighthouse/decisions/*.yaml`; `None` when the
/// directory does not exist.
pub fn load_local(root: &Path, cel: &Env) -> Result<Option<Catalog>, Error> {
    match local_files(root)? {
        Some(files) => Ok(Some(Catalog::from_local(files, cel)?)),
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
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(&dir).map_err(Error::io(&dir))? {
        let path = entry.map_err(Error::io(&dir))?.path();
        if Format::of_path(&path).is_some() {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let text = fs::read_to_string(&path).map_err(Error::io(&path))?;
            files.insert(name, text);
        }
    }

    Ok(Some(files))
}

/// The documents of one catalog file; files that are not documents (example
/// sources, anything under `testdata`) hold none.
pub(super) fn read_documents(path: &str, text: &str) -> Result<Loaded, Error> {
    let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir).to_owned();
    let mut loaded = Loaded {
        dir,
        packs: Vec::new(),
        decisions: Vec::new(),
        projects: Vec::new(),
        sources: Vec::new(),
    };
    let in_testdata = path.split('/').any(|part| part == "testdata");
    let Some(format) = Format::of_path(Path::new(path)).filter(|_| !in_testdata) else {
        return Ok(loaded);
    };
    for doc in documents(format, path, text).map_err(Error::from)? {
        match kind_of(&doc) {
            Some(PackSpec::KIND) => loaded
                .packs
                .push(resource::<PackSpec>(path, &doc).map_err(Error::from)?),
            Some(DecisionSpec::KIND) => {
                let Resource { metadata, spec } =
                    resource::<DecisionSpec>(path, &doc).map_err(Error::from)?;
                loaded.decisions.push(Decision::new(metadata, spec));
            }
            Some(ProjectSpec::KIND) => loaded
                .projects
                .push(resource::<ProjectSpec>(path, &doc).map_err(Error::from)?),
            Some(SourceMapSpec::KIND) => loaded.sources.extend(
                resource::<SourceMapSpec>(path, &doc)
                    .map_err(Error::from)?
                    .spec
                    .sources,
            ),
            // A document without `kind` is no resource: `resource` says so.
            None => {
                resource::<DecisionSpec>(path, &doc).map_err(Error::from)?;
            }
            Some(other) => {
                return Err(Error::layout(path, format!("unsupported kind `{other}`")));
            }
        }
    }
    Ok(loaded)
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
    for entry in fs::read_dir(dir).map_err(Error::io(dir))? {
        let path = entry.map_err(Error::io(dir))?.path();
        if path.is_dir() {
            collect_dir(root, &path, files)?;
            continue;
        }
        let text = fs::read_to_string(&path).map_err(Error::io(&path))?;

        let key = path.strip_prefix(root).unwrap_or(&path);
        files.insert(key.to_string_lossy().replace('\\', "/"), text);
    }
    Ok(())
}
