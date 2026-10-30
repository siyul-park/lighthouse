//! Test support: builds the bundled language plugins from source, and writes
//! the documents tests put in a project.

pub mod catalog;

/// The composition of the bundled CEL libraries, built the way every entry
/// point of the tool builds it before it reads a decision.
pub fn composition() -> Composition {
    lighthouse_checks::compose().expect("the bundled CEL libraries compose")
}

/// The CEL environment of [`composition`].
pub fn cel() -> Cel {
    composition().cel().clone()
}

/// The bundled catalog, validated in one composition shared by the tests of a
/// process, so that they do not validate it again each.
pub fn bundled() -> &'static Catalog {
    shared().bundled().expect("the bundled catalog is valid")
}

/// The bundled plugins, compiled in the composition [`bundled`] belongs to.
pub fn registry() -> Registry {
    lighthouse_checks::registry(shared()).expect("the bundled plugins build")
}

/// The language plugins of this repository that provide a pack.
const PACK_PLUGINS: [&str; 3] = ["lang-go", "lang-markdown", "lang-rust"];

/// The bundled catalog with the packs of the language plugins of this
/// repository (`go`, `markdown`, `rust`), validated once like [`bundled`]:
/// what a project that lists those plugins has.
pub fn full() -> &'static Catalog {
    static FULL: OnceLock<Catalog> = OnceLock::new();
    FULL.get_or_init(|| {
        let mut files = lighthouse_spec::Files::new();
        for plugin in PACK_PLUGINS {
            let dir = workspace().join("plugins").join(plugin).join("decisions");
            files.extend(Catalog::read_files(&dir).expect("a plugin catalog reads"));
        }
        Catalog::embedded_with(shared().cel(), files).expect("the catalogs of the plugins load")
    })
}

/// The plugins of the packs of [`full`], compiled in the composition
/// [`bundled`] belongs to.
pub fn full_registry() -> Registry {
    lighthouse_checks::registry_of(shared(), full()).expect("the plugins of the packs build")
}

fn shared() -> &'static Composition {
    static SHARED: OnceLock<Composition> = OnceLock::new();
    SHARED.get_or_init(composition)
}

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use lighthouse_checks::Composition;
use lighthouse_plugin::Registry;
use lighthouse_resource::API_VERSION;
use lighthouse_spec::{Catalog, Cel};

static LANG_GO: OnceLock<Option<PathBuf>> = OnceLock::new();
static LANG_TYPESCRIPT: OnceLock<Option<PathBuf>> = OnceLock::new();

static LANG_RUST: OnceLock<PathBuf> = OnceLock::new();

static FAKE_MODEL: OnceLock<PathBuf> = OnceLock::new();

static MODEL_LOCAL: OnceLock<PathBuf> = OnceLock::new();

static MODEL_HTTP: OnceLock<PathBuf> = OnceLock::new();

/// The text of a `lighthouse.toml`: a `Project` document around `spec`, the
/// bare TOML of its `spec` (`plugins`, `[rules]`, ...). It is four lines long,
/// the spec on one, so that a test that sets a small line limit on its files
/// does not trip over the configuration. Panics when `spec` is not TOML, so
/// that a typo in a test fails at once.
pub fn project(spec: &str) -> String {
    let spec: toml::Table = toml::from_str(spec).expect("a project spec is TOML");
    format!(
        "apiVersion = \"{API_VERSION}\"\nkind = \"Project\"\nmetadata = {{ name = \"test\" }}\nspec = {}\n",
        inline(&toml::Value::Table(spec))
    )
}

/// `config`, a `lighthouse.toml` written by [`project`], with `plugin` (a TOML
/// string or inline table) added to its `plugins`.
pub fn listing(config: &str, plugin: &str) -> String {
    let mut document: toml::Table = toml::from_str(config).expect("a project is TOML");
    let spec = document
        .get_mut("spec")
        .and_then(toml::Value::as_table_mut)
        .expect("a project has a spec");
    let entry = toml::from_str::<toml::Table>(&format!("entry = {plugin}"))
        .expect("a plugin entry is TOML")["entry"]
        .clone();
    spec.entry("plugins")
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut()
        .expect("plugins is a list")
        .push(entry);
    project(&toml::to_string(spec).expect("a spec is TOML"))
}

/// Directory of the `lang-go` plugin (manifest and binary, the layout
/// `make plugins` produces), built once per process. `None` after printing a
/// skip message when no Go toolchain runs in `plugins/lang-go`, unless `CI` or
/// `LIGHTHOUSE_REQUIRE_GO` is set, where a missing toolchain panics; a failing
/// build panics.
pub fn lang_go() -> Option<PathBuf> {
    LANG_GO.get_or_init(build_lang_go).clone()
}
/// Directory of the `lang-typescript` plugin (manifest, launcher, compiled
/// provider and its pinned dependencies), built once per process. `None` after
/// printing a skip message when no Node 24 or newer runs, unless `CI` or
/// `LIGHTHOUSE_REQUIRE_NODE` is set, where a missing Node panics; a failing
/// build panics.
pub fn lang_typescript() -> Option<PathBuf> {
    LANG_TYPESCRIPT.get_or_init(build_lang_typescript).clone()
}

