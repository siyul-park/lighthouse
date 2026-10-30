//! The manifests of the bundled plugins agree with the code that registers
//! them, and the bundled ids are the ones a project never lists.

use std::path::Path;

fn manifest_of(id: &str) -> lighthouse_rpc::Found {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .join(id);
    lighthouse_rpc::load(&dir).unwrap_or_else(|e| panic!("{id}: {e}"))
}

#[test]
fn every_bundled_manifest_matches_its_registration() {
    let registry = lighthouse_test_support::registry();
    for id in ["lang-cel", "core", "design", "testing"] {
        let found = manifest_of(id);
        let registered = registry
            .manifests()
            .iter()
            .find(|m| m.id == id)
            .unwrap_or_else(|| panic!("`{id}` is not registered"));
        assert_eq!(found.id(), registered.id, "{id}");
        assert_eq!(found.resource.spec.version, registered.version, "{id}");
        assert!(found.resource.spec.runtime.is_none(), "{id} has no process");
    }
}

#[test]
fn a_bundled_plugin_names_a_catalog_exactly_when_it_provides_packs() {
    for (id, catalog) in [
        ("lang-cel", false),
        ("core", true),
        ("design", true),
        ("testing", true),
    ] {
        let found = manifest_of(id);
        assert_eq!(found.catalog_dir().is_some(), catalog, "{id}");
        if let Some(dir) = found.catalog_dir() {
            assert!(dir.join(id).join("pack.yaml").is_file(), "{id}");
        }
    }
}

#[test]
fn the_bundled_ids_are_the_library_the_packs_and_metrics() {
    let ids = lighthouse_checks::ids(&lighthouse_test_support::composition()).unwrap();

    assert_eq!(ids, ["lang-cel", "core", "design", "testing", "metrics"]);
}

#[test]
fn the_registry_of_a_catalog_has_a_plugin_for_each_of_its_packs() {
    let registry = lighthouse_test_support::full_registry();

    for pack in lighthouse_test_support::full().packs() {
        assert!(registry.has_plugin(&pack.id), "{}", pack.id);
    }
    assert!(!lighthouse_test_support::registry().has_plugin("go"));
}
