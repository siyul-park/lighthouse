//! The `initialize` and `index` methods: finds the packages of the project's
//! files, builds the module tree of every crate target, and reports one
//! fragment per file of the units asked for. A unit is a crate: the files below
//! a `Cargo.toml`, down to the next.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Component, Path, PathBuf},
};

use lighthouse_protocol::{
    Conventions, ErrorObject, FileInfo, Fragment, Incomplete, IndexParams, IndexResult,
    LanguageManifest, Source, TestNaming, UnitRef, UnitResult, Units, UnitsBy, language,
};
use serde::Deserialize;

use crate::{
    api, index,
    names::Index,
    project::cargo::{self, Manifests, Package, Target, TargetKind},
    project::{Root, Tree},
    regions,
};

const LANGUAGE: &str = "rust";
const MANIFEST: &str = "Cargo.toml";
/// The import that stands for every unit of the language.
const EVERY: &str = "*";

pub struct Provider {
    id: String,
    version: String,
}

impl Provider {
    pub fn new(id: &str, version: &str) -> Self {
        Self {
            id: id.to_owned(),
            version: version.to_owned(),
        }
    }
}

impl language::Provider for Provider {
    fn name(&self) -> &str {
        &self.id
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn manifest(&self) -> Vec<LanguageManifest> {
        vec![LanguageManifest {
            id: LANGUAGE.to_owned(),
            aliases: vec!["rust".to_owned(), "rs".to_owned()],
            globs: vec!["**/*.rs".to_owned()],
            ignores: Vec::new(),
            priority: 0,
            fallback: false,
            conventions: Conventions {
                test_globs: vec![
                    "**/tests/**/*.rs".to_owned(),
                    "**/tests.rs".to_owned(),
                    "**/benches/**/*.rs".to_owned(),
                ],
                constructor_prefixes: vec!["new".to_owned()],
                // Tests are small snake_case functions named after what they
                // test, and the integration tests of a crate test all of it.
                test_naming: TestNaming {
                    prefix: String::new(),
                    snake_case: true,
                    variants: false,
                    ancestors: true,
                },
            },
            capabilities: ["extent", "reference-sites", "overlays", "source-ast"]
                .map(str::to_owned)
                .to_vec(),
            ordered_declarations: false,
            units: Units {
                by: UnitsBy::Manifest,
                manifest: Some(MANIFEST.to_owned()),
                // What decides the packages, targets and editions of every file.
                settings: [
                    MANIFEST,
                    "Cargo.lock",
                    "rust-toolchain",
                    "rust-toolchain.toml",
                ]
                .map(str::to_owned)
                .to_vec(),
                env: Vec::new(),
            },
        }]
    }

    fn index(&mut self, params: IndexParams) -> Result<IndexResult, ErrorObject> {
        let options = match params.context.options.get(LANGUAGE) {
            Some(raw) => match Options::deserialize(raw) {
                Ok(options) => options,
                Err(error) => {
                    return Ok(IndexResult {
                        incomplete: vec![Incomplete {
                            path: None,
                            reason: format!("[languages.{LANGUAGE}]: {error}"),
                        }],
                        ..IndexResult::default()
                    });
                }
            },
            None => Options::default(),
        };
        let root = normalize(Path::new(&params.project.root));
        let all = &params.files;
        let has_files = params.units.iter().any(|unit| !unit.files.is_empty());
        let packages = packages(&root, if has_files { all } else { &[] });
        let reached = reached(&packages, &params.units);
        let tree = module_tree(
            &root,
            &packages.list,
            &reached,
            &options,
            overlays(&params, &root),
        );
        let idx = Index::build(&tree, &packages.list);
        let analysis = Analysis {
            root: &root,
            found: &packages,
            tree: &tree,
            idx: &idx,
        };
        let units = params
            .units
            .iter()
            .map(|unit| {
                let (found, api) = match unit.regions.is_empty() {
                    true => analysis.unit(unit),
                    false => (regions_of(&params, unit, &options), None),
                };
                result_of(unit, found, api)
            })
            .collect();
        Ok(IndexResult {
            units,
            ..IndexResult::default()
        })
    }
}

/// `[languages.rust]`; unknown keys are rejected.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Options {
    /// What the `pub` items of a library with `publish = false` are.
    #[serde(default)]
    pub(crate) unpublished: Unpublished,
}

/// Whether the public items of an unpublished library are a contract.
#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Unpublished {
    /// `public` when another package of the project depends on the library,
    /// `internal` when none does.
    #[default]
    Auto,
    Public,
    Internal,
}

/// Who governs a file.
pub(crate) enum Governor {
    Manifest(PathBuf),
    Failed(String),
    Nobody,
}

