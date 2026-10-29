//! The Rust fences of host files, each a unit of its own, as rustdoc reads a
//! doctest: its own crate that uses the crate around the Markdown file from
//! outside, so a `use <crate>::` path is an edge to the crate's public symbols.
//! `language.context.crate` names a module file instead, and the fence is code
//! inside that module, where `crate::` and `super::` resolve.
//!
//! What the fences declare and call is reported in the host's coordinates. A
//! fence that is not valid Rust, or that uses names the project does not hold,
//! is a notice: prose shows fragments, and a fragment is not a gap.

mod remap;
mod text;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use lighthouse_protocol::{
    EdgeKind, EmbeddedFile, EmbeddedSource, FileInfo, Fragment, Incomplete, IndexParams,
    IndexResult,
};

use crate::{
    cargo::{Package, TargetKind},
    extract,
    names::Index,
    provider::{Governor, Options, governor, normalize, overlays, roots},
    tree::{Root, Tree},
};
use remap::{Lines, Remap};
use text::Text;

/// Where the units are, beside their Markdown file.
const DIR: &str = ".lighthouse-snippets";

/// The context key that puts a fence inside the module a file declares.
const CRATE: &str = "crate";

struct Snippet<'a> {
    host: &'a EmbeddedFile,
    source: &'a EmbeddedSource,
    /// What the fence is: the text the compiler reads, wrapped when the fence
    /// is statements.
    text: Text,
    /// The file the unit is, which no disk has.
    file: PathBuf,
    /// The unit's module path, or the name of the module it is in `into`.
    label: String,
    /// The module file the fence is code of, from `language.context.crate`.
    into: Option<PathBuf>,
    /// The package that governs the unit, as an index of the packages read.
    package: Option<usize>,
}

pub(crate) fn index(
    params: &IndexParams,
    files: &[&EmbeddedFile],
    options: &Options,
) -> IndexResult {
    let root = normalize(Path::new(&params.project.root));
    let mut result = IndexResult::default();
    let mut snippets = snippets(&root, files);
    let mut tree = Tree::new(&root);
    tree.overlays = overlays(params, &root);
    let packages = packages(&root, &mut snippets, options, &mut tree, &mut result);
    for snippet in &snippets {
        tree.overlays
            .insert(snippet.file.clone(), snippet.text.text.clone());
    }
    for snippet in &snippets {
        add(&mut tree, snippet, packages.len() - 1, &mut result);
    }
    let index = Index::build(&tree, &packages);
    // A host file with no fence worth reading has a fragment all the same.
    let mut by_host: BTreeMap<String, Fragment> = files
        .iter()
        .map(|host| {
            let fragment = Fragment {
                file: FileInfo {
                    path: host.path.clone(),
                    ..FileInfo::default()
                },
                ..Fragment::default()
            };
            (host.path.clone(), fragment)
        })
        .collect();
    let mut unresolved = Vec::new();
    for snippet in &snippets {
        let Some(file) = tree.file_of(&snippet.file) else {
            continue;
        };
        let source = &tree.files[file];
        // A `mod name;` the fence shows has no file to find; only a fence that
        // does not parse is not analysed.
        if let Some((_, reason)) = tree.problems.iter().find(|(rel, reason)| {
            *rel == source.rel && reason.starts_with(|c: char| c.is_ascii_digit())
        }) {
            result.notices.push(format!(
                "{}:{}: the Rust fence is not valid Rust and is not analysed: {}",
                snippet.host.path,
                Lines::of(&snippet.host.text)
                    .position(snippet.source.source_range[0] as usize)
                    .line,
                reason
                    .split_once(": ")
                    .map_or(reason.as_str(), |(_, why)| why)
            ));
            continue;
        }
        let (fragment, _) = extract::fragment(&index, file);
        if imports_unknown(&fragment, &tree) {
            unresolved.push(snippet.host.path.as_str());
        }
        let lines = Lines::of(&snippet.host.text);
        let remap = Remap {
            text: &snippet.text,
            source,
            lines: &lines,
        };
        let fragment = remap.fragment(fragment, &snippet.host.path);
        let entry = by_host.entry(snippet.host.path.clone()).or_default();
        entry.file = fragment.file;
        entry.modules.extend(fragment.modules);
        entry.symbols.extend(fragment.symbols);
        entry.edges.extend(fragment.edges);
        entry.functions.extend(fragment.functions);
        entry.tests.extend(fragment.tests);
    }
    if let Some(first) = unresolved.first() {
        result.notices.push(format!(
            "{} Rust fence(s) use names the project does not hold (another crate, or what the prose defines); their edges stay syntactic, e.g. in {first}",
            unresolved.len()
        ));
    }
    result.fragments = by_host.into_values().collect();
    result
}

