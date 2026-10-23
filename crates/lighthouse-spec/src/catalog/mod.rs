use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::Path,
};

use lighthouse_protocol::hash;
use lighthouse_resource::{Resource, SCHEMA_URL_BASE, Spec, header, to_yaml, write_atomic};

use crate::{
    Cel, Decision, DecisionSpec, Error, Pack, ProjectSpec, Projects, Section, project,
    sources::{SourceLine, extract},
    validate,
};

use load::{Files, read_documents};
use merge::{assemble, check_local, merge_sections, resolve_examples};

pub(crate) mod load;
mod merge;
mod variables;

const SOURCES_FILE: &str = "sources.yaml";
/// The pack and section the decisions of a project's own layer belong to.
const LOCAL_PACK: &str = "local";
const LOCAL_SECTION: &str = "rules";

/// A set of packs with the sources they came from. A catalog is a layer:
/// `overlay` stacks the project-local layer on the bundled one.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    packs: Vec<Pack>,
    pub(crate) sources: Vec<SourceLine>,
    /// The `Project` documents of the layer; the standard projects of the
    /// packs derive from the decisions and are not listed here.
    projects: Vec<Resource<ProjectSpec>>,
    /// Where each decision lies, by id: its pack, section and place in it.
    index: HashMap<String, [usize; 3]>,
    /// Identifies this set of decisions; see [`Catalog::version`].
    version: String,
    /// The projects `extends` can name: the standard ones of every pack, then
    /// the `Project` documents of the layer.
    shareable: Projects,
}

impl Catalog {
    /// Reads and validates the catalog under `dir`; see [`Catalog::from_files`].
    pub fn load(dir: &Path, cel: &Cel) -> Result<Self, Error> {
        Self::from_files(load::read_dir(dir)?, cel)
    }

    /// Builds a validated layer from files keyed by `/`-separated paths
    /// relative to the catalog root. Every YAML, TOML or JSON file outside a
    /// `testdata` directory holds documents of the kinds `Pack`, `Decision`,
    /// `Project` and `SourceMap`; a decision belongs to the pack its id
    /// names, and the pack lists it in a section.
    pub fn from_files(files: Files, cel: &Cel) -> Result<Self, Error> {
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
        catalog.packs = assemble(cel, &files, packs, decisions)?;
        catalog.sealed(|layer| validate::layer(layer, cel))
    }

    /// Builds the project-local layer from the files of
    /// `.lighthouse/decisions`, keyed by file name. A `Decision` there has an
    /// id `local/<name>` and a CEL check (or none); the loader lists it in the
    /// `local` pack. A `Project` is a shareable configuration that `extends`
    /// can name. Examples must be inline.
    pub fn from_local(files: Files, cel: &Cel) -> Result<Self, Error> {
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
                check_local(name, &decision)?;
                decisions.push(decision);
            }
        }
        if !decisions.is_empty() {
            catalog.packs.push(Pack {
                id: LOCAL_PACK.to_owned(),
                title: "Local decisions".to_owned(),
                intro: "Decisions of this project, from `.lighthouse/decisions`.".to_owned(),
                variables: Vec::new(),
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
        catalog.sealed(|layer| validate::layer(layer, cel))
    }

    /// `base` with `local` on top. Local packs, sections and decisions are
    /// added; into an existing pack or section only new sections or decisions
    /// are merged, and the base title and intro win. The projects of both are
    /// kept. Each layer was validated as it was built, so only what crosses
    /// them is: unique ids and uids, supersession, and the projects `extends`
    /// names. Sources are validated per layer, not across layers.
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
        let merged = merged.sealed(|merged| validate::overlay(merged, local))?;
        validate::projects(&merged)?;
        Ok(merged)
    }

    /// Indexes the decisions, has `validate` judge the layer, and settles what
    /// derives from the decisions, once: the version and the shareable projects.
    fn sealed(mut self, validate: impl FnOnce(&Self) -> Result<(), Error>) -> Result<Self, Error> {
        self.index = self
            .packs
            .iter()
            .enumerate()
            .flat_map(|(p, pack)| {
                pack.sections
                    .iter()
                    .enumerate()
                    .flat_map(move |(s, section)| {
                        section
                            .decisions
                            .iter()
                            .enumerate()
                            .map(move |(d, decision)| (decision.id().to_owned(), [p, s, d]))
                    })
            })
            .collect();
        validate(&self)?;
        let listing: String = self
            .decisions()
            .map(|d| format!("{}:{}\n", d.id(), d.version()))
            .collect();
        self.version = hash::short(&listing, 8);
        self.shareable = Projects::new(
            project::standard(&self)
                .into_iter()
                .chain(self.projects.iter().cloned()),
        )?;
        Ok(self)
    }

    /// The packs, in id order.
    pub fn packs(&self) -> &[Pack] {
        &self.packs
    }

    /// The projects `extends` can name: the standard ones of every pack, then
    /// the `Project` documents of the layer.
    pub fn projects(&self) -> &Projects {
        &self.shareable
    }

    /// The text of a decision file: a `Decision` document in YAML that starts
    /// with the schema comment editors use.
    pub fn decision_text(decision: &Decision) -> String {
        let resource = decision.clone().into_resource();
        format!(
            "{}{}",
            header(<DecisionSpec as Spec>::KIND, SCHEMA_URL_BASE),
            to_yaml(&resource)
        )
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
        fs::create_dir_all(dir).map_err(Error::io(dir))?;
        let path = dir.join(format!("{name}.yaml"));
        let inside = path
            .parent()
            .map(Path::canonicalize)
            .transpose()
            .map_err(Error::io(dir))?
            == Some(dir.canonicalize().map_err(Error::io(dir))?);
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
        let have: BTreeMap<&str, &str> = self
            .sources
            .iter()
            .map(|s| (s.reference.as_str(), s.text.as_str()))
            .collect();
        let missing: Vec<_> = want
            .iter()
            .filter(|(r, _)| !have.contains_key(r.as_str()))
            .map(|(r, t)| format!("{r}: {t}"))
            .collect();
        let stale: Vec<_> = have
            .keys()
            .filter(|r| !want.contains_key(**r))
            .copied()
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
        let [pack, section, decision] = *self.index.get(id)?;
        Some(&self.packs[pack].sections[section].decisions[decision])
    }

    /// Identifies this set of decisions: the hash of every decision's id and
    /// version, in catalog order.
    pub fn version(&self) -> &str {
        &self.version
    }
}
