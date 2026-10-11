//! The RPC host against fake plugins written as shell scripts: handshake,
//! failure handling, and discovery.
use std::{
    fs,
    path::{Path, PathBuf},
};

use lighthouse_plugin::{Plugin, Registry};
use lighthouse_rpc::{Error, discover, register, search_dirs};
use lighthouse_spec::Config;

#[cfg(unix)]
mod failure;
#[cfg(unix)]
mod fake;
#[cfg(unix)]
mod handshake;

// Discovery.

/// A `Plugin` document in TOML; `runtime` adds lines to the `[spec.runtime]` table.
fn plugin_document(id: &str, command: &str, runtime: &str) -> String {
    format!(
        "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Plugin\"\n[metadata]\nname = \"{id}\"\n[spec]\nversion = \"1\"\n[spec.runtime]\ncommand = \"{command}\"\n{runtime}"
    )
}

fn manifest(dir: &Path, id: &str, command: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("lighthouse-plugin.toml"),
        plugin_document(id, command, ""),
    )
    .unwrap();
}

fn config(plugins: &str) -> Config {
    Config::parse_inline(&format!("plugins = {plugins}")).unwrap()
}

fn paths(found: &[lighthouse_rpc::Found]) -> Vec<(String, PathBuf)> {
    found
        .iter()
        .map(|f| (f.manifest.id.clone(), f.dir.clone()))
        .collect()
}
#[test]
fn discovery_reads_manifests_under_each_search_directory_in_order() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    manifest(&a.path().join("two"), "two", "x");
    manifest(&a.path().join("one"), "one", "x");
    fs::create_dir_all(a.path().join("no-manifest")).unwrap();
    manifest(&b.path().join("three"), "three", "x");
    let search = [
        a.path().to_owned(),
        b.path().to_owned(),
        a.path().join("missing"),
    ];
    let found = discover(&search).unwrap().found;
    assert_eq!(
        paths(&found),
        [
            ("one".to_owned(), a.path().join("one")),
            ("two".to_owned(), a.path().join("two")),
            ("three".to_owned(), b.path().join("three")),
        ]
    );
}

#[test]
fn a_broken_manifest_only_matters_to_the_plugin_it_names() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("bad")).unwrap();
    fs::write(dir.path().join("bad/lighthouse-plugin.toml"), "id = 1").unwrap();
    let search = [dir.path().to_owned()];
    let discovered = discover(&search).unwrap();
    assert!(discovered.found.is_empty());
    assert!(
        discovered.broken[0]
            .to_string()
            .contains("bad/lighthouse-plugin.toml")
    );

    let mut registry = Registry::default();
    let registered = register(
        &mut registry,
        &config("[\"other\"]"),
        Path::new("."),
        &search,
    )
    .expect("an unlisted broken manifest is ignored");
    assert!(registered.notices[0].contains("ignored plugin manifest"));
    let err = register(&mut registry, &config("[\"bad\"]"), Path::new("."), &search).unwrap_err();
    assert!(matches!(err, Error::Manifest { .. }), "{err}");

    fs::write(
        dir.path().join("bad/lighthouse-plugin.toml"),
        plugin_document("x", "c", "extra = 1\n"),
    )
    .unwrap();
    assert_eq!(discover(&search).unwrap().broken.len(), 1);
}

#[test]
fn the_same_id_in_two_places_is_an_error_and_nothing_is_started() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    manifest(&a.path().join("lang"), "lang-x", "/does/not/exist");
    manifest(&b.path().join("lang"), "lang-x", "/does/not/exist");
    let mut registry = Registry::default();
    let err = register(
        &mut registry,
        &config("[\"lang-x\"]"),
        Path::new("."),
        &[a.path().to_owned(), b.path().to_owned()],
    )
    .unwrap_err();
    let Error::Conflict { id, places } = &err else {
        panic!("{err}");
    };
    assert_eq!(id, "lang-x");
    assert_eq!(places.len(), 2);
    assert!(err.to_string().contains("provided more than once"));
}