/// What macros kept from the analysis, over all files of a run.
#[derive(Default)]
struct MacroSummary {
    defining: usize,
    unread: u32,
    example: Option<String>,
}

impl MacroSummary {
    fn add(&mut self, file: &str, stats: &index::MacroStats) {
        self.defining += usize::from(stats.defines);
        self.unread += stats.unread;
        if (stats.defines || stats.unread > 0) && self.example.is_none() {
            self.example = Some(file.to_owned());
        }
    }

    /// One notice: macros are not expanded, and here is how much that hides.
    fn notice(&self) -> Option<String> {
        let example = self.example.as_ref()?;
        Some(format!(
            "macros are not expanded: {} file(s) define `macro_rules!` macros and {} macro invocation(s) were not read (item position, or arguments that are not expressions), e.g. {example}",
            self.defining, self.unread
        ))
    }
}

/// The module tree and the name index of every file of the project, which a
/// unit is analyzed in.
struct Analysis<'a> {
    root: &'a Path,
    found: &'a Packages,
    tree: &'a Tree,
    idx: &'a Index<'a>,
}

impl Analysis<'_> {
    /// The fragments, trees and gaps of the files of a unit, and its `api`.
    fn unit(&self, unit: &UnitRef) -> (Found, Option<String>) {
        let files: Vec<String> = unit.files.iter().map(|file| file.path.clone()).collect();
        let (root, tree) = (self.root, self.tree);
        let mut out = Found::default();
        let mut incomplete: BTreeSet<(Option<String>, String)> = BTreeSet::new();
        let mut editions = Manifests::default();
        let mut orphans = Vec::new();
        let mut macros = MacroSummary::default();
        for rel in &files {
            let mut fragment = empty_fragment(rel);
            match (
                self.found.of.get(rel),
                tree.file_of(&normalize(&root.join(rel))),
            ) {
                (_, Some(file)) => {
                    let (extracted, stats) = index::fragment(self.idx, file);
                    macros.add(rel, &stats);
                    fragment = extracted;
                }
                (Some(Err(reason)), None) => {
                    incomplete.insert((Some(rel.clone()), reason.clone()));
                }
                (_, None) => orphans.push(rel.clone()),
            }
            state_edition(&mut fragment, root, &mut editions, &mut incomplete);
            out.fragments.push(fragment);
        }
        for (rel, reason) in tree.problems.iter().filter(|(rel, _)| files.contains(rel)) {
            incomplete.insert((Some(rel.clone()), reason.clone()));
        }
        unreached(&orphans, tree, &mut incomplete, &mut out.notices);
        out.notices.extend(tree.included(&files));
        out.notices.extend(macros.notice());
        out.notices.extend(shared(tree, &files));
        let wanted: BTreeSet<&str> = unit
            .files
            .iter()
            .filter(|file| file.source_ast)
            .map(|file| file.path.as_str())
            .collect();
        let mut seen_trees = BTreeSet::new();
        for file in &tree.files {
            if !wanted.contains(file.rel.as_str()) || !seen_trees.insert(file.rel.clone()) {
                continue;
            }
            match crate::symbols::from_file(file) {
                Ok(source_ast) => out.sources.push(source_ast),
                Err(reason) => {
                    incomplete.insert((
                        Some(file.rel.clone()),
                        format!("cannot expose source: {reason}"),
                    ));
                }
            }
        }
        out.incomplete = incomplete
            .into_iter()
            .map(|(path, reason)| Incomplete { path, reason })
            .collect();
        out.reads = self.reads(&files);
        out.fragments.sort_by(|a, b| a.file.path.cmp(&b.file.path));
        let reached: Vec<&crate::project::SourceFile> = tree
            .files
            .iter()
            .filter(|file| files.contains(&file.rel))
            .collect();
        (out, Some(api::of(&reached)))
    }
}

impl Analysis<'_> {
    /// The manifests above the packages of the files that their answers read.
    fn reads(&self, files: &[String]) -> Vec<String> {
        let reads: BTreeSet<String> = files
            .iter()
            .filter_map(|rel| self.found.of.get(rel)?.as_ref().ok())
            .flat_map(|&at| &self.found.list[at].reads)
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        reads.into_iter().collect()
    }
}

struct Packages {
    list: Vec<Package>,
    /// Requested file to its package, or why it has none.
    of: BTreeMap<String, Result<usize, String>>,
}

/// What the analysis of a unit found.
#[derive(Default)]
pub(crate) struct Found {
    pub fragments: Vec<Fragment>,
    pub sources: Vec<Source>,
    pub notices: Vec<String>,
    pub incomplete: Vec<Incomplete>,
    /// The files outside the unit the answer depends on.
    pub reads: Vec<String>,
}