/// Directory of the `lang-rust` plugin (manifest and binary), built once per
/// process with the cargo that runs the tests; a failing build panics.
pub fn lang_rust() -> PathBuf {
    LANG_RUST
        .get_or_init(|| build_rust_plugin("lang-rust"))
        .clone()
}

/// Directory of the `fake-model` plugin (manifest and binary), built once per
/// process like `lang-rust`. The binary is also a `command` provider; see its
/// documentation.
pub fn fake_model() -> PathBuf {
    FAKE_MODEL
        .get_or_init(|| build_rust_plugin("fake-model"))
        .clone()
}

/// Directory of the `model-local` plugin (manifest and binary), built once per
/// process like `lang-rust`.
pub fn model_local() -> PathBuf {
    MODEL_LOCAL
        .get_or_init(|| build_rust_plugin("model-local"))
        .clone()
}

/// Directory of the `model-http` plugin (manifest and binary), built once per
/// process like `lang-rust`.
pub fn model_http() -> PathBuf {
    MODEL_HTTP
        .get_or_init(|| build_rust_plugin("model-http"))
        .clone()
}

/// Puts the plugin in the directory `plugin` into the plugin directory of the
/// data home `home`, as a user installs one: a project then lists it by its
/// id, and the project needs no trust for it.
pub fn installed(home: &Path, plugin: &Path) {
    let plugins = home.join("lighthouse/plugins");
    fs::create_dir_all(&plugins).expect("create plugin dir");
    let name = plugin.file_name().expect("a plugin directory name");
    #[cfg(unix)]
    std::os::unix::fs::symlink(plugin, plugins.join(name)).expect("link plugin");
}

/// `value` as TOML on one line.
fn inline(value: &toml::Value) -> String {
    match value {
        toml::Value::Table(table) => {
            let entries: Vec<String> = table
                .iter()
                .map(|(key, value)| format!("{} = {}", quoted(key), inline(value)))
                .collect();
            format!("{{ {} }}", entries.join(", "))
        }
        toml::Value::Array(items) => {
            let items: Vec<String> = items.iter().map(inline).collect();
            format!("[{}]", items.join(", "))
        }
        scalar => scalar.to_string(),
    }
}

fn quoted(key: &str) -> String {
    toml::Value::String(key.to_owned()).to_string()
}

fn build_lang_go() -> Option<PathBuf> {
    let source = workspace().join("plugins/lang-go");
    let version = Command::new("go")
        .arg("version")
        .current_dir(&source)
        .output();
    if !version.is_ok_and(|o| o.status.success()) {
        assert!(
            env::var_os("CI").is_none() && env::var_os("LIGHTHOUSE_REQUIRE_GO").is_none(),
            "no Go toolchain runs in plugins/lang-go (see .go-version) and Go is required here"
        );
        eprintln!("skipping: no Go toolchain runs in plugins/lang-go (see .go-version)");
        return None;
    }
    let out = workspace().join("target/plugins-test/lang-go");
    fs::create_dir_all(&out).expect("create plugin dir");
    install(&out.join("lang-go"), |tmp| {
        let status = Command::new("go")
            .args(["build", "-o"])
            .arg(tmp)
            .arg("./cmd/lang-go")
            .current_dir(&source)
            .status()
            .expect("run go build");
        assert!(status.success(), "go build of plugins/lang-go failed");
    });
    install_manifest(&source, &out, &goroot(&source));
    Some(out)
}

/// The oldest Node the provider runs on.
const MIN_NODE: u32 = 24;

