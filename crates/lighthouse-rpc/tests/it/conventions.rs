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
fn the_go_command_leaves_out_testdata_vendor_and_hidden_or_underscore_directories() {
    let found = kept(
        "lang-go",
        &[
            "a.go",
            "pkg/b.go",
            "testdata/c.go",
            "pkg/testdata/d.go",
            "vendor/dep/e.go",
            "_tmp/f.go",
            ".hidden/g.go",
            ".github/ci.yaml",
            ".lighthouse/decisions/x.yaml",
            ".lighthouse/cache/y.json",
        ],
    );
    assert_eq!(found, [".lighthouse/decisions/x.yaml", "a.go", "pkg/b.go"]);
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