#[test]
fn an_id_that_is_also_built_in_is_an_error() {
    let a = tempfile::tempdir().unwrap();
    manifest(&a.path().join("core"), "core", "/does/not/exist");
    let mut registry = lighthouse_builtin_stub();
    let err = register(
        &mut registry,
        &config("[\"core\"]"),
        Path::new("."),
        &[a.path().to_owned()],
    )
    .unwrap_err();
    let Error::Conflict { places, .. } = err else {
        panic!("not a conflict");
    };
    assert_eq!(places[0], "built in");
}

/// A registry that already holds an in-process plugin called `core`.
fn lighthouse_builtin_stub() -> Registry {
    struct Core(lighthouse_plugin::PluginManifest);
    impl Plugin for Core {
        fn manifest(&self) -> &lighthouse_plugin::PluginManifest {
            &self.0
        }
    }
    let mut registry = Registry::default();
    registry
        .register(&Core(lighthouse_plugin::PluginManifest {
            id: "core".to_owned(),
            version: "0".to_owned(),
        }))
        .unwrap();
    registry
}

#[test]
fn explicit_paths_must_hold_the_listed_id_and_a_runnable_command() {
    let mismatch = tempfile::tempdir().unwrap();
    manifest(mismatch.path(), "other", "x");
    let toml = format!(
        "[{{ id = \"fake\", path = {:?} }}]",
        mismatch.path().to_str().unwrap()
    );
    let mut registry = Registry::default();
    let err = register(&mut registry, &config(&toml), Path::new("."), &[]).unwrap_err();
    assert!(matches!(err, Error::IdMismatch { .. }), "{err}");

    let missing = tempfile::tempdir().unwrap();
    manifest(missing.path(), "fake", "./nothing-here");
    let toml = format!(
        "[{{ id = \"fake\", path = {:?} }}]",
        missing.path().to_str().unwrap()
    );
    let err = register(&mut registry, &config(&toml), Path::new("."), &[]).unwrap_err();
    assert!(err.to_string().contains("cannot start"), "{err}");

    let err = register(
        &mut registry,
        &config("[{ id = \"fake\", path = \"no/such/dir\" }]"),
        Path::new("."),
        &[],
    )
    .unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err}");
}

#[test]
fn only_listed_plugins_start_and_unknown_ids_are_left_to_the_engine() {
    let search = tempfile::tempdir().unwrap();
    manifest(
        &search.path().join("unlisted"),
        "unlisted",
        "/does/not/exist",
    );
    let mut registry = Registry::default();
    register(
        &mut registry,
        &config("[\"nowhere\"]"),
        Path::new("."),
        &[search.path().to_owned()],
    )
    .unwrap();
    assert!(!registry.has_plugin("nowhere") && !registry.has_plugin("unlisted"));
}

#[test]
fn search_directories_cover_the_project_the_xdg_data_home_and_the_executable() {
    let dirs = search_dirs(Path::new("/work/project"));
    assert_eq!(dirs[0], Path::new("/work/project/.lighthouse/plugins"));
    assert!(
        dirs.iter()
            .any(|d| d.ends_with("lighthouse/plugins") && d != &dirs[0])
    );
    assert!(dirs.last().unwrap().ends_with("plugins"));
}

#[test]
fn plugin_manifest_reads_command_and_defaults_args_and_rejects_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join(lighthouse_rpc::FILE_NAME),
        plugin_document("x", "./run", "args = [\"-v\"]\n"),
    )
    .unwrap();
    let found = lighthouse_rpc::load(dir.path()).unwrap();
    let manifest: &lighthouse_rpc::ProcessManifest = &found.manifest;
    assert_eq!(
        (manifest.id.as_str(), manifest.command.as_str()),
        ("x", "./run")
    );
    assert_eq!(manifest.args, ["-v"]);

    fs::write(
        dir.path().join(lighthouse_rpc::FILE_NAME),
        plugin_document("x", "c", ""),
    )
    .unwrap();
    assert!(
        lighthouse_rpc::load(dir.path())
            .unwrap()
            .manifest
            .args
            .is_empty()
    );
    fs::write(
        dir.path().join(lighthouse_rpc::FILE_NAME),
        plugin_document("x", "c", "extra = 1\n"),
    )
    .unwrap();
    assert!(matches!(
        lighthouse_rpc::load(dir.path()),
        Err(Error::Manifest { .. })
    ));
}