/// Collapses `.` and `..` so one file has one path however it was reached.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// The manifest nearest above `rel` within `root`, loading each directory once.
pub(crate) fn governor(
    root: &Path,
    rel: &str,
    by_dir: &mut BTreeMap<PathBuf, Result<Option<Package>, String>>,
) -> Governor {
    let mut dir = root.join(rel).parent().map(Path::to_owned);
    while let Some(current) = dir.filter(|d| d.starts_with(root)) {
        let loaded = by_dir
            .entry(current.clone())
            .or_insert_with(|| cargo::load(&current));
        match loaded {
            Ok(Some(_)) => return Governor::Manifest(current),
            Err(reason) => return Governor::Failed(reason.clone()),
            Ok(None) => dir = current.parent().map(Path::to_owned),
        }
    }
    Governor::Nobody
}

/// The texts that stand in for files, by absolute path.
pub(crate) fn overlays(params: &IndexParams, root: &Path) -> HashMap<PathBuf, String> {
    params
        .context
        .overlays
        .iter()
        .flatten()
        .map(|o| (normalize(&root.join(&o.path)), o.text.clone()))
        .collect()
}

pub(crate) fn roots(packages: &[Package], mode: Unpublished) -> Vec<Root> {
    let used = depended_on(packages);
    let shipped = shipped(packages);
    let mut roots = Vec::new();
    for (at, package) in packages.iter().enumerate() {
        let mut targets: Vec<&Target> = package.targets.iter().collect();
        targets.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
        let has_lib = targets.iter().any(|t| t.kind == TargetKind::Lib);
        let base = cargo::crate_name(&package.name);
        // A library only tests link in, and that ships nowhere, is test code.
        let support = !package.publishable
            && used.contains(package.name.as_str())
            && !shipped.contains(package.name.as_str())
            && !targets.iter().any(|t| t.kind == TargetKind::Bin);
        let exported = package.publishable
            || match mode {
                Unpublished::Public => true,
                Unpublished::Internal => false,
                Unpublished::Auto => used.contains(package.name.as_str()),
            };
        for target in targets {
            let label = match target.kind {
                TargetKind::Lib => base.clone(),
                kind => format!("{base}[{}:{}]", kind.label(), target.name),
            };
            roots.push(Root {
                test_of: ((target.kind == TargetKind::Test && has_lib)
                    || (target.kind == TargetKind::Lib && support))
                    .then(|| base.clone()),
                label,
                package: at,
                kind: target.kind,
                exported,
                file: normalize(&target.root),
            });
        }
    }
    roots
}

/// The fences of the host files of a unit of regions.
fn regions_of(params: &IndexParams, unit: &UnitRef, options: &Options) -> Found {
    let hosts: Vec<_> = unit.regions.iter().collect();
    regions::index(params, &hosts, options)
}

/// The wire result of a unit. A unit of files has the `api` of its skeletons; a
/// unit of regions is imported by none, and what it holds is all its `api`
/// says.
fn result_of(unit: &UnitRef, found: Found, api: Option<String>) -> UnitResult {
    let hashes: Vec<&str> = unit
        .files
        .iter()
        .map(|file| file.hash.as_str())
        .chain(unit.regions.iter().map(|file| file.hash.as_str()))
        .collect();
    let body = api::digest(&hashes);
    UnitResult {
        id: unit.id.clone(),
        fragments: found.fragments,
        sources: found.sources,
        api: api.unwrap_or_else(|| body.clone()),
        body,
        // A fragment reads what the other crates declare, as a module tree of
        // all of them resolves a path: every unit is imported.
        imports: match unit.regions.is_empty() {
            true => vec![EVERY.to_owned()],
            false => Vec::new(),
        },
        reads: found.reads,
        notices: found.notices,
        incomplete: found.incomplete,
    }
}

/// The fact `edition` of the file `fragment` describes: the edition of the
/// crate that holds it, which a formatter needs and cannot read for itself. A
/// file whose edition cannot be settled is a gap, not a guess.
fn state_edition(
    fragment: &mut Fragment,
    root: &Path,
    editions: &mut Manifests,
    incomplete: &mut BTreeSet<(Option<String>, String)>,
) {
    let rel = fragment.file.path.clone();
    match editions.edition(root, &root.join(&rel)) {
        Ok(edition) => {
            fragment.file.facts.insert("edition".to_owned(), edition);
        }
        Err(reason) => {
            incomplete.insert((Some(rel), reason));
        }
    }
}

fn empty_fragment(path: &str) -> Fragment {
    Fragment {
        file: FileInfo {
            path: path.to_owned(),
            facts: Default::default(),
        },
        ..Fragment::default()
    }
}

