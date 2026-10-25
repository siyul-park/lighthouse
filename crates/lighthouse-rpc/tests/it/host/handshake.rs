//! The handshake and the registration of a plugin that is a script.

use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use lighthouse_plugin::{Plugin, Registry, Workspace};
use lighthouse_rpc::register;
use serde_json::{Value, json};

use super::{
    config,
    fake::{Fake, T, index},
    manifest,
};

#[test]
fn a_manifest_that_provides_the_languages_the_process_reports_is_accepted() {
    let fake = Fake::new("ok");
    let manifest = fake.path().join("lighthouse-plugin.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        format!("{text}[spec.provides]\nlanguages = [\"fake\"]\n"),
    )
    .unwrap();

    assert!(fake.connect(T).is_ok());
}

#[test]
fn a_manifest_that_provides_other_languages_than_the_process_reports_is_refused() {
    let fake = Fake::new("ok");
    let manifest = fake.path().join("lighthouse-plugin.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        format!("{text}[spec.provides]\nlanguages = [\"other\"]\n"),
    )
    .unwrap();

    let error = fake.connect(T).err().unwrap();

    assert!(
        error
            .to_string()
            .contains("provides languages [other], the process reports [fake]"),
        "{error}"
    );
}

#[test]
fn handshake_declares_languages_and_index_converts_the_result() {
    let fake = Fake::new("ok");
    let plugin = fake.connect(T).unwrap();
    let manifest = plugin.manifest();
    assert_eq!(
        (manifest.id.as_str(), manifest.version.as_str()),
        ("fake", "1.2")
    );
    let provider = plugin.languages().remove(0);
    assert_eq!(provider.manifest().id, "fake");
    assert_eq!(provider.manifest().globs, ["**/*.fake"]);
    assert_eq!(
        provider.manifest().capabilities,
        [lighthouse_model::ucm::Capability::SemanticEdges],
        "unknown capabilities are ignored"
    );
    let mut ws = Workspace::new(fake.path());
    ws.languages.insert(
        "fake".to_owned(),
        serde_json::from_value(json!({ "tags": ["x"] })).unwrap(),
    );
    let indexed = index(provider.as_ref(), &ws, &["a.fake", "b.fake"]);
    assert_eq!(indexed.fragments.len(), 1);
    let kept = &indexed.fragments[0].files[0];
    assert_eq!(kept.path, Path::new("a.fake"));
    assert!(kept.generated);
    assert_eq!(kept.hash, "abc", "the engine's file data is kept");
    assert_eq!(indexed.fragments[0].modules[0].path, "m");
    assert_eq!(indexed.notices, ["from plugin"]);
    assert!(indexed.incomplete.iter().any(|i| i.reason == "half"));
    assert!(
        indexed
            .incomplete
            .iter()
            .any(|i| i.path.as_deref() == Some(Path::new("b.fake"))
                && i.reason.contains("no usable fragment"))
    );

    let request: Value =
        serde_json::from_str(&fs::read_to_string(fake.path().join("log.request")).unwrap())
            .unwrap();
    let params = &request["params"];
    assert_eq!(params["language"], "fake");
    assert_eq!(
        params["project"]["root"],
        fake.path().to_string_lossy().as_ref()
    );
    assert_eq!(
        params["inputs"][0],
        json!({ "kind": "file", "file": { "path": "a.fake", "hash": "abc" } })
    );
    assert_eq!(params["context"]["options"]["fake"]["tags"][0], "x");
    assert!(params["context"].get("overlays").is_none());
}

#[test]
fn dropping_the_plugin_sends_shutdown_then_exit() {
    let fake = Fake::new("ok");
    drop(fake.provider(T));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fake.log().contains("exit") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fake.log(), "shutdown\nexit\n");
}

#[test]
fn handshake_refuses_other_protocol_versions_and_identities() {
    let message = |mode: &str| match Fake::new(mode).connect(T) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("{mode} connected"),
    };
    assert!(message("version").contains("speaks protocol 9.9, lighthouse speaks 0.4"));
    assert!(message("identity").contains("identifies itself as `other`"));
    let early = message("crash_at_init");
    assert!(early.contains("plugin `fake`"), "{early}");
}

#[test]
fn an_explicit_path_disambiguates_and_relative_commands_resolve_next_to_the_manifest() {
    let search = tempfile::tempdir().unwrap();
    manifest(&search.path().join("a"), "fake", "/does/not/exist");
    manifest(&search.path().join("b"), "fake", "/does/not/exist");
    let fake = Fake::new("ok");
    let project = tempfile::tempdir().unwrap();
    let toml = format!(
        "[{{ id = \"fake\", path = {:?}, timeout = \"30s\" }}]",
        fake.path().to_str().unwrap()
    );
    let mut registry = Registry::default();
    register(
        &mut registry,
        &config(&toml),
        project.path(),
        project.path(),
        &[search.path().to_owned()],
    )
    .unwrap();
    assert!(registry.has_plugin("fake"));
    assert_eq!(registry.languages().len(), 1);
}

#[test]
fn the_cache_directory_is_named_by_the_plugin_binary_and_absent_with_overlays() {
    let fake = Fake::new("ok");
    let provider = fake.provider(T);
    let cache = tempfile::tempdir().unwrap();
    let mut ws = Workspace::new(fake.path());
    ws.cache_dir = Some(cache.path().to_owned());
    let sent = || -> Value {
        let text = fs::read_to_string(fake.path().join("log.request")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["params"]["context"].clone()
    };

    index(provider.as_ref(), &ws, &["a.fake"]);

    let digest =
        lighthouse_protocol::hash::sha256(fs::read(fake.path().join("plugin.sh")).unwrap());
    let expected = cache.path().join("fake").join(&digest[..16]);
    assert_eq!(sent()["cache"]["dir"], expected.to_string_lossy().as_ref());
    assert!(expected.is_dir());

    ws.overlays.insert("a.fake".into(), "edited".to_owned());
    index(provider.as_ref(), &ws, &["a.fake"]);

    assert!(sent().get("cache").is_none(), "{}", sent());
    assert_eq!(sent()["overlays"][0]["path"], "a.fake");
}
