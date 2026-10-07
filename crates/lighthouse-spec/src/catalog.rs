use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    Content, Error, Example, ExampleFile, Implementation, Pack, Pattern, Section,
    load::{self, Files},
    model::short_hash,
    sources::{Source, extract},
    validate,
};

const PACK_FILE: &str = "pack.yaml";
const SECTION_FILE: &str = "section.yaml";
const SOURCES_FILE: &str = "sources.yaml";

/// A set of packs with the sources and declarative rule files they came from.
/// A catalog is a layer: `overlay` stacks the project-local layer on the bundled one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Catalog {
    pub packs: Vec<Pack>,
    pub(crate) sources: Vec<Source>,
    overrides: Vec<Override>,
    /// Text of the declarative rule files that patterns name, by their path
    /// in the catalog layer.
    rules: BTreeMap<String, String>,
}

/// A local pattern file with `extends`: adjusts a pattern of a lower layer.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Override {
    extends: String,
    severity: Option<lighthouse_model::Severity>,
    exceptions: Option<String>,
    #[serde(default)]
    options: BTreeMap<String, OptionOverride>,
    #[serde(default)]
    tuning: BTreeMap<String, String>,
    #[serde(default)]
    examples: Vec<Example>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct OptionOverride {
    default: Option<Value>,
    #[serde(default)]
    per_language: BTreeMap<String, Value>,
}

