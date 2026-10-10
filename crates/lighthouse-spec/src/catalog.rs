use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use lighthouse_model::hash;
use lighthouse_resource::{
    Format, Resource, SCHEMA_URL_BASE, Spec, documents, kind_of, resource, to_document,
};
use serde::Serialize;

use crate::{
    Content, Decision, DecisionSpec, Error, Example, ExampleFile, Pack, PackSpec, ProjectError,
    ProjectSpec, Projects, Section, SectionSpec,
    decision::{PACK_LABEL, SECTION_LABEL},
    load::{self, Files},
    project,
    sources::{SourceLine, SourceMapSpec, extract},
    validate,
};

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

/// What a catalog file holds, with the directory it lies in.
struct Loaded {
    dir: String,
    packs: Vec<Resource<PackSpec>>,
    decisions: Vec<Decision>,
    projects: Vec<Resource<ProjectSpec>>,
    sources: Vec<SourceLine>,
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
        write_atomic(&path, text)
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

    /// The uid of every decision by each id it answers to: its own and the ids
    /// it had before it was renamed. Decisions without a uid are left out.
    pub fn identities(&self) -> BTreeMap<String, String> {
        self.decisions()
            .filter_map(|d| {
                let uid = d.uid()?;
                Some(
                    std::iter::once(d.id())
                        .chain(d.was_names())
                        .map(|name| (name.to_owned(), uid.to_owned())),
                )
            })
            .flatten()
            .collect()
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

/// Writes `text` to `path` through a uniquely named temporary file in the same
/// directory, flushed to disk before it is renamed over `path`, so a reader
/// sees the old file or the new one and a failure leaves no partial file. A
/// `path` that is a symlink is refused rather than followed or replaced.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), Error> {
    write_atomic_guarded(path, text, &|| Ok(()))
}

/// Like [`write_atomic`], but `guard` is asked once the temporary file is
/// complete, right before it is renamed over `path`; an `Err` abandons the
/// write and leaves `path` as it was.
pub fn write_atomic_guarded(
    path: &Path,
    text: &str,
    guard: &dyn Fn() -> Result<(), String>,
) -> Result<(), Error> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let io = |source| Error::Io {
        path: path.display().to_string(),
        source,
    };
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(Error::invalid(
            &path.display().to_string(),
            "is a symlink; edit its target by hand",
        ));
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        // The replacement keeps the mode of the file it replaces.
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        guard().map_err(std::io::Error::other)?;
        fs::rename(&tmp, path)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(io(e));
    }
    Ok(())
}

