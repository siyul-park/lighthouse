//! The `initialize` and `index` methods: finds the packages of the requested
//! files, builds the module tree of every crate target, and reports one
//! fragment per file.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Component, Path, PathBuf},
};

use lighthouse_protocol::{
    Conventions, ErrorObject, FileInfo, Fragment, Handler, Incomplete, IndexInput, IndexParams,
    IndexResult, InitializeParams, InitializeResult, ProviderManifest, TestNaming,
};
use serde::Deserialize;

use crate::{
    cache::Cache,
    cargo::{self, Manifests, Package, Target, TargetKind},
    extract,
    names::Index,
    region,
    tree::{Root, Tree},
};

const LANGUAGE: &str = "rust";

pub struct Provider {
    id: String,
    version: String,
}

impl Handler for Provider {
    fn initialize(&mut self, _: InitializeParams) -> Result<InitializeResult, ErrorObject> {
        let language = ProviderManifest {
            id: LANGUAGE.to_owned(),
            aliases: vec!["rust".to_owned(), "rs".to_owned()],
            globs: vec!["**/*.rs".to_owned()],
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
        };
        Ok(InitializeResult::new(
            &self.id,
            &self.version,
            vec![language],
            Vec::new(),
        ))
    }

    fn index(&mut self, params: IndexParams) -> Result<IndexResult, ErrorObject> {
        let regions: Vec<_> = params
            .inputs
            .iter()
            .filter_map(|input| match input {
                IndexInput::Regions { regions } => Some(regions),
                IndexInput::File { .. } => None,
            })
            .collect();
        let options = match params.context.options.get(LANGUAGE) {
            Some(raw) => match Options::deserialize(raw) {
                Ok(options) => options,
                Err(error) => {
                    return Ok(IndexResult {
                        fragments: Vec::new(),
                        sources: Vec::new(),
                        notices: Vec::new(),
                        incomplete: vec![Incomplete {
                            path: None,
                            reason: format!("[languages.{LANGUAGE}]: {error}"),
                        }],
                    });
                }
            },
            None => Options::default(),
        };
        let provider = format!("{}@{}", self.id, self.version);
        let has_files = params
            .inputs
            .iter()
            .any(|input| matches!(input, IndexInput::File { .. }));
        if regions.is_empty() {
            return Ok(index_files(&params, &options, &provider));
        }
        let mut result = region::index(&params, &regions, &options);
        if has_files {
            let files = index_files(&params, &options, &provider);
            result.fragments.extend(files.fragments);
            result.sources.extend(files.sources);
            result.notices.extend(files.notices);
            result.incomplete.extend(files.incomplete);
            result
                .fragments
                .sort_by(|a, b| a.file.path.cmp(&b.file.path));
        }
        Ok(result)
    }
}