enum Loaded {
    Pattern(Box<Pattern>),
    Override(Override),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackFile {
    id: String,
    title: String,
    intro: String,
    sections: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SectionFile {
    id: String,
    title: String,
    intro: String,
    patterns: Vec<String>,
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
    /// relative to the catalog root. `sources.yaml` is optional. Pattern files
    /// holding `extends` become overrides, applied by `overlay`.
    pub fn from_files(files: Files) -> Result<Self, Error> {
        let mut catalog = Self::default();
        for path in files.keys() {
            if let Some(id) = path.strip_suffix(&format!("/{PACK_FILE}"))
                && !id.contains('/')
            {
                catalog.load_pack(&files, id)?;
            }
        }
        if let Some(text) = files.get(SOURCES_FILE) {
            catalog.sources = parse(SOURCES_FILE, text)?;
        }
        validate::layer(&catalog)?;
        catalog.attach_rules(&files)?;
        Ok(catalog)
    }

    /// Builds the project-local layer from the files of `.lighthouse/rules`,
    /// keyed by file name. A file is a pattern (id `local/<name>`, the fields
    /// of any pattern but `implementation`) with its declarative rule under
    /// `rule:`; the pattern is added to the `local` pack. A file with
    /// `extends` adjusts a pattern of a lower layer as `overlay` describes.
    /// Examples must be inline.
    pub fn from_local(files: Files) -> Result<Self, Error> {
        let mut catalog = Self::default();
        let mut patterns = Vec::new();
        for (name, text) in &files {
            let doc: serde_norway::Mapping = parse(name, text)?;
            if doc.contains_key("extends") {
                let mut o: Override = parse(name, text)?;
                o.examples = resolve_all(&files, "", &o.extends, o.examples)?;
                catalog.overrides.push(o);
                continue;
            }
            let (pattern, rule) = local_rule(name, doc)?;
            catalog.rules.insert(name.clone(), rule);
            patterns.push(pattern);
        }
        if !patterns.is_empty() {
            catalog.packs.push(Pack {
                id: "local".to_owned(),
                title: "Local rules".to_owned(),
                intro: "Rules of this project, from `.lighthouse/rules`.".to_owned(),
                sections: vec![Section {
                    id: "rules".to_owned(),
                    title: "Rules".to_owned(),
                    intro: "Project-local declarative rules.".to_owned(),
                    patterns,
                }],
            });
        }
        validate::layer(&catalog)?;
        Ok(catalog)
    }

    /// The declarative rule file `path`, as written in the layer that defines
    /// the pattern naming it.
    pub fn declarative(&self, path: &str) -> Option<&str> {
        self.rules.get(path).map(String::as_str)
    }

    fn attach_rules(&mut self, files: &Files) -> Result<(), Error> {
        let wanted: Vec<(String, String)> = self
            .patterns()
            .filter_map(|p| match &p.implementation {
                Some(Implementation::Declarative(path)) => Some((p.id.clone(), path.clone())),
                _ => None,
            })
            .collect();
        for (id, path) in wanted {
            let text = files.get(&path).ok_or_else(|| {
                Error::invalid(&id, format!("declarative rule `{path}` does not exist"))
            })?;
            self.rules.insert(path, text.clone());
        }
        Ok(())
    }

    /// `base` with `local` on top. Local packs, sections and patterns are
    /// added; into an existing pack or section only new sections or patterns
    /// are merged, and the base title and intro win. A local pattern with
    /// `extends` adjusts the named pattern: `severity` and `exceptions`
    /// replace, `tuning` and `options` replace per key, `examples` are
    /// appended. Sources are validated per layer, not across layers.
    pub fn overlay(base: &Self, local: &Self) -> Result<Self, Error> {
        let mut merged = base.clone();
        for pack in &local.packs {
            match merged.packs.iter_mut().find(|p| p.id == pack.id) {
                None => merged.packs.push(pack.clone()),
                Some(into) => merge_sections(into, pack),
            }
        }
        merged.sources.extend(local.sources.iter().cloned());
        merged.rules.extend(local.rules.clone());
        for o in &local.overrides {
            let pattern = merged
                .packs
                .iter_mut()
                .flat_map(|p| &mut p.sections)
                .flat_map(|s| &mut s.patterns)
                .find(|p| p.id == o.extends)
                .ok_or_else(|| Error::invalid(&o.extends, "extends an unknown pattern"))?;
            apply(pattern, o)?;
        }
        validate::patterns(&merged)?;
        Ok(merged)
    }

    /// Writes `pattern` into `<root>/<pack>/<section>/` and lists it in that
    /// section's order. Both files are written to temporary names first, so a
    /// failure leaves no half-written file; the pattern is renamed before the
    /// section list. `source` example files must already exist.
    pub fn write_pattern(root: &Path, section: &str, pattern: &Pattern) -> Result<(), Error> {
        validate::pattern(pattern)?;
        let (pack, name) = pattern
            .id
            .split_once('/')
            .ok_or_else(|| Error::invalid(&pattern.id, "an id is `<pack>/<name>`"))?;
        let dir = root.join(pack).join(section);
        let section_path = dir.join(SECTION_FILE);
        let text = read(&section_path)?;
        let mut file: SectionFile = parse(&section_path.display().to_string(), &text)?;
        if !file.patterns.iter().any(|p| p == name) {
            file.patterns.push(name.to_owned());
        }
        let pattern_path = dir.join(format!("{name}.yaml"));
        let staged = [
            (pattern_path.clone(), dump(pattern)?),
            (section_path, dump(&file)?),
        ];
        let temps: Vec<(PathBuf, PathBuf)> = staged
            .iter()
            .map(|(path, _)| (path.with_extension("yaml.tmp"), path.clone()))
            .collect();
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

    /// The text of a project-local rule file: `pattern` without an
    /// implementation (the loader adds it) and `rule`, the declarative rule
    /// definition, under `rule:`.
    pub fn local_rule_text(pattern: &Pattern, rule: &Value) -> Result<String, Error> {
        let mut doc = serde_norway::to_value(Pattern {
            implementation: None,
            ..pattern.clone()
        })
        .map_err(|e| Error::invalid(&pattern.id, e.to_string()))?;
        let rule =
            serde_norway::to_value(rule).map_err(|e| Error::invalid(&pattern.id, e.to_string()))?;
        let map = doc
            .as_mapping_mut()
            .ok_or_else(|| Error::invalid(&pattern.id, "a pattern is a mapping"))?;
        map.insert("rule".into(), rule);
        dump(&doc)
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
                "a local rule file name is lowercase letters, digits, `.`, `_` and `-`, without `..`",
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
            return Err(Error::invalid(name, "resolves outside the rules directory"));
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

    /// Every pattern in pack, section and file order.
    pub fn patterns(&self) -> impl Iterator<Item = &Pattern> {
        self.packs
            .iter()
            .flat_map(|p| &p.sections)
            .flat_map(|s| &s.patterns)
    }

    /// The pattern with this `<pack>/<name>` id.
    pub fn pattern(&self, id: &str) -> Option<&Pattern> {
        self.patterns().find(|p| p.id == id)
    }

    /// Identifies this set of patterns: the hash of every pattern's id and
    /// version, in catalog order.
    pub fn version(&self) -> String {
        let listing: String = self
            .patterns()
            .map(|p| format!("{}:{}\n", p.id, p.version()))
            .collect();
        short_hash(&listing)
    }

    fn load_pack(&mut self, files: &Files, id: &str) -> Result<(), Error> {
        let path = format!("{id}/{PACK_FILE}");
        let file: PackFile = parse(&path, &files[&path])?;
        expect(&path, id, &file.id)?;
        check_order(&path, &file.sections, &sections_in(files, id))?;
        let mut sections = Vec::new();
        for name in &file.sections {
            sections.push(self.load_section(files, id, name)?);
        }
        self.packs.push(Pack {
            id: file.id,
            title: file.title,
            intro: file.intro,
            sections,
        });
        Ok(())
    }

    fn load_section(&mut self, files: &Files, pack: &str, name: &str) -> Result<Section, Error> {
        let path = format!("{pack}/{name}/{SECTION_FILE}");
        let file: SectionFile = parse(&path, &files[&path])?;
        expect(&path, name, &file.id)?;
        check_order(&path, &file.patterns, &patterns_in(files, pack, name))?;
        let mut patterns = Vec::new();
        for pattern in &file.patterns {
            match load_pattern(files, pack, name, pattern)? {
                Loaded::Pattern(p) => patterns.push(*p),
                Loaded::Override(o) => self.overrides.push(o),
            }
        }
        Ok(Section {
            id: file.id,
            title: file.title,
            intro: file.intro,
            patterns,
        })
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

/// The pattern of a local rule file and the text of its `rule:` section.
fn local_rule(name: &str, mut doc: serde_norway::Mapping) -> Result<(Pattern, String), Error> {
    let rule = doc
        .remove("rule")
        .ok_or_else(|| Error::layout(name, "a local rule file needs a `rule:` section"))?;
    doc.insert(
        "implementation".into(),
        serde_norway::from_str(&format!("declarative: {name:?}"))
            .map_err(|e| Error::layout(name, e.to_string()))?,
    );
    let pattern: Pattern =
        serde_norway::from_value(doc.into()).map_err(|e| Error::layout(name, e.to_string()))?;
    if !pattern.id.starts_with("local/") {
        return Err(Error::layout(
            name,
            format!("id is `{}`, a local rule is `local/<name>`", pattern.id),
        ));
    }
    Ok((pattern, dump(&rule)?))
}

fn merge_sections(into: &mut Pack, from: &Pack) {
    for section in &from.sections {
        match into.sections.iter_mut().find(|s| s.id == section.id) {
            None => into.sections.push(section.clone()),
            Some(existing) => existing.patterns.extend(section.patterns.iter().cloned()),
        }
    }
}

fn apply(pattern: &mut Pattern, o: &Override) -> Result<(), Error> {
    if o.severity.is_some() {
        pattern.severity_override = o.severity;
    }
    if o.exceptions.is_some() {
        pattern.exceptions.clone_from(&o.exceptions);
    }
    for (language, text) in &o.tuning {
        pattern.tuning.insert(language.clone(), text.clone());
    }
    for (name, change) in &o.options {
        let Some(spec) = pattern.options.get_mut(name) else {
            return Err(Error::invalid(
                &o.extends,
                format!("extends an unknown option `{name}`"),
            ));
        };
        if let Some(default) = &change.default {
            spec.default = default.clone();
        }
        for (language, value) in &change.per_language {
            spec.per_language.insert(language.clone(), value.clone());
        }
    }
    pattern.examples.extend(o.examples.iter().cloned());
    Ok(())
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

fn dump<T: Serialize>(value: &T) -> Result<String, Error> {
    serde_norway::to_string(value).map_err(|e| Error::Parse {
        path: "<serialize>".to_owned(),
        message: e.to_string(),
    })
}

fn parse<T: DeserializeOwned>(path: &str, text: &str) -> Result<T, Error> {
    serde_norway::from_str(text).map_err(|e| Error::Parse {
        path: path.to_owned(),
        message: e.to_string(),
    })
}

fn load_pattern(files: &Files, pack: &str, section: &str, name: &str) -> Result<Loaded, Error> {
    let path = format!("{pack}/{section}/{name}.yaml");
    let dir = format!("{pack}/{section}");
    let text = &files[&path];
    let probe: serde_norway::Value = parse(&path, text)?;
    if probe.get("extends").is_some() {
        let mut o: Override = parse(&path, text)?;
        o.examples = resolve_all(files, &dir, &o.extends, o.examples)?;
        return Ok(Loaded::Override(o));
    }
    let mut pattern: Pattern = parse(&path, text)?;
    expect(&path, &format!("{pack}/{name}"), &pattern.id)?;
    pattern.examples = resolve_all(
        files,
        &dir,
        &pattern.id,
        std::mem::take(&mut pattern.examples),
    )?;
    Ok(Loaded::Pattern(Box::new(pattern)))
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
                format!("example source `{source}` escapes its section"),
            ));
        }
        let text = files
            .get(&format!("{dir}/{source}"))
            .ok_or_else(|| Error::invalid(id, format!("example source `{source}` does not exist")))?
            .clone();
        resolved.push(file.with_loaded(text));
    }
    Ok(resolved)
}

fn expect(path: &str, expected: &str, found: &str) -> Result<(), Error> {
    if expected == found {
        return Ok(());
    }
    Err(Error::layout(
        path,
        format!("id is `{found}`, expected `{expected}`"),
    ))
}

fn sections_in<'a>(files: &'a Files, pack: &str) -> Vec<&'a str> {
    let prefix = format!("{pack}/");
    files
        .keys()
        .filter_map(|p| p.strip_prefix(&prefix))
        .filter_map(|rest| rest.strip_suffix(&format!("/{SECTION_FILE}")))
        .filter(|name| !name.contains('/'))
        .collect()
}

/// Direct `.yaml` children of the section directory; subdirectories such as
/// `testdata/` are never patterns.
fn patterns_in<'a>(files: &'a Files, pack: &str, section: &str) -> Vec<&'a str> {
    let prefix = format!("{pack}/{section}/");
    files
        .keys()
        .filter_map(|p| p.strip_prefix(&prefix))
        .filter_map(|rest| rest.strip_suffix(".yaml"))
        .filter(|name| !name.contains('/') && *name != "section")
        .collect()
}

fn check_order(path: &str, listed: &[String], present: &[&str]) -> Result<(), Error> {
    let mut seen: Vec<&str> = Vec::new();
    for name in listed {
        let name = name.as_str();
        if seen.contains(&name) {
            return Err(Error::layout(path, format!("`{name}` is listed twice")));
        }
        if !present.contains(&name) {
            return Err(Error::layout(
                path,
                format!("`{name}` is listed but has no file"),
            ));
        }
        seen.push(name);
    }
    match present.iter().find(|n| !seen.contains(n)) {
        Some(name) => Err(Error::layout(
            path,
            format!("`{name}` exists but is not listed"),
        )),
        None => Ok(()),
    }
}
