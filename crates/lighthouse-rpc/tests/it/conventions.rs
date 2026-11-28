//! What the manifests of the language plugins leave out of a project: the
//! conventions of their toolchains, as the host reads them.

use std::{fs, path::Path};

use lighthouse_spec::ProjectFiles;

/// The `ignores` of the plugin directory `name` of this repository, and the
/// files left of `tree` (project-relative paths) once they apply.
fn kept(name: &str, tree: &[&str]) -> Vec<String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .join(name);
    let ignores = lighthouse_rpc::load(&dir).unwrap().resource.spec.ignores;
    let root = tempfile::tempdir().unwrap();
    for file in tree {
        let path = root.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "x").unwrap();
    }
    ProjectFiles::new(root.path(), &ignores)
        .unwrap()
        .list()
        .files
        .iter()
        .map(|f| f.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn the_go_command_leaves_out_testdata_and_vendor_for_every_language() {
    let found = kept(
        "lang-go",
        &[
            "a.go",
            "pkg/b.go",
            "testdata/c.go",
            "pkg/testdata/d.go",
            "vendor/dep/e.go",
            "_tmp/f.go",
            ".github/ci.yaml",
            ".lighthouse/decisions/x.yaml",
        ],
    );
    // `_*/` and `.*/` are the `ignores` of the Go language, which the provider declares.
    assert_eq!(
        found,
        [
            ".github/ci.yaml",
            ".lighthouse/decisions/x.yaml",
            "_tmp/f.go",
            "a.go",
            "pkg/b.go"
        ]
    );
}

#[test]
fn a_listed_plugins_ignores_are_read_from_its_manifest_without_starting_it() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(".lighthouse/plugins/p");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("lighthouse-plugin.toml"),
        "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Plugin\"\n[metadata]\nname = \"p\"\n[spec]\nversion = \"0\"\nignores = [\"gen/\", \"tmp/\"]\n[spec.runtime]\ncommand = \"./does-not-exist\"\n",
    )
    .unwrap();
    let config = lighthouse_spec::Config::parse_inline("plugins = [\"p\", \"unknown\"]\n").unwrap();
    let search = lighthouse_rpc::search_dirs(root.path());
    assert_eq!(
        lighthouse_rpc::plugin_ignores(&config, root.path(), &search).unwrap(),
        ["gen/", "tmp/"]
    );
}

#[test]
fn rust_leaves_out_the_target_directory_and_nothing_hidden() {
    let found = kept(
        "lang-rust",
        &[
            "src/lib.rs",
            "target/debug/x.rs",
            ".hidden/a.rs",
            "testdata/a.rs",
        ],
    );
    assert_eq!(found, [".hidden/a.rs", "src/lib.rs", "testdata/a.rs"]);
}

#[test]
fn typescript_leaves_out_node_modules() {
    let found = kept(
        "lang-typescript",
        &["src/a.ts", "node_modules/x/b.ts", "pkg/node_modules/y/c.ts"],
    );
    assert_eq!(found, ["src/a.ts"]);
}

#[test]
fn python_leaves_out_caches_installed_packages_and_the_virtual_environment() {
    let found = kept(
        "lang-python",
        &[
            "pkg/a.py",
            "pkg/__pycache__/a.pyc",
            "lib/site-packages/x.py",
            ".venv/lib/y.py",
            "testdata/z.py",
        ],
    );
    assert_eq!(found, ["pkg/a.py", "testdata/z.py"]);
}

#[test]
fn the_plugins_name_the_files_their_version_manager_reads() {
    let named = |name: &str| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins")
            .join(name);
        lighthouse_rpc::load(&dir)
            .unwrap()
            .resource
            .spec
            .version_files
    };

    assert_eq!(named("lang-go"), [".go-version"]);
    assert_eq!(
        named("lang-rust"),
        ["rust-toolchain", "rust-toolchain.toml"]
    );
    assert_eq!(named("lang-python"), [".python-version"]);
    assert_eq!(named("lang-typescript"), [".node-version"]);
}

#[test]
fn plugin_version_files() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(".lighthouse/plugins/p");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("lighthouse-plugin.toml"),
        "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Plugin\"\n[metadata]\nname = \"p\"\n[spec]\nversion = \"0\"\nversionFiles = [\".p-version\"]\n[spec.runtime]\ncommand = \"./does-not-exist\"\n",
    )
    .unwrap();
    let config = lighthouse_spec::Config::parse_inline("plugins = [\"p\", \"unknown\"]\n").unwrap();
    let search = lighthouse_rpc::search_dirs(root.path());

    let found = lighthouse_rpc::plugin_version_files(&config, root.path(), &search).unwrap();

    assert_eq!(found, [".p-version"]);
}

fn plugin_dir(root: &Path, name: &str, spec_extra: &str) {
    let dir = root.join(".lighthouse/plugins").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("lighthouse-plugin.toml"),
        format!(
            "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Plugin\"\n[metadata]\nname = \"{name}\"\n[spec]\nversion = \"0\"\n{spec_extra}[spec.runtime]\ncommand = \"./does-not-exist\"\n"
        ),
    )
    .unwrap();
}

fn plugin_env_of(root: &Path, plugins: &str) -> Result<Vec<(String, String)>, String> {
    let config = lighthouse_spec::Config::parse_inline(&format!("plugins = {plugins}\n")).unwrap();
    let search = lighthouse_rpc::search_dirs(root);
    lighthouse_rpc::plugin_env(&config, root, &search).map_err(|e| e.to_string())
}

#[test]
fn plugin_env_is_the_union_of_the_variables_the_plugins_declare() {
    let root = tempfile::tempdir().unwrap();
    plugin_dir(root.path(), "p", "env = { A = \"1\" }\n");
    plugin_dir(root.path(), "q", "env = { A = \"1\", B = \"2\" }\n");

    let found = plugin_env_of(root.path(), "[\"p\", \"q\"]").unwrap();

    assert_eq!(
        found,
        [
            ("A".to_owned(), "1".to_owned()),
            ("B".to_owned(), "2".to_owned())
        ]
    );
}

#[test]
fn plugin_env_refuses_one_name_with_two_values_naming_both_plugins() {
    let root = tempfile::tempdir().unwrap();
    plugin_dir(root.path(), "p", "env = { A = \"1\" }\n");
    plugin_dir(root.path(), "q", "env = { A = \"2\" }\n");

    let why = plugin_env_of(root.path(), "[\"p\", \"q\"]").unwrap_err();

    assert!(
        why.contains("`A`") && why.contains("`p`") && why.contains("`q`"),
        "{why}"
    );
}

#[test]
fn plugin_env_refuses_the_names_lighthouse_sets_or_forwards() {
    for name in ["LIGHTHOUSE_X", "PATH", "HOME", "TMPDIR", "LANG"] {
        let root = tempfile::tempdir().unwrap();
        plugin_dir(root.path(), "p", &format!("env = {{ {name} = \"1\" }}\n"));

        let why = plugin_env_of(root.path(), "[\"p\"]").unwrap_err();

        assert!(why.contains(name) && why.contains("`p`"), "{why}");
    }
}