fn build_lang_typescript() -> Option<PathBuf> {
    let source = workspace().join("plugins/lang-typescript");
    if node_major(&source).is_none_or(|major| major < MIN_NODE) {
        assert!(
            env::var_os("CI").is_none() && env::var_os("LIGHTHOUSE_REQUIRE_NODE").is_none(),
            "Node {MIN_NODE} or newer is required here to run plugins/lang-typescript"
        );
        eprintln!("skipping: no Node {MIN_NODE} or newer for plugins/lang-typescript");
        return None;
    }
    install_node_modules(&source);
    let out = workspace().join("target/plugins-test/lang-typescript");
    fs::create_dir_all(&out).expect("create plugin dir");
    let built = out.join(format!("build.{}.tmp", std::process::id()));
    let status = Command::new("node")
        .arg("scripts/wire.mjs")
        .current_dir(&source)
        .status()
        .expect("run wire.mjs");
    assert!(
        status.success(),
        "wire.mjs of plugins/lang-typescript failed"
    );
    let status = Command::new("node")
        .arg("node_modules/typescript/bin/tsc")
        .args(["-p", "tsconfig.json", "--outDir"])
        .arg(&built)
        .current_dir(&source)
        .status()
        .expect("run tsc");
    assert!(status.success(), "tsc of plugins/lang-typescript failed");
    install_tree(&built.join("src"), &out.join("dist/src"));
    fs::remove_dir_all(&built).expect("remove build directory");
    for name in ["lang-typescript", "lighthouse-plugin.toml", "package.json"] {
        install(&out.join(name), |tmp| {
            fs::copy(source.join(name), tmp).expect("copy plugin file");
            #[cfg(unix)]
            fs::set_permissions(tmp, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        });
    }
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(source.join("node_modules"), out.join("node_modules"));
    Some(out)
}

/// The major version of the Node on `PATH`, when there is one.
fn node_major(dir: &Path) -> Option<u32> {
    let output = Command::new("node")
        .arg("--version")
        .current_dir(dir)
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    text.trim()
        .strip_prefix('v')?
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// Installs the pinned dependencies without running any script, unless the
/// last install is newer than the lock file.
fn install_node_modules(source: &Path) {
    let installed = fs::metadata(source.join("node_modules/.package-lock.json"))
        .and_then(|m| m.modified())
        .ok();
    let locked = fs::metadata(source.join("package-lock.json"))
        .and_then(|m| m.modified())
        .expect("package-lock.json of plugins/lang-typescript");
    if installed.is_some_and(|installed| installed >= locked) {
        return;
    }
    let status = Command::new("npm")
        .args(["ci", "--ignore-scripts"])
        .current_dir(source)
        .status()
        .expect("run npm ci");
    assert!(status.success(), "npm ci of plugins/lang-typescript failed");
}

/// Puts every file of the tree `from` at the same place under `to`, each as `install` does.
fn install_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create directory");
    for entry in fs::read_dir(from).expect("read build directory") {
        let entry = entry.expect("directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            install_tree(&entry.path(), &target);
        } else {
            install(&target, |tmp| {
                fs::copy(entry.path(), tmp).expect("copy compiled file");
            });
        }
    }
}

fn goroot(dir: &Path) -> String {
    let output = Command::new("go")
        .args(["env", "GOROOT"])
        .current_dir(dir)
        .output()
        .expect("run go env");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Installs the manifest. On unix the plugin starts through a launcher that
/// puts the toolchain that built it first on `PATH`, so tests do not depend on
/// version manager shims resolving a toolchain from the directory of a
/// throwaway project.
fn install_manifest(source: &Path, out: &Path, goroot: &str) {
    let mut text = fs::read_to_string(source.join("lighthouse-plugin.toml")).unwrap();
    if cfg!(unix) {
        let script = format!(
            "#!/bin/sh\nPATH=\"{goroot}/bin:$PATH\" exec \"$(dirname \"$0\")/lang-go\" \"$@\"\n"
        );
        install(&out.join("run"), |tmp| {
            fs::write(tmp, script).unwrap();
            #[cfg(unix)]
            fs::set_permissions(tmp, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        });
        text = text.replace("command = \"./lang-go\"", "command = \"./run\"");
    }
    install(&out.join("lighthouse-plugin.toml"), |tmp| {
        fs::write(tmp, text).unwrap();
    });
    link_catalog(source, out);
}

/// Makes the `decisions` directory of the plugin source `source`, if it has
/// one, the catalog of the installed plugin `out`.
fn link_catalog(source: &Path, out: &Path) {
    let catalog = source.join("decisions");
    if !catalog.is_dir() {
        return;
    }
    #[cfg(unix)]
    install(&out.join("decisions"), |tmp| {
        std::os::unix::fs::symlink(&catalog, tmp).expect("link plugin catalog");
    });
}

/// Puts the file `make` writes at `to`: it is written beside it under a name of
/// this process and renamed into place, so that test processes building the
/// same plugin at once never see a partial file.
fn install(to: &Path, make: impl FnOnce(&Path)) {
    let name = to.file_name().expect("a file name").to_string_lossy();
    let tmp = to.with_file_name(format!("{name}.{}.tmp", std::process::id()));
    make(&tmp);
    fs::rename(&tmp, to).unwrap_or_else(|e| panic!("install {}: {e}", to.display()));
}

/// Builds the plugin crate `name` under `plugins/` and installs its manifest
/// and binary in `target/plugins-test/<name>`.
fn build_rust_plugin(name: &str) -> PathBuf {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(&cargo)
        .args(["build", "-p", name])
        .current_dir(workspace())
        .status()
        .expect("run cargo build");
    assert!(status.success(), "cargo build of {name} failed");
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace().join("target"));
    let binary = format!("{name}{}", env::consts::EXE_SUFFIX);
    let built = target.join("debug").join(&binary);
    let out = workspace().join("target/plugins-test").join(name);
    fs::create_dir_all(&out).expect("create plugin dir");
    install(&out.join(&binary), |tmp| {
        fs::copy(&built, tmp).expect("copy plugin binary");
    });
    let manifest = workspace()
        .join("plugins")
        .join(name)
        .join("lighthouse-plugin.toml");
    install(&out.join("lighthouse-plugin.toml"), |tmp| {
        fs::copy(&manifest, tmp).expect("copy plugin manifest");
    });
    link_catalog(&workspace().join("plugins").join(name), &out);
    out
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