/// The documents of one catalog file; files that are not documents (example
/// sources, anything under `testdata`) hold none.
fn read_documents(path: &str, text: &str) -> Result<Loaded, Error> {
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
            Some("DecisionOverride") => {
                return Err(Error::layout(
                    path,
                    "`DecisionOverride` is not a kind any more: set the decision's level and options in `rules` of the project, or write a local decision (run `lighthouse spec migrate`)",
                ));
            }
            Some(SourceMapSpec::KIND) => loaded.sources.extend(
                resource::<SourceMapSpec>(path, &doc)
                    .map_err(Error::from)?
                    .spec
                    .sources,
            ),
            // A document without `kind` is a legacy one: `resource` says so.
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

/// Packs with their decisions, in the order the packs list them. Every
/// decision is listed once, in the pack and section its labels name.
fn assemble(
    files: &Files,
    packs: Vec<(String, Resource<PackSpec>)>,
    decisions: Vec<(String, Decision)>,
) -> Result<Vec<Pack>, Error> {
    let mut waiting: BTreeMap<String, (String, Decision)> = BTreeMap::new();
    for (dir, decision) in decisions {
        let id = decision.id().to_owned();
        if waiting.insert(id.clone(), (dir, decision)).is_some() {
            return Err(Error::invalid(&id, "defined twice"));
        }
    }
    let mut built = Vec::new();
    let mut seen_packs = BTreeSet::new();
    for (path, pack) in packs {
        let id = pack.metadata.name;
        if !seen_packs.insert(id.clone()) {
            return Err(Error::layout(
                &path,
                format!("pack `{id}` is defined twice"),
            ));
        }
        let mut sections = Vec::new();
        let mut names = BTreeSet::new();
        for SectionSpec {
            name,
            title,
            intro,
            decisions: listed,
        } in pack.spec.sections
        {
            if !names.insert(name.clone()) {
                return Err(Error::layout(
                    &path,
                    format!("section `{name}` is listed twice"),
                ));
            }
            let mut members = Vec::new();
            for short in &listed {
                let full = format!("{id}/{short}");
                let (dir, decision) = waiting.remove(&full).ok_or_else(|| {
                    Error::layout(
                        &path,
                        format!("`{short}` is listed in section `{name}` but is not defined, or is listed twice"),
                    )
                })?;
                labels_match(&decision, &id, &name)?;
                members.push(resolve_examples(files, &dir, decision)?);
            }
            sections.push(Section {
                id: name,
                title,
                intro,
                decisions: members,
            });
        }
        built.push(Pack {
            id,
            title: pack.spec.title,
            intro: pack.spec.intro,
            sections,
        });
    }
    if let Some((id, (dir, decision))) = waiting.into_iter().next() {
        let place = if decision.section().is_empty() {
            "no section".to_owned()
        } else {
            format!(
                "section `{}` of pack `{}`",
                decision.section(),
                decision.pack()
            )
        };
        return Err(Error::layout(
            &dir,
            format!("`{id}` names {place} but its pack does not list it"),
        ));
    }
    Ok(built)
}

/// A decision's labels name the pack and section that list it.
fn labels_match(decision: &Decision, pack: &str, section: &str) -> Result<(), Error> {
    let id = decision.id();
    let labels = &decision.metadata().labels;
    for (key, want) in [(PACK_LABEL, pack), (SECTION_LABEL, section)] {
        match labels.get(key) {
            Some(found) if found == want => {}
            Some(found) => {
                return Err(Error::invalid(
                    id,
                    format!(
                        "label `{key}` is `{found}` but the {} lists it",
                        if key == PACK_LABEL { "pack" } else { "section" }
                    ),
                ));
            }
            None => {
                return Err(Error::invalid(
                    id,
                    format!("needs the label `{key}: {want}`"),
                ));
            }
        }
    }
    Ok(())
}

/// A decision of the local layer, with the pack and section labels a project
/// does not have to write.
fn local_decision(file: &str, mut decision: Decision) -> Result<Decision, Error> {
    if !decision.id().starts_with("local/") {
        return Err(Error::layout(
            file,
            format!(
                "id is `{}`, a local decision is `local/<name>`",
                decision.id()
            ),
        ));
    }
    for (key, value) in [(PACK_LABEL, LOCAL_PACK), (SECTION_LABEL, LOCAL_SECTION)] {
        let labels = &mut decision.metadata_mut().labels;
        match labels.get(key) {
            Some(found) if found != value => {
                return Err(Error::layout(
                    file,
                    format!("label `{key}` is `{found}`, a local decision's is `{value}`"),
                ));
            }
            _ => {
                labels.insert(key.to_owned(), value.to_owned());
            }
        }
    }
    if let Some(crate::CheckKind::Builtin(builtin)) = decision.check.as_ref().map(|c| &c.kind)
        && builtin.named().is_some()
    {
        return Err(Error::layout(
            file,
            "a local decision may not name a bundled rule with `builtin: {id}`; use a standard operation, `cel` or `command`",
        ));
    }
    Ok(decision)
}

fn merge_sections(into: &mut Pack, from: &Pack) {
    for section in &from.sections {
        match into.sections.iter_mut().find(|s| s.id == section.id) {
            None => into.sections.push(section.clone()),
            Some(existing) => existing.decisions.extend(section.decisions.iter().cloned()),
        }
    }
}

fn resolve_examples(files: &Files, dir: &str, decision: Decision) -> Result<Decision, Error> {
    let id = decision.id().to_owned();
    let mut resolved = Ok(());
    let decision = decision.map_spec(|mut spec| {
        match resolve_all(files, dir, &id, std::mem::take(&mut spec.examples)) {
            Ok(examples) => spec.examples = examples,
            Err(e) => resolved = Err(e),
        }
        spec
    });
    resolved.map(|()| decision)
}

fn resolve_all(
    files: &Files,
    dir: &str,
    id: &str,
    examples: Vec<Example>,
) -> Result<Vec<Example>, Error> {
    examples
        .into_iter()
        .map(|mut example| {
            example.files = resolve_files(files, dir, id, std::mem::take(&mut example.files))?;
            example.fixed = resolve_files(files, dir, id, std::mem::take(&mut example.fixed))?;
            Ok(example)
        })
        .collect()
}

/// Loads the text of the files that name a `source` in the catalog layer.
fn resolve_files(
    files: &Files,
    dir: &str,
    id: &str,
    list: Vec<ExampleFile>,
) -> Result<Vec<ExampleFile>, Error> {
    let mut resolved = Vec::new();
    for file in list {
        let Content::File(source) = &file.content else {
            resolved.push(file);
            continue;
        };
        if !validate::relative(source) {
            return Err(Error::invalid(
                id,
                format!("example source `{source}` escapes its directory"),
            ));
        }
        let key = if dir.is_empty() {
            source.clone()
        } else {
            format!("{dir}/{source}")
        };
        let text = files
            .get(&key)
            .ok_or_else(|| Error::invalid(id, format!("example source `{source}` does not exist")))?
            .clone();
        resolved.push(file.with_loaded(text));
    }
    Ok(resolved)
}

fn decision_document(decision: &Decision) -> Result<String, Error> {
    let resource = decision.clone().into_resource();
    document(&resource)
}

fn document<S: Spec + Serialize>(resource: &Resource<S>) -> Result<String, Error> {
    Ok(to_document(resource, SCHEMA_URL_BASE))
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
