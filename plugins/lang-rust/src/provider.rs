//! The `initialize` and `index` methods: finds the packages of the requested
//! files, builds the module tree of every crate target, and reports one
//! fragment per file.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

use lighthouse_protocol::{
    self as wire, Conventions, FileInfo, Fragment, Handler, Incomplete, IndexParams, IndexResult,
    InitializeParams, InitializeResult, Language,
};
use serde::Deserialize;

use crate::{
    cargo::{self, Package, Target, TargetKind},
    extract,
    names::Index,
    tree::{Root, Tree},
};

const LANGUAGE: &str = "rust";

pub struct Provider {
    id: String,
    version: String,
}

impl Handler for Provider {
    fn initialize(&mut self, params: InitializeParams) -> Result<InitializeResult, String> {
        if params.protocol_version != wire::VERSION {
            return Err(format!(
                "host speaks protocol {}, this plugin speaks {}",
                params.protocol_version,
                wire::VERSION
            ));
        }
        Ok(InitializeResult {
            id: self.id.clone(),
            version: self.version.clone(),
            protocol_version: wire::VERSION.to_owned(),
            languages: vec![Language {
                id: LANGUAGE.to_owned(),
                globs: vec!["**/*.rs".to_owned()],
                priority: 0,
                fallback: false,
                conventions: Conventions {
                    test_globs: vec![
                        "**/tests/**/*.rs".to_owned(),
                        "**/tests.rs".to_owned(),
                        "**/benches/**/*.rs".to_owned(),
                    ],
                },
                capabilities: Vec::new(),
            }],
        })
    }

    fn index(&mut self, params: IndexParams) -> Result<IndexResult, String> {
        let options = match params.context.options.get(LANGUAGE) {
            Some(raw) => match Options::deserialize(raw) {
                Ok(options) => options,
                Err(error) => {
                    return Ok(IndexResult {
                        fragments: Vec::new(),
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
        Ok(index_files(&params, &options))
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
struct Options {
    /// What the `pub` items of a library with `publish = false` are.
    #[serde(default)]
    unpublished: Unpublished,
}

/// Whether the public items of an unpublished library are a contract.
#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Unpublished {
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
enum Governor {
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

fn index_files(params: &IndexParams, options: &Options) -> IndexResult {
    let root = normalize(Path::new(&params.project.root));
    let (skipped, files) = requested_files(params);
    let mut fragments: BTreeMap<String, Fragment> = skipped
        .iter()
        .map(|rel| (rel.clone(), empty_fragment(rel)))
        .collect();
    let mut incomplete: BTreeSet<(Option<String>, String)> = BTreeSet::new();
    let mut notices = Vec::new();

    let found = packages(&root, &files);
    let tree = module_tree(&root, &found.list, options);
    let idx = Index::build(&tree, &found.list);
    let mut orphans = Vec::new();
    let mut macros = MacroSummary::default();
    for rel in &files {
        let mut fragment = empty_fragment(rel);
        match (found.of.get(rel), tree.file_of(&normalize(&root.join(rel)))) {
            (_, Some(file)) => {
                let extracted = extract::Extracted::of(&idx, file);
                macros.add(rel, &extracted.stats);
                fragment = extracted.fragment;
            }
            (Some(Err(reason)), None) => {
                incomplete.insert((Some(rel.clone()), reason.clone()));
            }
            (_, None) => orphans.push(rel.clone()),
        }
        fragments.insert(rel.clone(), fragment);
    }
    for (rel, reason) in &tree.problems {
        incomplete.insert((Some(rel.clone()), reason.clone()));
    }
    unreached(&orphans, &tree, &mut incomplete, &mut notices);
    notices.extend(included(&tree, &files));
    notices.extend(macros.notice());
    notices.extend(shared(&tree));
    IndexResult {
        fragments: fragments.into_values().collect(),
        notices,
        incomplete: incomplete
            .into_iter()
            .map(|(path, reason)| Incomplete { path, reason })
            .collect(),
    }
}

/// The requested paths, sorted and deduplicated: the ones left alone, then
/// the ones to analyze.
fn requested_files(params: &IndexParams) -> (Vec<String>, Vec<String>) {
    let mut files: Vec<String> = params.files.iter().map(|f| f.path.clone()).collect();
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
fn governor(
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

fn module_tree(root: &Path, packages: &[Package], options: &Options) -> Tree {
    let mut tree = Tree::new(root);
    for r in roots(packages, options.unpublished) {
        tree.add_crate(r);
    }
    tree
}

fn roots(packages: &[Package], mode: Unpublished) -> Vec<Root> {
    let used = depended_on(packages);
    let mut roots = Vec::new();
    for (at, package) in packages.iter().enumerate() {
        let mut targets: Vec<&Target> = package.targets.iter().collect();
        targets.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
        let has_lib = targets.iter().any(|t| t.kind == TargetKind::Lib);
        let base = cargo::crate_name(&package.name);
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
                test_of: (target.kind == TargetKind::Test && has_lib).then(|| base.clone()),
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

/// `include!` pastes a file the analysis does not read; say so once.
fn included(tree: &Tree, requested: &[String]) -> Option<String> {
    let mut count = 0;
    let mut example = None;
    for m in 0..tree.mods.len() {
        let file = &tree.files[tree.mods[m].file].rel;
        if !requested.contains(file) {
            continue;
        }
        for item in tree.items(m) {
            if let syn::Item::Macro(mac) = item
                && mac.mac.path.is_ident("include")
            {
                count += 1;
                example.get_or_insert_with(|| file.clone());
            }
        }
    }
    example.map(|file| {
        format!("{count} `include!` invocation(s) paste code that is not analyzed, e.g. in {file}")
    })
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
