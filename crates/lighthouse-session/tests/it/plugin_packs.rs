//! A pack comes from the plugin that provides it: `extends` enables it, the
//! plugin is listed once and only when it is external, and a plugin the
//! project brings is trusted together with its catalog.

use std::path::Path;

use lighthouse_session::{Session, baseline};
use tempfile::TempDir;

use crate::fixture::{in_dir, isolate, session, trusted, write};

const MANIFEST: &str = "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Plugin\"\n[metadata]\nname = \"lang-x\"\n[spec]\nversion = \"1\"\ncatalog = \"decisions\"\n[spec.runtime]\ncommand = \"./run\"\n";

const PACK: &str = "apiVersion: lighthouse/v1alpha1\nkind: Pack\nmetadata:\n  name: x\nspec:\n  title: X\n  intro: x\n  sections:\n    - name: s\n      title: S\n      intro: x\n      decisions: [a]\n";

const DECISION: &str = "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: x/a\nspec:\n  title: A\n  context: i\n  scope: { subject: file }\n  requirement: A MUST b.\n  severity: warn\n  check:\n    type: cel\n    select: file\n    where: \"false\"\n    message: m\n    evidence:\n      path: file.path\n  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{ path: a.txt, body: \"a\\n\" }]\n      expect: [{ line: 1 }]\n    - name: good\n      language: text\n      kind: valid\n      files: [{ path: a.txt, body: \"a\\n\" }]\n";

/// A project that lists `lang-x`, a plugin of its own with the pack `x`.
fn with_pack(spec: &str) -> TempDir {
    isolate();
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(spec),
    );
    write(
        dir.path(),
        "plugins/lang-x/lighthouse-plugin.toml",
        MANIFEST,
    );
    write(dir.path(), "plugins/lang-x/decisions/x/pack.yaml", PACK);
    write(dir.path(), "plugins/lang-x/decisions/x/s/a.yaml", DECISION);
    dir
}

const LISTED: &str =
    "plugins = [{ id = \"lang-x\", path = \"plugins/lang-x\" }]\nextends = [\"x/recommended\"]\n";

fn load(dir: &Path) -> lighthouse_session::Result<Session> {
    Session::load_in(dir, None)
}

#[test]
fn a_listed_plugin_ships_its_packs_to_a_trusted_project() {
    let dir = with_pack(LISTED);

    let session = trusted(dir.path());

    assert!(session.catalog.decision("x/a").is_some());
    assert!(session.catalog.projects().get("x/recommended").is_some());
    assert!(
        lighthouse_session::active_decisions(&session)
            .unwrap()
            .contains("x/a")
    );
}

#[test]
fn an_untrusted_project_loads_neither_the_plugin_nor_its_catalog() {
    let dir = with_pack(LISTED);

    let session = session(dir.path());

    assert!(session.catalog.decision("x/a").is_none());
    let why = session.untrusted().expect("the plugin would run a program");
    assert!(
        why.contains("lang-x") && why.contains("lighthouse trust"),
        "{why}"
    );
    let error = lighthouse_session::active_decisions(&session)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("pack `x` comes from plugin `lang-x`") && error.contains("not trusted"),
        "{error}"
    );
}

#[test]
fn a_pack_whose_plugin_is_not_listed_names_the_plugin_to_list() {
    let dir = with_pack("extends = [\"x/recommended\"]\n");
    let session = session(dir.path());

    let error = lighthouse_session::active_decisions(&session)
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("`x/recommended`: pack `x` comes from plugin `lang-x`; add it to `plugins`"),
        "{error}"
    );
    assert!(session.untrusted().is_none());
}

#[test]
fn a_bundled_plugin_is_not_listed() {
    for id in ["core", "design", "testing", "lang-cel", "local"] {
        let dir = with_pack(&format!("plugins = [\"{id}\"]\n"));

        let error = in_dir(dir.path(), || load(dir.path())).err().unwrap();

        assert!(error.to_string().contains(&format!("`{id}`")), "{error}");
        assert!(
            error.to_string().contains("remove it from `plugins`"),
            "{error}"
        );
    }
}

#[test]
fn spec_validate_reports_a_bundled_plugin_and_a_pack_without_its_plugin() {
    let dir = with_pack(LISTED);
    let bundled = dir.path().join("bundled.toml");
    let missing = dir.path().join("missing.toml");
    write(
        dir.path(),
        "bundled.toml",
        &lighthouse_test_support::project("plugins = [\"core\"]\n"),
    );
    write(
        dir.path(),
        "missing.toml",
        &lighthouse_test_support::project("extends = [\"x/recommended\"]\n"),
    );
    let session = trusted(dir.path());

    let found = lighthouse_session::validate_paths(&session, &[bundled, missing], false).unwrap();

    let messages: Vec<&str> = found.problems.iter().map(|p| p.message.as_str()).collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("plugin `core` is bundled: remove it from `plugins`")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("pack `x` comes from plugin `lang-x`; add it to `plugins`")),
        "{messages:?}"
    );
}

#[test]
fn the_docs_cover_the_packs_of_the_plugins_of_the_repository() {
    let dir = with_pack(LISTED);

    let docs = lighthouse_session::pack_docs(dir.path()).unwrap();

    assert!(docs.contains_key("decisions/x.md"), "{:?}", docs.keys());
    assert!(docs.contains_key("decisions/core.md"));
}

#[test]
fn a_plugin_is_added_to_the_listed_ones_of_a_project_file() {
    let none = lighthouse_test_support::project("extends = [\"core/recommended\"]\n");
    let some = lighthouse_test_support::project("plugins = [\"a\"]\n");

    let added = lighthouse_test_support::listing(&none, "{ id = \"b\", path = \"p\" }");
    let appended = lighthouse_test_support::listing(&some, "\"b\"");

    assert!(
        added.contains("\"id\" = \"b\"") && added.contains("core/recommended"),
        "{added}"
    );
    assert!(
        appended.contains("\"a\"") && appended.contains("\"b\""),
        "{appended}"
    );
}

#[test]
fn the_project_is_told_a_notice_once_per_session() {
    let dir = tempfile::tempdir().unwrap();

    assert!(baseline::first_telling(dir.path(), "s1", "untrusted").unwrap());
    assert!(!baseline::first_telling(dir.path(), "s1", "untrusted").unwrap());
    assert!(baseline::first_telling(dir.path(), "s1", "other").unwrap());
    assert!(baseline::first_telling(dir.path(), "s2", "untrusted").unwrap());
}

#[test]
fn only_a_project_that_would_run_a_program_waits_for_trust() {
    let plain = crate::fixture::project("error");
    assert!(session(plain.path()).untrusted().is_none());

    let open = crate::fixture::project_with("error", "[sandbox]\nenabled = false\n");
    let waiting = session(open.path());
    assert!(
        waiting
            .untrusted()
            .is_some_and(|why| why.contains("sandbox"))
    );
    assert!(trusted(open.path()).untrusted().is_none());
}
