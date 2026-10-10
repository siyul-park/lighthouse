//! The skill digest comes from the catalog, never from a started plugin: every
//! rule the engine's registry enables must already be in it.

use std::fs;

use lighthouse_engine::active_rules;
use lighthouse_session::{Session, skill_for};

#[test]
fn skill_for_lists_every_rule_the_engines_registry_enables() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = lighthouse_test_support::lang_rust();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"core\", \"design\", \"testing\"]\nextends = [\"core/recommended\", \"design/recommended\", \"testing/recommended\"]\n",
            plugin.to_str().unwrap()
        )),
    )
    .unwrap();
    let session = Session::find_in(dir.path()).unwrap().unwrap();

    // The registry the engine runs with has the language plugin started.
    let (registry, registered) = session.registry().unwrap();
    assert!(registered.incomplete.is_empty());
    assert!(registry.languages().any(|(_, l)| l.manifest().id == "rust"));
    let enabled = active_rules(
        &session.config,
        &session.catalog().unwrap().projects().unwrap(),
    )
    .unwrap();
    assert!(!enabled.is_empty());

    let skill = skill_for(&session).unwrap();
    let catalog = session.catalog().unwrap();
    for id in &enabled {
        assert!(catalog.decision(id).is_some(), "{id} has no decision");
        assert!(
            skill.contains(&format!("`{id}`")),
            "{id} missing from the skill"
        );
    }
}
