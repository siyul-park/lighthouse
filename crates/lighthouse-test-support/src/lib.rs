//! Test support: builds the bundled language plugins from source, and writes
//! the documents tests put in a project.

pub mod catalog;

/// Installs the CEL environment of the bundled libraries, as every entry point
/// of the tool does before it reads a decision. A test binary calls it from a
/// constructor, so that it runs before any test; it is idempotent.
pub fn cel() {
    lighthouse_checks::compose().expect("the bundled CEL libraries compose");
}

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use lighthouse_resource::API_VERSION;

static LANG_GO: OnceLock<Option<PathBuf>> = OnceLock::new();

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

/// Directory of the `lang-go` plugin (manifest and binary, the layout
/// `make plugins` produces), built once per process. `None` after printing a
/// skip message when no Go toolchain runs in `plugins/lang-go`, unless `CI` or
/// `LIGHTHOUSE_REQUIRE_GO` is set, where a missing toolchain panics; a failing
/// build panics.
pub fn lang_go() -> Option<PathBuf> {
    LANG_GO.get_or_init(build_lang_go).clone()
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
    out
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