/// The nearest manifest with a `[package]` at or above each requested file;
/// files no manifest governs share one synthetic package at the root.
fn packages(root: &Path, files: &[String]) -> Packages {
    let mut by_dir: BTreeMap<PathBuf, Result<Option<Package>, String>> = BTreeMap::new();
    let mut owner: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut failed: BTreeMap<String, String> = BTreeMap::new();
    let mut ungoverned = false;
    for rel in files {
        match governor(root, rel, &mut by_dir) {
            Governor::Manifest(dir) => {
                owner.insert(rel.clone(), dir);
            }
            Governor::Failed(reason) => {
                failed.insert(rel.clone(), reason);
            }
            Governor::Nobody => ungoverned = true,
        }
    }
    let mut list = Vec::new();
    let mut at: BTreeMap<PathBuf, usize> = BTreeMap::new();
    for (dir, loaded) in by_dir {
        if let Ok(Some(package)) = loaded
            && owner.values().any(|d| *d == dir)
        {
            at.insert(dir, list.len());
            list.push(package);
        }
    }
    let synthetic = ungoverned.then(|| {
        list.push(cargo::synthetic(root));
        list.len() - 1
    });
    let of = files
        .iter()
        .map(|rel| {
            let found = match (owner.get(rel), failed.get(rel), synthetic) {
                (Some(dir), _, _) => Ok(at[dir]),
                (None, Some(reason), _) => Err(reason.clone()),
                (None, None, Some(package)) => Ok(package),
                (None, None, None) => Err("no package governs the file".to_owned()),
            };
            (rel.clone(), found)
        })
        .collect();
    Packages { list, of }
}

/// The module tree of the crate targets of the packages in `reached`.
fn module_tree(
    root: &Path,
    packages: &[Package],
    reached: &BTreeSet<usize>,
    options: &Options,
    overlays: HashMap<PathBuf, String>,
) -> Tree {
    let mut tree = Tree::new(root);
    tree.overlays = overlays;
    for r in roots(packages, options.unpublished) {
        if reached.contains(&r.package) {
            tree.add_crate(r);
        }
    }
    tree
}

/// The packages a fragment of the units can read: their own, and the packages
/// they depend on, through the dependencies of those in turn. A path resolves
/// through the crates it names and no others, so the rest of the project is
/// not parsed.
fn reached(found: &Packages, units: &[UnitRef]) -> BTreeSet<usize> {
    let by_name: HashMap<&str, usize> = found
        .list
        .iter()
        .enumerate()
        .map(|(at, package)| (package.name.as_str(), at))
        .collect();
    let mut reached = BTreeSet::new();
    let mut pending: Vec<usize> = units
        .iter()
        .flat_map(|unit| &unit.files)
        .filter_map(|file| found.of.get(&file.path)?.as_ref().ok().copied())
        .collect();
    while let Some(at) = pending.pop() {
        if !reached.insert(at) {
            continue;
        }
        pending.extend(
            found.list[at]
                .deps
                .values()
                .filter_map(|name| by_name.get(name.as_str()).copied()),
        );
    }
    reached
}

/// Packages another package of the project links into what it ships.
fn shipped(packages: &[Package]) -> BTreeSet<&str> {
    packages
        .iter()
        .flat_map(|p| p.runtime.iter())
        .map(String::as_str)
        .collect()
}

/// Packages another package of the project depends on.
fn depended_on(packages: &[Package]) -> BTreeSet<&str> {
    packages
        .iter()
        .flat_map(|p| p.deps.values())
        .map(String::as_str)
        .collect()
}

/// Files no crate target reaches: a notice when nothing else is wrong, a gap
/// when a file of the project could not be read or parsed, since the module
/// tree may then be cut short.
fn unreached(
    orphans: &[String],
    tree: &Tree,
    incomplete: &mut BTreeSet<(Option<String>, String)>,
    notices: &mut Vec<String>,
) {
    if orphans.is_empty() {
        return;
    }
    if tree.problems.is_empty() {
        notices.push(format!(
            "{} file(s) are not reached from any crate target of their package, e.g. {}",
            orphans.len(),
            orphans[0]
        ));
        return;
    }
    for rel in orphans {
        incomplete.insert((
            Some(rel.clone()),
            "not reached from any crate target; a file of the project could not be read or parsed, so the module tree may be incomplete".to_owned(),
        ));
    }
}

/// Files that several crate targets reach are analyzed once; say so.
fn shared(tree: &Tree, of: &[String]) -> Option<String> {
    let files: BTreeSet<&String> = tree.shared.iter().filter(|rel| of.contains(rel)).collect();
    let example = files.first()?;
    Some(format!(
        "{} file(s) belong to several crate targets (a `mod` shared by tests, for example) and are analyzed once, as part of the first target, e.g. {example}",
        files.len()
    ))
}
