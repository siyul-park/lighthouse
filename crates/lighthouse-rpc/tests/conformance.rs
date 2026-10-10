//! Golden conformance of language plugins: every case under
//! `plugins/conformance/<language>/<case>` is a project plus the exact wire
//! output the plugin must produce for it, per option variant. The Go plugin's
//! own tests validate the same results against the JSON Schema. Regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p lighthouse-rpc --test conformance` and read
//! the diff.
//!
//! One runner serves every language: a [`Lang`] says where the plugin comes
//! from, which files a case holds and how options are keyed.

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

struct Lang {
    id: &'static str,
    ext: &'static str,
    min_cases: usize,
    plugin: fn() -> Option<PathBuf>,
    /// A file of `generated/project` and an options table the plugin rejects.
    probe: (&'static str, fn() -> Value),
    manifest_id: &'static str,
    globs: &'static [&'static str],
    test_globs: &'static [&'static str],
    capabilities: &'static [&'static str],
}

const GO: Lang = Lang {
    id: "go",
    ext: ".go",
    min_cases: 15,
    plugin: lighthouse_test_support::lang_go,
    probe: ("plain.go", || json!({ "go": { "tagz": [] } })),
    manifest_id: "lang-go",
    globs: &["**/*.go"],
    test_globs: &["**/*_test.go"],
    capabilities: &[
        wire::SEMANTIC_EDGES,
        wire::EXTENT,
        wire::REFERENCE_SITES,
        wire::OVERLAYS,
    ],
};

const RUST: Lang = Lang {
    id: "rust",
    ext: ".rs",
    min_cases: 14,
    plugin: || Some(lighthouse_test_support::lang_rust()),
    probe: ("src/plain.rs", || json!({ "rust": { "nope": 1 } })),
    manifest_id: "lang-rust",
    globs: &["**/*.rs"],
    test_globs: &["**/tests/**/*.rs", "**/tests.rs", "**/benches/**/*.rs"],
    capabilities: &[wire::EXTENT, wire::REFERENCE_SITES, wire::OVERLAYS],
};

const LANGS: [&Lang; 2] = [&GO, &RUST];

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
    index_overlaid(plugin, root, language, options, files, None)
}

fn index_overlaid(
    plugin: &mut Plugin,
    root: &Path,
    language: &str,
    options: &Value,
    files: &[String],
    overlays: Option<Vec<wire::Overlay>>,
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
            overlays,
        },
    };
    plugin.call::<Value>(wire::INDEX, params)
}

