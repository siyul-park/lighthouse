//! Golden conformance of language plugins: every case under
//! `plugins/conformance/<language>/<case>` is a project plus the exact wire
//! output the plugin must produce for it, per option variant. The Go plugin's
//! own tests validate the same results against the JSON Schema. Regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p lighthouse-rpc --test conformance` and read
//! the diff.

mod support;

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use lighthouse_protocol::{self as wire, IndexParams};
use serde_json::{Value, json};
use support::Plugin;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files(dir: &Path, base: &Path, glob_suffix: &str, out: &mut Vec<String>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            files(&path, base, glob_suffix, out);
        } else if path.to_string_lossy().ends_with(glob_suffix) {
            let rel = path.strip_prefix(base).unwrap();
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// `options[.variant].json` next to `expected[.variant].json`; the default
/// variant runs without options when no `options.json` exists.
fn variants(case: &Path) -> Vec<(String, Value)> {
    let mut found = Vec::new();
    for entry in fs::read_dir(case).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("options") else {
            continue;
        };
        let Some(variant) = rest.strip_suffix(".json") else {
            continue;
        };
        let options = fs::read_to_string(case.join(&name)).unwrap();
        found.push((
            variant.trim_start_matches('.').to_owned(),
            serde_json::from_str(&options).unwrap(),
        ));
    }
    if !found.iter().any(|(variant, _)| variant.is_empty()) {
        found.push((String::new(), json!({})));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    text
}

fn index(
    plugin: &mut Plugin,
    root: &Path,
    language: &str,
    options: &Value,
    files: &[String],
) -> Value {
    let params = IndexParams {
        project: wire::ProjectRef {
            root: root.to_string_lossy().into_owned(),
        },
        language: language.to_owned(),
        files: files
            .iter()
            .map(|path| wire::FileRef {
                path: path.clone(),
                hash: "unused".to_owned(),
            })
            .collect(),
        context: wire::Context {
            options: options
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
            overlays: None,
        },
    };
    plugin.call::<Value>(wire::INDEX, params)
}

#[test]
fn go_plugin_matches_the_golden_wire_output() {
    let Some(dir) = lighthouse_testkit::lang_go() else {
        return;
    };
    let cases = workspace().join("plugins/conformance/go");
    let update = env::var_os("UPDATE_GOLDEN").is_some();
    let mut names: Vec<_> = fs::read_dir(&cases)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    names.sort();
    assert!(names.len() >= 12, "conformance cases went missing");
    let mut failures = Vec::new();
    for case in names {
        let root = case.join("project").canonicalize().unwrap();
        let mut sources = Vec::new();
        files(&root, &root, ".go", &mut sources);
        for (variant, options) in variants(&case) {
            let mut plugin = Plugin::start(&dir, &root);
            plugin.initialize(&root);
            let actual = index(&mut plugin, &root, "go", &options, &sources);
            serde_json::from_value::<wire::IndexResult>(actual.clone()).unwrap_or_else(|e| {
                panic!("{} {variant} is not an index result: {e}", case.display())
            });
            assert_eq!(plugin.finish(), Some(0));
            let name = if variant.is_empty() {
                "expected.json".to_owned()
            } else {
                format!("expected.{variant}.json")
            };
            let golden = case.join(&name);
            if update {
                fs::write(&golden, pretty(&actual)).unwrap();
            } else if fs::read_to_string(&golden).ok().as_deref() != Some(&pretty(&actual)) {
                failures.push(golden.display().to_string());
            }
        }
    }
    assert!(
        failures.is_empty(),
        "stale goldens (UPDATE_GOLDEN=1 to regenerate): {failures:#?}"
    );
}

#[test]
fn initialize_declares_the_go_language_and_checks_the_protocol_version() {
    let Some(dir) = lighthouse_testkit::lang_go() else {
        return;
    };
    let root = workspace();
    let mut plugin = Plugin::start(&dir, &root);
    let result = plugin.initialize(&root);
    assert_eq!(result.id, "lang-go");
    assert_eq!(result.protocol_version, wire::VERSION);
    let go = &result.languages[0];
    assert_eq!(go.id, "go");
    assert_eq!(go.globs, ["**/*.go"]);
    assert_eq!(go.conventions.test_globs, ["**/*_test.go"]);
    assert_eq!(go.capabilities, [wire::SEMANTIC_EDGES]);
    assert!(!go.fallback);

    let stale = plugin.request(
        wire::INITIALIZE,
        json!({ "root": "/", "protocolVersion": "9.9", "clientInfo": { "name": "x", "version": "0" } }),
    );
    assert!(stale.error.unwrap().message.contains("protocol"));
    let unknown = plugin.request("nope", json!({}));
    assert_eq!(unknown.error.unwrap().code, -32601);
    assert_eq!(plugin.finish(), Some(0));
}

#[test]
fn bad_options_are_reported_as_incomplete_and_the_plugin_keeps_serving() {
    let Some(dir) = lighthouse_testkit::lang_go() else {
        return;
    };
    let root = workspace()
        .join("plugins/conformance/go/generated/project")
        .canonicalize()
        .unwrap();
    let mut plugin = Plugin::start(&dir, &root);
    plugin.initialize(&root);
    let files = vec!["plain.go".to_owned()];
    let bad = index(
        &mut plugin,
        &root,
        "go",
        &json!({ "go": { "tagz": [] } }),
        &files,
    );
    let reason = bad["incomplete"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("tagz"), "{reason}");
    assert!(bad["fragments"].as_array().unwrap().is_empty());
    let good = index(&mut plugin, &root, "go", &json!({}), &files);
    assert_eq!(good["fragments"][0]["file"]["path"], "plain.go");
    assert_eq!(plugin.finish(), Some(0));
}

#[test]
fn garbage_on_the_wire_is_answered_with_a_parse_error_and_ends_the_plugin() {
    let Some(dir) = lighthouse_testkit::lang_go() else {
        return;
    };
    let root = workspace();
    let mut plugin = Plugin::start(&dir, &root);
    plugin.raw("Content-Length: 2\r\n\r\n{]");
    let reply = plugin.read().unwrap();
    assert_eq!(reply.error.unwrap().code, -32700);
    assert!(plugin.read().is_none());
}

#[test]
fn framing_survives_bodies_larger_than_one_read_and_mixed_case_headers() {
    let Some(dir) = lighthouse_testkit::lang_go() else {
        return;
    };
    let root = workspace();
    let mut plugin = Plugin::start(&dir, &root);
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "root": root, "protocolVersion": wire::VERSION,
                    "clientInfo": { "name": "t", "version": "0" } }
    })
    .to_string();
    plugin.raw(&format!(
        "content-LENGTH:   {}\r\nContent-Type: x\r\n\r\n{init}",
        init.len()
    ));
    assert!(plugin.read().unwrap().error.is_none());
    // Requests and responses of several hundred KB: ignored paths cost the
    // plugin nothing but still produce one fragment each.
    let files: Vec<String> = (0..6000).map(|n| format!("testdata/{n:0>40}.go")).collect();
    let mut options = BTreeMap::new();
    options.insert("other".to_owned(), json!({ "blob": "x".repeat(300_000) }));
    let params = IndexParams {
        project: wire::ProjectRef {
            root: root.to_string_lossy().into_owned(),
        },
        language: "go".to_owned(),
        files: files
            .iter()
            .map(|path| wire::FileRef {
                path: path.clone(),
                hash: "0".to_owned(),
            })
            .collect(),
        context: wire::Context {
            options,
            overlays: None,
        },
    };
    let result: wire::IndexResult = plugin.call(wire::INDEX, params);
    assert_eq!(result.fragments.len(), files.len());
    assert!(serde_json::to_vec(&result).unwrap().len() > 500_000);
    assert_eq!(plugin.finish(), Some(0));
}