/// The units of the fences, in the order of the files and of the fences.
fn snippets<'a>(root: &Path, files: &[&'a EmbeddedFile]) -> Vec<Snippet<'a>> {
    let mut found = Vec::new();
    for host in files {
        let mut sources: Vec<&EmbeddedSource> = host.sources.iter().collect();
        sources.sort_by_key(|source| source.source_range);
        for (ordinal, source) in sources.into_iter().enumerate() {
            let shown = Text::of(source);
            let text = if syn::parse_file(&shown.text).is_ok() {
                shown
            } else {
                shown.wrapped()
            };
            let at = root.join(&host.path);
            let dir = at.parent().unwrap_or(root).join(DIR);
            let name = at
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            found.push(Snippet {
                host,
                source,
                text,
                file: dir
                    .join(format!("{name}.{}", ordinal + 1))
                    .join("snippet.rs"),
                label: format!("{}#{}", host.path, ordinal + 1),
                into: source
                    .context
                    .get(CRATE)
                    .and_then(serde_json::Value::as_str)
                    .map(|path| normalize(&root.join(path))),
                package: None,
            });
        }
    }
    found
}

/// The packages that govern the units, read once each, and a last one for the
/// units no manifest governs. A unit's own package is read for its library,
/// a package a unit is code of for every target.
fn packages(
    root: &Path,
    snippets: &mut [Snippet],
    options: &Options,
    tree: &mut Tree,
    result: &mut IndexResult,
) -> Vec<Package> {
    let mut by_dir = BTreeMap::new();
    let mut homes: Vec<Option<PathBuf>> = Vec::new();
    let mut whole = BTreeSet::new();
    for snippet in snippets.iter() {
        let governed = match &snippet.into {
            Some(file) => file
                .strip_prefix(root)
                .unwrap_or(file)
                .to_string_lossy()
                .into_owned(),
            None => snippet.host.path.clone(),
        };
        let home = match governor(root, &governed, &mut by_dir) {
            Governor::Manifest(dir) => Some(dir),
            Governor::Failed(reason) => {
                result.incomplete.push(Incomplete {
                    path: Some(snippet.host.path.clone()),
                    reason,
                });
                None
            }
            Governor::Nobody => None,
        };
        if let (Some(dir), Some(_)) = (&home, &snippet.into) {
            whole.insert(dir.clone());
        }
        homes.push(home);
    }
    let mut list = Vec::new();
    let mut at: BTreeMap<PathBuf, usize> = BTreeMap::new();
    for dir in homes.iter().flatten() {
        if at.contains_key(dir) {
            continue;
        }
        if let Some(Ok(Some(package))) = by_dir.remove(dir) {
            at.insert(dir.clone(), list.len());
            list.push(package);
        }
    }
    for (snippet, home) in snippets.iter_mut().zip(&homes) {
        snippet.package = home.as_ref().and_then(|dir| at.get(dir).copied());
    }
    let whole: BTreeSet<usize> = whole
        .iter()
        .filter_map(|dir| at.get(dir).copied())
        .collect();
    for mut target in roots(&list, options.unpublished) {
        if target.kind == TargetKind::Lib || whole.contains(&target.package) {
            target.exported = true;
            tree.add_crate(target);
        }
    }
    list.push(Package {
        name: "snippets".to_owned(),
        lib_name: "snippets".to_owned(),
        publishable: false,
        targets: Vec::new(),
        deps: BTreeMap::new(),
        runtime: BTreeSet::new(),
    });
    list
}

/// Adds the unit to the tree: a crate of its own, or a module of the one it is
/// code of.
fn add(tree: &mut Tree, snippet: &Snippet, none: usize, result: &mut IndexResult) {
    let Some(file) = &snippet.into else {
        tree.add_crate(Root {
            label: snippet.label.clone(),
            package: snippet.package.unwrap_or(none),
            kind: TargetKind::Example,
            file: snippet.file.clone(),
            exported: false,
            test_of: None,
        });
        return;
    };
    let module = tree.file_of(file).and_then(|at| {
        tree.mods
            .iter()
            .position(|module| module.file == at && module.inline.is_empty())
    });
    let Some(parent) = module else {
        result.incomplete.push(Incomplete {
            path: Some(snippet.host.path.clone()),
            reason: format!(
                "`language.context.{CRATE}` names {}, which is no module file of a package of the project",
                file.display()
            ),
        });
        return;
    };
    let name: String = snippet
        .label
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    tree.add_snippet(parent, &format!("__{name}"), &snippet.file);
}

/// Whether the fence imports what the project holds no module for, the
/// standard library excepted.
fn imports_unknown(fragment: &Fragment, tree: &Tree) -> bool {
    fragment.edges.iter().any(|edge| {
        edge.kind == EdgeKind::Imports
            && !matches!(
                edge.to.split(['/', ':']).next(),
                Some("std" | "core" | "alloc")
            )
            && !tree.mods.iter().any(|module| module.path == edge.to)
    })
}