fn golden(lang: &Lang) {
    let Some(dir) = (lang.plugin)() else {
        return;
    };
    let cases = workspace().join("plugins/conformance").join(lang.id);
    let update = env::var_os("UPDATE_GOLDEN").is_some();
    let mut names: Vec<_> = fs::read_dir(&cases)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    names.sort();
    assert!(
        names.len() >= lang.min_cases,
        "conformance cases went missing"
    );
    let mut failures = Vec::new();
    for case in names {
        let root = case.join("project").canonicalize().unwrap();
        let mut sources = Vec::new();
        files(&root, &root, lang.ext, &mut sources);
        for (variant, options) in variants(&case) {
            let mut plugin = Plugin::start(&dir, &root);
            plugin.initialize(&root);
            let actual = index(&mut plugin, &root, lang.id, &options, &sources);
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
fn go_plugin_matches_the_golden_wire_output() {
    golden(&GO);
}

#[test]
fn rust_plugin_matches_the_golden_wire_output() {
    golden(&RUST);
}

#[test]
fn initialize_declares_the_language_and_checks_the_protocol_version() {
    for lang in LANGS {
        let Some(dir) = (lang.plugin)() else {
            continue;
        };
        let root = workspace();
        let mut plugin = Plugin::start(&dir, &root);
        let result = plugin.initialize(&root);
        assert_eq!(result.id, lang.manifest_id);
        assert_eq!(result.protocol_version, wire::VERSION);
        let declared = &result.languages[0];
        assert_eq!(declared.id, lang.id);
        assert_eq!(declared.globs, lang.globs);
        assert_eq!(declared.conventions.test_globs, lang.test_globs);
        assert_eq!(declared.capabilities, lang.capabilities);
        assert!(!declared.fallback);

        let stale = plugin.request(
            wire::INITIALIZE,
            json!({ "root": "/", "protocolVersion": "9.9", "clientInfo": { "name": "x", "version": "0" } }),
        );
        assert!(stale.error.unwrap().message.contains("protocol"));
        let unknown = plugin.request("nope", json!({}));
        assert_eq!(unknown.error.unwrap().code, -32601);
        assert_eq!(plugin.finish(), Some(0));
    }
}

#[test]
fn bad_options_are_reported_as_incomplete_and_the_plugin_keeps_serving() {
    for lang in LANGS {
        let Some(dir) = (lang.plugin)() else {
            continue;
        };
        let root = workspace()
            .join("plugins/conformance")
            .join(lang.id)
            .join("generated/project")
            .canonicalize()
            .unwrap();
        let mut plugin = Plugin::start(&dir, &root);
        plugin.initialize(&root);
        let (file, bad_options) = lang.probe;
        let files = vec![file.to_owned()];
        let bad = index(&mut plugin, &root, lang.id, &bad_options(), &files);
        let reason = bad["incomplete"][0]["reason"].as_str().unwrap();
        let key = bad_options()[lang.id]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        assert!(reason.contains(&key), "{reason}");
        assert!(bad["fragments"].as_array().unwrap().is_empty());
        let good = index(&mut plugin, &root, lang.id, &json!({}), &files);
        assert_eq!(good["fragments"][0]["file"]["path"], file);
        assert_eq!(plugin.finish(), Some(0));
    }
}

#[test]
fn garbage_on_the_wire_is_answered_with_a_parse_error_and_ends_the_plugin() {
    for lang in LANGS {
        let Some(dir) = (lang.plugin)() else {
            continue;
        };
        let root = workspace();
        let mut plugin = Plugin::start(&dir, &root);
        plugin.raw("Content-Length: 2\r\n\r\n{]");
        let reply = plugin.read().unwrap();
        assert_eq!(reply.error.unwrap().code, -32700);
        assert!(plugin.read().is_none());
    }
}

#[test]
fn framing_survives_bodies_larger_than_one_read_and_mixed_case_headers() {
    for lang in LANGS {
        let Some(dir) = (lang.plugin)() else {
            continue;
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
        let files: Vec<String> = (0..6000)
            .map(|n| format!("testdata/{n:0>40}{}", lang.ext))
            .collect();
        let mut options = BTreeMap::new();
        options.insert("other".to_owned(), json!({ "blob": "x".repeat(300_000) }));
        let params = IndexParams {
            project: wire::ProjectRef {
                root: root.to_string_lossy().into_owned(),
            },
            language: lang.id.to_owned(),
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
}

/// An overlay stands in for the file: the provider indexes its text, never the
/// disk, and the file on disk is left as it was.
#[test]
fn overlays_replace_the_text_a_provider_reads() {
    for (lang, file, added, symbol) in [
        (
            &GO,
            "extents.go",
            "\nfunc Added() int { return 1 }\n",
            "::Added#function",
        ),
        (
            &RUST,
            "src/lib.rs",
            "\npub fn added() -> u8 {\n    1\n}\n",
            "::added#function",
        ),
    ] {
        let Some(dir) = (lang.plugin)() else {
            continue;
        };
        let root = workspace()
            .join("plugins/conformance")
            .join(lang.id)
            .join("extents/project")
            .canonicalize()
            .unwrap();
        let on_disk = fs::read_to_string(root.join(file)).unwrap();
        let mut sources = Vec::new();
        files(&root, &root, lang.ext, &mut sources);
        let ids = |result: &Value| -> Vec<String> {
            result["fragments"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|f| f["symbols"].as_array().unwrap().iter())
                .map(|s| s["id"].as_str().unwrap().to_owned())
                .collect()
        };
        let mut plugin = Plugin::start(&dir, &root);
        plugin.initialize(&root);

        let plain = index(&mut plugin, &root, lang.id, &json!({}), &sources);
        let overlaid = index_overlaid(
            &mut plugin,
            &root,
            lang.id,
            &json!({}),
            &sources,
            Some(vec![wire::Overlay {
                path: file.to_owned(),
                text: format!("{on_disk}{added}"),
            }]),
        );

        assert!(
            !ids(&plain).iter().any(|id| id.ends_with(symbol)),
            "{}",
            lang.id
        );
        assert!(
            ids(&overlaid).iter().any(|id| id.ends_with(symbol)),
            "{}",
            lang.id
        );
        assert!(
            overlaid["incomplete"].as_array().unwrap().is_empty(),
            "{overlaid}"
        );
        assert_eq!(fs::read_to_string(root.join(file)).unwrap(), on_disk);
        assert_eq!(plugin.finish(), Some(0));
    }
}
