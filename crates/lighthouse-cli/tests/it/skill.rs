//! The skill digest comes from the catalog, never from a started plugin: every
//! rule the engine's registry enables must already be in it.

use std::{env, ffi::OsString, fs, path::Path};

use lighthouse_engine::active;
use lighthouse_session::{Session, skill_for};

/// The user-level directories a session reads (the user's file, trust and
/// plugins), moved into `dir` while the guard lives, so that the test never
/// reads the real ones.
struct UserHome(Vec<(&'static str, Option<OsString>)>);

impl UserHome {
    const NAMES: [&'static str; 2] = ["XDG_CONFIG_HOME", "XDG_DATA_HOME"];

    fn at(dir: &Path) -> Self {
        let saved = Self::NAMES.map(|name| (name, env::var_os(name)));
        for name in Self::NAMES {
            // SAFETY: the other tests of this binary give the programs they
            // start user-level directories of their own.
            unsafe { env::set_var(name, dir) };
        }
        Self(saved.into())
    }
}

impl Drop for UserHome {
    fn drop(&mut self) {
        for (name, value) in &self.0 {
            // SAFETY: as in `at`.
            match value {
                Some(value) => unsafe { env::set_var(name, value) },
                None => unsafe { env::remove_var(name) },
            }
        }
    }
}

#[test]
fn skill_for_lists_every_rule_the_engines_registry_enables() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".home");
    lighthouse_test_support::installed(&home, &lighthouse_test_support::lang_rust());
    let _user = UserHome::at(&home);
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_test_support::project(
            "plugins = [\"lang-rust\", \"core\", \"design\", \"testing\"]\nextends = [\"core/recommended\", \"design/recommended\", \"testing/recommended\"]\n",
        ),
    )
    .unwrap();
    let session = Session::find_in(dir.path()).unwrap().unwrap();

    // The registry the engine runs with has the language plugin started.
    let (registry, registered) = session.registry().unwrap();
    assert!(registered.incomplete.is_empty());
    assert!(
        registry
            .languages()
            .iter()
            .any(|l| l.provider.manifest().id == "rust")
    );
    let catalog = &session.catalog;
    let enabled = active(&session.config, catalog.projects()).unwrap();
    assert!(!enabled.is_empty());

    let skill = skill_for(&session).unwrap();
    for id in &enabled {
        assert!(catalog.decision(id).is_some(), "{id} has no decision");
        assert!(
            skill.contains(&format!("`{id}`")),
            "{id} missing from the skill"
        );
    }
}
