//! Packages and their crate targets, read from `Cargo.toml` with the `toml`
//! crate. Cargo itself is never run: analysis stays hermetic, needs no network
//! or lock file, and runs no build script.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use toml::{Table, Value};

const DEP_SECTIONS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
/// The sections whose crates are linked into what the package ships.
const RUNTIME_SECTIONS: [&str; 2] = ["dependencies", "build-dependencies"];

/// The edition Cargo gives a package that sets none.
const DEFAULT_EDITION: &str = "2015";

#[derive(Debug)]
pub struct Package {
    pub name: String,
    pub lib_name: String,
    /// Whether the package can be published to a registry; one that cannot
    /// (`publish = false`) has no consumers outside the project.
    pub publishable: bool,
    pub targets: Vec<Target>,
    /// Extern crate name (as written in code) to the package that provides it.
    pub deps: BTreeMap<String, String>,
    /// The packages that `[dependencies]` and `[build-dependencies]` name, as
    /// opposed to `[dev-dependencies]`.
    pub runtime: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct Target {
    pub kind: TargetKind,
    pub name: String,
    pub root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetKind {
    Lib,
    Bin,
    Test,
    Example,
    Bench,
    Build,
}

impl TargetKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lib => "lib",
            Self::Bin => "bin",
            Self::Test => "test",
            Self::Example => "example",
            Self::Bench => "bench",
            Self::Build => "build",
        }
    }
}

/// The manifests read so far, by directory, so that each is parsed once.
#[derive(Default)]
pub struct Manifests(BTreeMap<PathBuf, Option<Result<Table, String>>>);

impl Manifests {
    /// The edition of the package that holds `file` (a path under `root`): the
    /// `edition` of the nearest `Cargo.toml` with a `[package]`, or the one its
    /// workspace sets for it (`edition.workspace = true`), or Cargo's own
    /// default, 2015, for a package that sets none. A file no package holds
    /// has the default too, which is also what `rustfmt` assumes.
    pub fn edition(&mut self, root: &Path, file: &Path) -> Result<String, String> {
        let mut dir = file.parent().map(Path::to_owned);
        while let Some(current) = dir.filter(|d| d.starts_with(root)) {
            if let Some(manifest) = self.manifest(&current)? {
                let Some(package) = manifest.get("package").and_then(Value::as_table) else {
                    dir = current.parent().map(Path::to_owned);
                    continue;
                };
                return match package.get("edition") {
                    None => Ok(DEFAULT_EDITION.to_owned()),
                    Some(Value::String(edition)) => Ok(edition.clone()),
                    Some(inherited)
                        if inherited.get("workspace").and_then(Value::as_bool) == Some(true) =>
                    {
                        self.workspace_edition(root, &current)
                    }
                    Some(other) => Err(format!(
                        "{}: `edition` is {other}, not a string",
                        current.join("Cargo.toml").display()
                    )),
                };
            }
            dir = current.parent().map(Path::to_owned);
        }
        Ok(DEFAULT_EDITION.to_owned())
    }

    /// The `[workspace.package] edition` of the nearest workspace at or above `from`.
    fn workspace_edition(&mut self, root: &Path, from: &Path) -> Result<String, String> {
        let mut dir = Some(from.to_owned());
        while let Some(current) = dir.filter(|d| d.starts_with(root)) {
            if let Some(manifest) = self.manifest(&current)?
                && let Some(workspace) = manifest.get("workspace").and_then(Value::as_table)
            {
                return match workspace
                    .get("package")
                    .and_then(|p| p.get("edition"))
                    .and_then(Value::as_str)
                {
                    Some(edition) => Ok(edition.to_owned()),
                    None => Err(format!(
                        "{}: the package inherits its edition, and the workspace sets none",
                        from.join("Cargo.toml").display()
                    )),
                };
            }
            dir = current.parent().map(Path::to_owned);
        }
        Err(format!(
            "{}: the package inherits its edition from a workspace that is not there",
            from.join("Cargo.toml").display()
        ))
    }

