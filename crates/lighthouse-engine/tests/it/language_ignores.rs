//! `ignores` at two levels: a plugin's own apply to every file, a language's
//! only to the files that language claims.

use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use lighthouse_engine::Engine;
use lighthouse_plugin::{
    Error as PluginError, Indexed, LanguageManifest, LanguageProvider, Plugin, PluginManifest,
    Registry, Unit, Workspace,
};
use lighthouse_spec::Config;

use crate::engine::project;

type Seen = Arc<Mutex<BTreeSet<PathBuf>>>;

/// A language that records the files it was given.
struct Recorder(LanguageManifest, Seen);

impl LanguageProvider for Recorder {
    fn manifest(&self) -> &LanguageManifest {
        &self.0
    }
    fn index(&self, _: &Workspace, units: &[Unit], _: &[String]) -> Result<Indexed, PluginError> {
        self.1.lock().unwrap().extend(
            units
                .iter()
                .flat_map(|unit| unit.files)
                .map(|s| s.file.path.clone()),
        );
        Ok(Indexed::default())
    }
}

struct Lang {
    manifest: PluginManifest,
    id: &'static str,
    glob: &'static str,
    language: Vec<String>,
    plugin: Vec<String>,
    seen: Seen,
}

impl Plugin for Lang {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    fn ignores(&self) -> Vec<String> {
        self.plugin.clone()
    }
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Recorder(
            LanguageManifest {
                ignores: self.language.clone(),
                ..LanguageManifest::new(self.id, vec![self.glob.to_owned()])
            },
            Arc::clone(&self.seen),
        ))]
    }
}

fn lang(id: &'static str, glob: &'static str, language: &[&str], plugin: &[&str]) -> (Lang, Seen) {
    let seen = Seen::default();
    let own = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect();
    let one = Lang {
        manifest: PluginManifest {
            id: id.to_owned(),
            version: "0".to_owned(),
        },
        id,
        glob,
        language: own(language),
        plugin: own(plugin),
        seen: Arc::clone(&seen),
    };
    (one, seen)
}

#[test]
fn a_language_leaves_its_ignored_files_to_the_others_and_a_plugin_ignores_for_all() {
    let dir = project(&[
        ("x.go", b"1"),
        ("_old/x.go", b"1"),
        (".github/w.go", b"1"),
        ("__tests__/a.test.ts", b"1"),
        ("src/a.ts", b"1"),
        ("vendor/README.md", b"1"),
        ("vendor/v.go", b"1"),
        ("README.md", b"1"),
    ]);
    let (go, go_seen) = lang("lang-go", "**/*.go", &["_*/", ".*/"], &["vendor/"]);
    let (ts, ts_seen) = lang("lang-ts", "**/*.ts", &[], &[]);
    let (md, md_seen) = lang("lang-md", "**/*.md", &[], &[]);
    let mut registry = Registry::default();
    for plugin in [&go, &ts, &md] {
        registry.register(plugin).unwrap();
    }
    let config =
        Config::parse_inline("plugins = [\"lang-go\", \"lang-ts\", \"lang-md\"]\n").unwrap();
    Engine::new(
        registry,
        config,
        lighthouse_test_support::bundled(),
        dir.path(),
    )
    .unwrap()
    .check(&[], &[])
    .unwrap();

    let names = |seen: &Seen| -> Vec<String> {
        seen.lock()
            .unwrap()
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    };
    assert_eq!(names(&go_seen), ["x.go"], "_old and .github are not Go's");
    assert_eq!(
        names(&ts_seen),
        ["__tests__/a.test.ts", "src/a.ts"],
        "the Go convention is not TypeScript's"
    );
    assert_eq!(
        names(&md_seen),
        ["README.md"],
        "vendor/ is ignored for every language"
    );
}
