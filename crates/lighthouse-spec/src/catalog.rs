use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    Content, Error, Example, Pack, Pattern, Section,
    load::{self, Files},
    sources::{Source, extract},
    validate,
};

const PACK_FILE: &str = "pack.yaml";
const SECTION_FILE: &str = "section.yaml";
const SOURCES_FILE: &str = "sources.yaml";
const LEGACY_FILE: &str = "legacy.yaml";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Catalog {
    pub packs: Vec<Pack>,
    pub(crate) sources: Vec<Source>,
    pub(crate) legacy: BTreeMap<String, Vec<String>>,
    overrides: Vec<Override>,
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

    pub fn load(dir: &Path) -> Result<Self, Error> {
        Self::from_files(load::read_dir(dir)?)
    }

    /// Builds a validated layer from files keyed by `/`-separated paths
    /// relative to the catalog root. `sources.yaml` and `legacy.yaml` are
    /// optional. Pattern files holding `extends` become overrides, applied by
    /// `overlay`.
    pub fn from_files(files: Files) -> Result<Self, Error> {
        let mut catalog = Self::default();
        for path in files.keys() {
            if let Some(id) = path.strip_suffix(&format!("/{PACK_FILE}"))
                && !id.contains('/')
            {
                load_pack(&files, id, &mut catalog)?;
            }
        }
        if let Some(text) = files.get(SOURCES_FILE) {
            catalog.sources = parse(SOURCES_FILE, text)?;
        }
        if let Some(text) = files.get(LEGACY_FILE) {
            catalog.legacy = parse(LEGACY_FILE, text)?;
        }
        validate::layer(&catalog)?;
        Ok(catalog)
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
        for (old, targets) in &local.legacy {
            if merged.legacy.insert(old.clone(), targets.clone()).is_some() {
                return Err(Error::invalid(old, "legacy id mapped in two layers"));
            }
        }
        for o in &local.overrides {
            let pattern = merged
                .packs
                .iter_mut()
                .flat_map(|p| &mut p.sections)
                .flat_map(|s| &mut s.patterns)
                .find(|p| p.id == o.extends)
                .ok_or_else(|| Error::invalid(&o.extends, "extends an unknown pattern"))?;
            apply(pattern, o);
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

    pub fn patterns(&self) -> impl Iterator<Item = &Pattern> {
        self.packs
            .iter()
            .flat_map(|p| &p.sections)
            .flat_map(|s| &s.patterns)
    }

    pub fn pattern(&self, id: &str) -> Option<&Pattern> {
        self.patterns().find(|p| p.id == id)
    }

    /// Prototype rule id to the patterns that replace it.
    pub fn legacy(&self) -> &BTreeMap<String, Vec<String>> {
        &self.legacy
    }
}

fn merge_sections(into: &mut Pack, from: &Pack) {
    for section in &from.sections {
        match into.sections.iter_mut().find(|s| s.id == section.id) {
            None => into.sections.push(section.clone()),
            Some(existing) => existing.patterns.extend(section.patterns.iter().cloned()),
        }
    }
}

fn apply(pattern: &mut Pattern, o: &Override) {
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
            continue;
        };
        if let Some(default) = &change.default {
            spec.default = default.clone();
        }
        for (language, value) in &change.per_language {
            spec.per_language.insert(language.clone(), value.clone());
        }
    }
    pattern.examples.extend(o.examples.iter().cloned());
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

fn load_pack(files: &Files, id: &str, catalog: &mut Catalog) -> Result<(), Error> {
    let path = format!("{id}/{PACK_FILE}");
    let file: PackFile = parse(&path, &files[&path])?;
    expect(&path, id, &file.id)?;
    check_order(&path, &file.sections, &sections_in(files, id))?;
    let mut sections = Vec::new();
    for name in &file.sections {
        sections.push(load_section(files, id, name, catalog)?);
    }
    catalog.packs.push(Pack {
        id: file.id,
        title: file.title,
        intro: file.intro,
        sections,
    });
    Ok(())
}

fn load_section(
    files: &Files,
    pack: &str,
    name: &str,
    catalog: &mut Catalog,
) -> Result<Section, Error> {
    let path = format!("{pack}/{name}/{SECTION_FILE}");
    let file: SectionFile = parse(&path, &files[&path])?;
    expect(&path, name, &file.id)?;
    check_order(&path, &file.patterns, &patterns_in(files, pack, name))?;
    let mut patterns = Vec::new();
    for pattern in &file.patterns {
        match load_pattern(files, pack, name, pattern)? {
            Loaded::Pattern(p) => patterns.push(*p),
            Loaded::Override(o) => catalog.overrides.push(o),
        }
    }
    Ok(Section {
        id: file.id,
        title: file.title,
        intro: file.intro,
        patterns,
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
            let mut resolved = Vec::new();
            for file in std::mem::take(&mut example.files) {
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
                    .ok_or_else(|| {
                        Error::invalid(id, format!("example source `{source}` does not exist"))
                    })?
                    .clone();
                resolved.push(file.with_loaded(text));
            }
            example.files = resolved;
            Ok(example)
        })
        .collect()
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
/// `examples/` are never patterns.
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