    /// The manifest in `dir`, parsed once; `None` when there is none.
    fn manifest(&mut self, dir: &Path) -> Result<Option<&Table>, String> {
        let loaded = self.0.entry(dir.to_owned()).or_insert_with(|| {
            let path = dir.join("Cargo.toml");
            let text = fs::read_to_string(&path).ok()?;
            Some(
                text.parse::<Table>()
                    .map_err(|e| format!("{}: {e}", path.display())),
            )
        });
        match loaded {
            None => Ok(None),
            Some(Ok(table)) => Ok(Some(table)),
            Some(Err(reason)) => Err(reason.clone()),
        }
    }
}

/// Reads the package of the manifest in `dir`; `None` for a virtual workspace
/// manifest or a missing manifest. `Err` carries a reason.
pub fn load(dir: &Path) -> Result<Option<Package>, String> {
    let path = dir.join("Cargo.toml");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(None);
    };
    let manifest: Table = text
        .parse()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let Some(package) = manifest.get("package").and_then(Value::as_table) else {
        return Ok(None);
    };
    let name = package
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{}: package has no name", path.display()))?
        .to_owned();
    let lib_name = manifest
        .get("lib")
        .and_then(|l| l.get("name"))
        .and_then(Value::as_str)
        .map_or_else(|| crate_name(&name), crate_name);
    Ok(Some(Package {
        publishable: publishable(dir, package),
        targets: targets(dir, &name, package, &manifest),
        deps: dependencies(&manifest, &DEP_SECTIONS),
        runtime: dependencies(&manifest, &RUNTIME_SECTIONS)
            .into_values()
            .collect(),
        name,
        lib_name,
    }))
}

/// The package assumed for Rust files that no `Cargo.toml` governs: the
/// standard layout under `dir`, named `workspace`.
pub fn synthetic(dir: &Path) -> Package {
    let empty = Table::new();
    Package {
        name: "workspace".to_owned(),
        lib_name: "workspace".to_owned(),
        publishable: true,
        targets: targets(dir, "workspace", &empty, &empty),
        deps: BTreeMap::new(),
        runtime: BTreeSet::new(),
    }
}

/// How code spells a crate: hyphens become underscores.
pub fn crate_name(name: &str) -> String {
    name.replace('-', "_")
}

/// `publish = false`, `publish = []`, or `publish.workspace = true` with the
/// workspace saying so.
fn publishable(dir: &Path, package: &Table) -> bool {
    match package.get("publish") {
        None => true,
        Some(Value::Table(t)) if t.get("workspace").and_then(Value::as_bool) == Some(true) => dir
            .ancestors()
            .skip(1)
            .find_map(workspace_publish)
            .unwrap_or(true),
        Some(other) => is_publishable(other),
    }
}

fn workspace_publish(dir: &Path) -> Option<bool> {
    let text = fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let manifest: Table = text.parse().ok()?;
    let package = manifest.get("workspace")?.get("package")?;
    Some(package.get("publish").is_none_or(is_publishable))
}

fn is_publishable(value: &Value) -> bool {
    match value {
        Value::Boolean(flag) => *flag,
        Value::Array(registries) => !registries.is_empty(),
        _ => true,
    }
}

fn dependencies(manifest: &Table, sections: &[&str]) -> BTreeMap<String, String> {
    let mut tables: Vec<&Table> = Vec::new();
    collect_dependency_tables(manifest, sections, &mut tables);
    if let Some(platforms) = manifest.get("target").and_then(Value::as_table) {
        for platform in platforms.values().filter_map(Value::as_table) {
            collect_dependency_tables(platform, sections, &mut tables);
        }
    }
    let mut found = BTreeMap::new();
    for (key, value) in tables.into_iter().flatten() {
        let real = value
            .get("package")
            .and_then(Value::as_str)
            .unwrap_or(key.as_str());
        found.insert(crate_name(key), real.to_owned());
    }
    found
}

