use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use lighthouse_model::hash;
use lighthouse_resource::{
    Format, Resource, SCHEMA_URL_BASE, Spec, documents, header, resource, to_yaml, write_atomic,
};
use serde::Serialize;

use crate::{
    Decision, Error, Pack, PackSpec, ProjectError, ProjectSpec, Projects, Section,
    decision::SECTION_LABEL,
    project,
    sources::{SourceLine, extract},
    validate,
};

use load::{Files, read_documents};
use merge::{assemble, local_decision, merge_sections, resolve_examples};

pub(crate) mod load;
mod merge;

const PACK_FILE: &str = "pack.yaml";
const SOURCES_FILE: &str = "sources.yaml";
/// The pack and section the decisions of a project's own layer belong to.
const LOCAL_PACK: &str = "local";
const LOCAL_SECTION: &str = "rules";

/// A set of packs with the sources they came from. A catalog is a layer:
/// `overlay` stacks the project-local layer on the bundled one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Catalog {
    pub packs: Vec<Pack>,
    pub(crate) sources: Vec<SourceLine>,
    /// The `Project` documents of the layer; the standard projects of the
    /// packs derive from the decisions and are not listed here.
    projects: Vec<Resource<ProjectSpec>>,
}

impl Catalog {
    /// The catalog compiled into the binary.
    pub fn bundled() -> &'static Self {
        static BUNDLED: OnceLock<Catalog> = OnceLock::new();
        BUNDLED
            .get_or_init(|| Self::from_files(load::embedded()).expect("bundled catalog is valid"))
    }

    /// Reads and validates the catalog under `dir`; see [`Catalog::from_files`].
    pub fn load(dir: &Path) -> Result<Self, Error> {
        Self::from_files(load::read_dir(dir)?)
    }

    /// Builds a validated layer from files keyed by `/`-separated paths
    /// relative to the catalog root. Every YAML, TOML or JSON file outside a
    /// `testdata` directory holds documents of the kinds `Pack`, `Decision`,
    /// `Project` and `SourceMap`; a decision belongs to the pack and section
    /// its labels name, and the pack lists it.
    pub fn from_files(files: Files) -> Result<Self, Error> {
        let mut catalog = Self::default();
        let mut packs = Vec::new();
        let mut decisions: Vec<(String, Decision)> = Vec::new();
        for (path, text) in &files {
            let loaded = read_documents(path, text)?;
            packs.extend(loaded.packs.into_iter().map(|p| (path.clone(), p)));
            decisions.extend(
                loaded
                    .decisions
                    .into_iter()
                    .map(|d| (loaded.dir.clone(), d)),
            );
            catalog.projects.extend(loaded.projects);
            catalog.sources.extend(loaded.sources);
        }
        packs.sort_by(|a, b| a.1.metadata.name.cmp(&b.1.metadata.name));
        catalog.packs = assemble(&files, packs, decisions)?;
        validate::layer(&catalog)?;
        Ok(catalog)
    }

    /// Builds the project-local layer from the files of
    /// `.lighthouse/decisions`, keyed by file name. A `Decision` there has an
    /// id `local/<name>` and a CEL check (or none); the loader adds the
    /// `local` pack and `rules` section labels it leaves out. A `Project` is a
    /// shareable configuration that `extends` can name. Examples must be
    /// inline.
    pub fn from_local(files: Files) -> Result<Self, Error> {
        let mut catalog = Self::default();
        let mut decisions = Vec::new();
        for (name, text) in &files {
            let loaded = read_documents(name, text)?;
            if let Some(pack) = loaded.packs.first() {
                return Err(Error::layout(
                    name,
                    format!("a local layer has no packs, found `{}`", pack.metadata.name),
                ));
            }
            catalog.projects.extend(loaded.projects);
            for decision in loaded.decisions {
                decisions.push(local_decision(name, decision)?);
            }
        }
        if !decisions.is_empty() {
            catalog.packs.push(Pack {
                id: LOCAL_PACK.to_owned(),
                title: "Local decisions".to_owned(),
                intro: "Decisions of this project, from `.lighthouse/decisions`.".to_owned(),
                sections: vec![Section {
                    id: LOCAL_SECTION.to_owned(),
                    title: "Decisions".to_owned(),
                    intro: "Project-local decisions with CEL checks.".to_owned(),
                    decisions: decisions
                        .into_iter()
                        .map(|d| resolve_examples(&files, "", d))
                        .collect::<Result<_, _>>()?,
                }],
            });
        }
        validate::layer(&catalog)?;
        Ok(catalog)
    }

    /// `base` with `local` on top. Local packs, sections and decisions are
    /// added; into an existing pack or section only new sections or decisions
    /// are merged, and the base title and intro win. The projects of both are
    /// kept. Sources are validated per layer, not across layers.
    pub fn overlay(base: &Self, local: &Self) -> Result<Self, Error> {
        let mut merged = base.clone();
        for pack in &local.packs {
            match merged.packs.iter_mut().find(|p| p.id == pack.id) {
                None => merged.packs.push(pack.clone()),
                Some(into) => merge_sections(into, pack),
            }
        }
        merged.sources.extend(local.sources.iter().cloned());
        merged.projects.extend(local.projects.iter().cloned());
        validate::decisions(&merged)?;
        validate::projects(&merged)?;
        Ok(merged)
    }

    /// The projects `extends` can name: the standard ones of every pack, then
    /// the `Project` documents of the layer.
    pub fn projects(&self) -> Result<Projects, ProjectError> {
        Projects::new(
            project::standard(self)
                .into_iter()
                .chain(self.projects.iter().cloned()),
        )
    }

    /// Writes `decision` into `<root>/<pack>/<section>/` and lists it in that
    /// section of the pack file. Both files are written to temporary names
    /// first, so a failure leaves no half-written file; the decision is
    /// renamed before the pack. `source` example files must already exist.
    pub fn write_decision(root: &Path, decision: &Decision) -> Result<(), Error> {
        validate::decision(decision)?;
        let id = decision.id();
        let (pack, name) = id
            .split_once('/')
            .ok_or_else(|| Error::invalid(id, "an id is `<pack>/<name>`"))?;
        let section = decision.section();
        if section.is_empty() {
            return Err(Error::invalid(
                id,
                format!("needs a `{SECTION_LABEL}` label"),
            ));
        }
        let dir = root.join(pack).join(section);
        let pack_path = root.join(pack).join(PACK_FILE);
        let text = read(&pack_path)?;
        let label = pack_path.display().to_string();
        let mut docs = documents(Format::Yaml, &label, &text).map_err(Error::from)?;
        let doc = docs
            .pop()
            .ok_or_else(|| Error::layout(&label, "is empty"))?;
        let mut file = resource::<PackSpec>(&label, &doc).map_err(Error::from)?;
        let listed = file
            .spec
            .sections
            .iter_mut()
            .find(|s| s.name == section)
            .ok_or_else(|| Error::layout(&label, format!("has no section `{section}`")))?;
        if !listed.decisions.iter().any(|d| d == name) {
            listed.decisions.push(name.to_owned());
        }
        let decision_path = dir.join(format!("{name}.yaml"));
        let staged = [
            (decision_path.clone(), decision_document(decision)?),
            (pack_path, document(&file)?),
        ];
        let temps: Vec<(PathBuf, PathBuf)> = staged
            .iter()
            .map(|(path, _)| (path.with_extension("yaml.tmp"), path.clone()))
            .collect();
        fs::create_dir_all(&dir).map_err(|source| Error::Io {
            path: dir.display().to_string(),
            source,
        })?;
        for ((tmp, _), (_, text)) in temps.iter().zip(&staged) {
            write(tmp, text)?;
        }
        for (tmp, path) in &temps {
            fs::rename(tmp, path).map_err(|source| Error::Io {
                path: path.display().to_string(),
                source,
            })?;
        }
        Ok(())
    }

    /// The text of a decision file: a `Decision` document in YAML that starts
    /// with the schema comment editors use.
    pub fn decision_text(decision: &Decision) -> Result<String, Error> {
        decision_document(decision)
    }

    /// Whether `name` can be the file name of a local layer file: it starts
    /// with a lowercase letter or digit and goes on with lowercase letters,
    /// digits, `.`, `_` and `-`, and has no `..`. Slashes, backslashes and
    /// empty names never pass, so a name cannot leave the directory.
    pub fn local_name_ok(name: &str) -> bool {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && chars.all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
            })
            && !name.contains("..")
    }

    /// Writes a local layer file `<dir>/<name>.yaml` atomically (see
    /// `write_atomic`). A name that fails `local_name_ok`, and a target that
    /// does not resolve to a file directly inside `dir`, are refused.
    pub fn write_local(dir: &Path, name: &str, text: &str) -> Result<(), Error> {
        if !Self::local_name_ok(name) {
            return Err(Error::invalid(
                name,
                "a local decision file name is lowercase letters, digits, `.`, `_` and `-`, without `..`",
            ));
        }
        let io = |path: &Path, source| Error::Io {
            path: path.display().to_string(),
            source,
        };
        fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
        let path = dir.join(format!("{name}.yaml"));
        let inside = path
            .parent()
            .map(Path::canonicalize)
            .transpose()
            .map_err(|e| io(dir, e))?
            == Some(dir.canonicalize().map_err(|e| io(dir, e))?);
        if !inside {
            return Err(Error::invalid(
                name,
                "resolves outside the decisions directory",
            ));
        }
        Ok(write_atomic(&path, text)?)
    }

    /// Fails unless `sources.yaml` lists exactly the normative lines of the
    /// given documents, keyed by document name (`coding-patterns` for refs
    /// like `coding-patterns#...`).
    pub fn verify_sources(&self, docs: &BTreeMap<String, String>) -> Result<(), Error> {
        let mut want: BTreeMap<String, String> = BTreeMap::new();
        for (doc, markdown) in docs {
            for bullet in extract(doc, markdown) {
                want.insert(bullet.reference, bullet.text);
            }
        }
        let have: BTreeMap<_, _> = self
            .sources
            .iter()
            .map(|s| (s.reference.clone(), s.text.clone()))
            .collect();
        let missing: Vec<_> = want
            .iter()
            .filter(|(r, _)| !have.contains_key(*r))
            .map(|(r, t)| format!("{r}: {t}"))
            .collect();
        let stale: Vec<_> = have
            .keys()
            .filter(|r| !want.contains_key(*r))
            .cloned()
            .collect();
        if missing.is_empty() && stale.is_empty() {
            return Ok(());
        }
        Err(Error::invalid(
            SOURCES_FILE,
            format!(
                "out of date with its documents; missing: [{}]; stale: [{}]",
                missing.join("; "),
                stale.join(", ")
            ),
        ))
    }

    /// Every decision in pack, section and file order.
    pub fn decisions(&self) -> impl Iterator<Item = &Decision> {
        self.packs
            .iter()
            .flat_map(|p| &p.sections)
            .flat_map(|s| &s.decisions)
    }

    /// The decision with this `<pack>/<name>` id.
    pub fn decision(&self, id: &str) -> Option<&Decision> {
        self.decisions().find(|d| d.id() == id)
    }

    /// Identifies this set of decisions: the hash of every decision's id and
    /// version, in catalog order.
    pub fn version(&self) -> String {
        let listing: String = self
            .decisions()
            .map(|d| format!("{}:{}\n", d.id(), d.version()))
            .collect();
        hash::short(&listing, 8)
    }
}

fn decision_document(decision: &Decision) -> Result<String, Error> {
    let resource = decision.clone().into_resource();
    document(&resource)
}

fn document<S: Spec + Serialize>(resource: &Resource<S>) -> Result<String, Error> {
    Ok(format!(
        "{}{}",
        header(S::KIND, SCHEMA_URL_BASE),
        to_yaml(resource)
    ))
}

fn read(path: &Path) -> Result<String, Error> {
    fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })
}

fn write(path: &Path, text: &str) -> Result<(), Error> {
    fs::write(path, text).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })
}
