//! The catalog a plugin ships, found from its manifest without starting it.

use std::{fs, path::Path};

use lighthouse_spec::Config;

const WITH_CATALOG: &str = "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Plugin\"\n[metadata]\nname = \"lang-x\"\n[spec]\nversion = \"1\"\ncatalog = \"decisions\"\n[spec.runtime]\ncommand = \"./run\"\n";
const BUNDLED: &str = "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Plugin\"\n[metadata]\nname = \"core\"\n[spec]\nversion = \"1\"\ncatalog = \"decisions\"\n";

fn plugin(root: &Path, dir: &str, manifest: &str, pack: &str) {
    let at = root.join(dir);
    fs::create_dir_all(at.join("decisions").join(pack)).unwrap();
    fs::write(at.join("lighthouse-plugin.toml"), manifest).unwrap();
    fs::write(at.join("decisions").join(pack).join("pack.yaml"), "").unwrap();
}

#[test]
fn a_manifest_names_its_catalog_and_a_bundled_one_has_no_process() {
    let dir = tempfile::tempdir().unwrap();
    plugin(dir.path(), "lang-x", WITH_CATALOG, "x");
    plugin(dir.path(), "core", BUNDLED, "core");

    let process = lighthouse_rpc::load(&dir.path().join("lang-x")).unwrap();
    let bundled = lighthouse_rpc::load(&dir.path().join("core")).unwrap();

    assert_eq!(
        process.catalog_dir(),
        Some(dir.path().join("lang-x/decisions"))
    );
    assert!(process.resource.spec.runtime.is_some());
    assert!(bundled.resource.spec.runtime.is_none());
}

#[test]
fn the_catalogs_of_the_listed_plugins_are_found_and_the_projects_own_are_told_apart() {
    let dir = tempfile::tempdir().unwrap();
    plugin(dir.path(), "own/lang-x", WITH_CATALOG, "x");
    let config = Config::parse_inline(
        "plugins = [{ id = \"lang-x\", path = \"own/lang-x\" }, \"lang-y\"]\n",
    )
    .unwrap();

    let found = lighthouse_rpc::plugin_catalogs(&config, dir.path(), &[]).unwrap();

    assert_eq!(found.len(), 1, "an unknown plugin ships nothing here");
    assert_eq!(found[0].plugin, "lang-x");
    assert!(found[0].local);
    assert!(found[0].dir.ends_with("own/lang-x/decisions"));
}

#[test]
fn a_plugin_found_in_the_search_locations_does_not_come_with_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let search = dir.path().join("installed");
    plugin(&search, "lang-x", WITH_CATALOG, "x");
    let config = Config::parse_inline("plugins = [\"lang-x\"]\n").unwrap();

    let found = lighthouse_rpc::plugin_catalogs(&config, dir.path(), std::slice::from_ref(&search))
        .unwrap();

    assert_eq!(found.len(), 1);
    assert!(!found[0].local);
}

#[test]
fn the_plugin_of_a_pack_is_the_one_that_holds_it_else_the_plugin_of_the_language() {
    let dir = tempfile::tempdir().unwrap();
    let search = dir.path().join("installed");
    plugin(
        &search,
        "tools",
        &WITH_CATALOG.replace("lang-x", "toolbox"),
        "x",
    );

    let by_catalog = lighthouse_rpc::plugin_of_pack("x", &[], std::slice::from_ref(&search));
    let by_language = lighthouse_rpc::plugin_of_pack("go", &[], std::slice::from_ref(&search));

    assert_eq!(by_catalog, "toolbox");
    assert_eq!(by_language, "lang-go");
}