impl Provider {
    pub fn new(id: &str, version: &str) -> Self {
        Self {
            id: id.to_owned(),
            version: version.to_owned(),
        }
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

struct Packages {
    list: Vec<Package>,
    /// Requested file to its package, or why it has none.
    of: BTreeMap<String, Result<usize, String>>,
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
    fn add(&mut self, file: &str, stats: &extract::MacroStats) {
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

fn index_files(params: &IndexParams, options: &Options, provider: &str) -> IndexResult {
    let root = normalize(Path::new(&params.project.root));
    let requested_asts: BTreeSet<String> = params
        .inputs
        .iter()
        .filter_map(|input| match input {
            IndexInput::File {
                file,
                source_ast: true,
            } => Some(file.path.clone()),
            _ => None,
        })
        .collect();
    let (skipped, files) = requested_files(params);
    let mut incomplete: BTreeSet<(Option<String>, String)> = BTreeSet::new();
    let mut editions = Manifests::default();
    let mut fragments: BTreeMap<String, Fragment> = skipped
        .iter()
        .map(|rel| {
            let mut fragment = empty_fragment(rel);
            state_edition(&mut fragment, &root, &mut editions, &mut incomplete);
            (rel.clone(), fragment)
        })
        .collect();
    let mut notices = Vec::new();

    let found = packages(&root, &files);
    let tree = module_tree(&root, &found.list, options, overlays(params, &root));
    let idx = Index::build(&tree, &found.list);
    let mut cache = Cache::open(params, provider, &tree, &files);
    let mut orphans = Vec::new();
    let mut macros = MacroSummary::default();
    for rel in &files {
        let mut fragment = empty_fragment(rel);
        match (found.of.get(rel), tree.file_of(&normalize(&root.join(rel)))) {
            (_, Some(file)) => {
                let (extracted, stats) = analyzed(&idx, file, cache.as_mut());
                macros.add(rel, &stats);
                fragment = extracted;
            }
            (Some(Err(reason)), None) => {
                incomplete.insert((Some(rel.clone()), reason.clone()));
            }
            (_, None) => orphans.push(rel.clone()),
        }
        state_edition(&mut fragment, &root, &mut editions, &mut incomplete);
        fragments.insert(rel.clone(), fragment);
    }
    for (rel, reason) in &tree.problems {
        incomplete.insert((Some(rel.clone()), reason.clone()));
    }
    unreached(&orphans, &tree, &mut incomplete, &mut notices);
    notices.extend(tree.included(&files));
    notices.extend(macros.notice());
    notices.extend(shared(&tree));
    notices.extend(cache.and_then(Cache::save));
    let mut sources = Vec::new();
    let mut seen_trees = BTreeSet::new();
    for file in &tree.files {
        if !requested_asts.contains(&file.rel) || !seen_trees.insert(file.rel.clone()) {
            continue;
        }
        match crate::source::from_file(file) {
            Ok(source_ast) => sources.push(source_ast),
            Err(reason) => {
                incomplete.insert((
                    Some(file.rel.clone()),
                    format!("cannot expose source: {reason}"),
                ));
            }
        }
    }
    IndexResult {
        fragments: fragments.into_values().collect(),
        sources,
        notices,
        incomplete: incomplete
            .into_iter()
            .map(|(path, reason)| Incomplete { path, reason })
            .collect(),
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

/// The fragment of a file, from the cache when its text and context are
/// unchanged.
fn analyzed(
    idx: &Index,
    file: usize,
    mut cache: Option<&mut Cache>,
) -> (Fragment, extract::MacroStats) {
    let source = &idx.tree.files[file];
    if let Some(hit) = cache.as_deref_mut().and_then(|c| c.get(source)) {
        return hit;
    }
    let (fragment, stats) = extract::fragment(idx, file);
    if let Some(c) = cache {
        c.put(source, &fragment, &stats);
    }
    (fragment, stats)
}

/// The requested paths, sorted and deduplicated: the ones left alone, then
/// the ones to analyze.
fn requested_files(params: &IndexParams) -> (Vec<String>, Vec<String>) {
    let mut files: Vec<String> = params
        .inputs
        .iter()
        .filter_map(|input| match input {
            IndexInput::File { file, .. } => Some(file.path.clone()),
            IndexInput::Regions { .. } => None,
        })
        .collect();
    files.sort();
    files.dedup();
    files.into_iter().partition(|f| ignored(f))
}

/// Directories the analysis leaves alone: `testdata` and hidden ones.
fn ignored(rel: &str) -> bool {
    rel.split('/')
        .any(|part| part == "testdata" || part.starts_with('.'))
}

fn empty_fragment(path: &str) -> Fragment {
    Fragment {
        file: FileInfo {
            path: path.to_owned(),
            generated: false,
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

fn module_tree(
    root: &Path,
    packages: &[Package],
    options: &Options,
    overlays: HashMap<PathBuf, String>,
) -> Tree {
    let mut tree = Tree::new(root);
    tree.overlays = overlays;
    for r in roots(packages, options.unpublished) {
        tree.add_crate(r);
    }
    tree
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
fn shared(tree: &Tree) -> Option<String> {
    let files: BTreeSet<&String> = tree.shared.iter().collect();
    let example = files.first()?;
    Some(format!(
        "{} file(s) belong to several crate targets (a `mod` shared by tests, for example) and are analyzed once, as part of the first target, e.g. {example}",
        files.len()
    ))
}