fn collect_dependency_tables<'a>(from: &'a Table, sections: &[&str], into: &mut Vec<&'a Table>) {
    for section in sections {
        into.extend(from.get(*section).and_then(Value::as_table));
    }
}

fn targets(dir: &Path, package: &str, section: &Table, manifest: &Table) -> Vec<Target> {
    let mut found = Vec::new();
    let auto = |key: &str| section.get(key).and_then(Value::as_bool).unwrap_or(true);
    lib_target(dir, package, manifest, &mut found);
    explicit(dir, manifest, "bin", TargetKind::Bin, &mut found);
    if auto("autobins") {
        let main = dir.join("src/main.rs");
        if main.is_file() {
            push(&mut found, TargetKind::Bin, package, main);
        }
        discover(dir, "src/bin", TargetKind::Bin, &mut found);
    }
    for (key, kind, folder) in [
        ("test", TargetKind::Test, "tests"),
        ("example", TargetKind::Example, "examples"),
        ("bench", TargetKind::Bench, "benches"),
    ] {
        explicit(dir, manifest, key, kind, &mut found);
        if auto(&format!("auto{key}s")) {
            discover(dir, folder, kind, &mut found);
        }
    }
    build_target(dir, section, &mut found);
    found
}

fn lib_target(dir: &Path, package: &str, manifest: &Table, found: &mut Vec<Target>) {
    let lib = manifest.get("lib").and_then(Value::as_table);
    let path = lib
        .and_then(|l| l.get("path"))
        .and_then(Value::as_str)
        .map_or_else(|| dir.join("src/lib.rs"), |p| dir.join(p));
    if path.is_file() {
        push(found, TargetKind::Lib, package, path);
    }
}

fn push(found: &mut Vec<Target>, kind: TargetKind, name: &str, root: PathBuf) {
    if !found.iter().any(|t| t.root == root) {
        found.push(Target {
            kind,
            name: name.to_owned(),
            root,
        });
    }
}

fn explicit(dir: &Path, manifest: &Table, key: &str, kind: TargetKind, found: &mut Vec<Target>) {
    let Some(list) = manifest.get(key).and_then(Value::as_array) else {
        return;
    };
    let folder = match kind {
        TargetKind::Bin => "src/bin",
        TargetKind::Test => "tests",
        TargetKind::Example => "examples",
        _ => "benches",
    };
    for entry in list.iter().filter_map(Value::as_table) {
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        let root = entry.get("path").and_then(Value::as_str).map_or_else(
            || dir.join(folder).join(format!("{name}.rs")),
            |p| dir.join(p),
        );
        if root.is_file() {
            push(found, kind, name, root);
        }
    }
}

/// `<folder>/*.rs` and `<folder>/*/main.rs`, named after the file or directory.
fn discover(dir: &Path, folder: &str, kind: TargetKind, found: &mut Vec<Target>) {
    let Ok(entries) = fs::read_dir(dir.join(folder)) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        let name = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().into_owned());
        if path.is_file() && path.extension().is_some_and(|e| e == "rs") {
            push(found, kind, &name(&path).unwrap_or_default(), path);
        } else if path.join("main.rs").is_file() {
            let dir_name = path.file_name().map(|s| s.to_string_lossy().into_owned());
            push(
                found,
                kind,
                &dir_name.unwrap_or_default(),
                path.join("main.rs"),
            );
        }
    }
}

fn build_target(dir: &Path, section: &Table, found: &mut Vec<Target>) {
    let path = match section.get("build") {
        Some(Value::Boolean(false)) => return,
        Some(Value::String(p)) => dir.join(p),
        _ => dir.join("build.rs"),
    };
    if path.is_file() {
        push(found, TargetKind::Build, "build-script", path);
    }
}