#[test]
fn a_manifest_is_a_document_in_toml_yaml_or_json() {
    let toml = plugin_document("x", "./run", "args = [\"-v\"]\n");
    let yaml = "apiVersion: lighthouse/v1alpha1\nkind: Plugin\nmetadata: {name: x}\nspec:\n  version: '1'\n  runtime: {command: ./run, args: ['-v']}\n  provides: {languages: [go], orderKeys: [x/group]}\n";
    let json = r#"{"apiVersion":"lighthouse/v1alpha1","kind":"Plugin","metadata":{"name":"x"},"spec":{"version":"1","runtime":{"command":"./run","args":["-v"]}}}"#;
    for (file, text) in [
        ("lighthouse-plugin.toml", toml.as_str()),
        ("lighthouse-plugin.yaml", yaml),
        ("lighthouse-plugin.json", json),
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(file), text).unwrap();

        let found = lighthouse_rpc::load(dir.path()).unwrap();

        assert_eq!(found.manifest.id, "x", "{file}");
        assert_eq!(found.manifest.command, "./run", "{file}");
        assert_eq!(found.manifest.args, ["-v"], "{file}");
    }
}

#[test]
fn the_plugin_kind_has_a_schema_and_provides_nothing_unless_it_says_so() {
    let kinds: Vec<&str> = lighthouse_rpc::descriptors()
        .iter()
        .map(|d| d.kind)
        .collect();
    assert_eq!(kinds, ["Plugin"]);

    let bare = lighthouse_rpc::parse(
        lighthouse_resource::Format::Toml,
        "x",
        &plugin_document("x", "./run", ""),
    )
    .unwrap();
    assert_eq!(bare.provides, lighthouse_rpc::Provides::default());

    let yaml = "apiVersion: lighthouse/v1alpha1\nkind: Plugin\nmetadata: {name: x}\nspec:\n  version: '1'\n  runtime: {command: ./run}\n  provides: {languages: [go], decisions: [decisions], orderKeys: [x/group], embedders: [x/e], fixOps: [x/op]}\n";
    let rich = lighthouse_rpc::parse(lighthouse_resource::Format::Yaml, "x", yaml).unwrap();
    assert_eq!(rich.provides.languages, ["go"]);
    assert_eq!(rich.provides.order_keys, ["x/group"]);
    assert_eq!(rich.provides.fix_ops, ["x/op"]);
}

#[test]
fn a_manifest_is_found_in_a_directory_by_any_of_its_names() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(lighthouse_rpc::file_in(dir.path()), None);

    fs::write(dir.path().join("lighthouse-plugin.json"), "{}").unwrap();
    assert_eq!(
        lighthouse_rpc::file_in(dir.path()),
        Some(dir.path().join("lighthouse-plugin.json"))
    );
    fs::write(dir.path().join(lighthouse_rpc::FILE_NAME), "").unwrap();
    assert_eq!(
        lighthouse_rpc::file_in(dir.path()),
        Some(dir.path().join(lighthouse_rpc::FILE_NAME))
    );
}

#[test]
fn a_parsed_plugin_document_becomes_a_manifest() {
    let document = lighthouse_resource::Resource::new(
        lighthouse_resource::Metadata::named("x"),
        lighthouse_rpc::PluginSpec {
            version: "2".to_owned(),
            runtime: lighthouse_rpc::Runtime {
                command: "run".to_owned(),
                args: vec!["-v".to_owned()],
            },
            provides: lighthouse_rpc::Provides::default(),
        },
    );

    let manifest = lighthouse_rpc::ProcessManifest::from_resource(document);

    assert_eq!(
        (
            manifest.id.as_str(),
            manifest.version.as_str(),
            manifest.command.as_str()
        ),
        ("x", "2", "run")
    );
    assert_eq!(manifest.args, ["-v"]);
}
