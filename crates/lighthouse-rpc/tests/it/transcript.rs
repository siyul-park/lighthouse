//! Recorded exchanges of the protocol: every file of `protocol/conformance` is
//! an ordered list of messages to send and the reply each must get, and every
//! plugin of its kind replays it. The Go sdk replays the same files
//! (`plugins/lang-go/sdk`), so the two servers cannot drift.
//!
//! `"$any"` in an expected reply matches any value and `"$string"` any string.

use std::{fs, path::Path};

use lighthouse_protocol::Message;
use serde::Deserialize;
use serde_json::Value;

use crate::support::Plugin;

#[derive(Deserialize)]
struct Transcript {
    applies: Vec<Kind>,
    exchanges: Vec<Exchange>,
}

#[derive(Deserialize)]
struct Exchange {
    send: Message,
    expect: Option<Message>,
}

#[derive(Deserialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Language,
    Model,
}

/// The plugins to replay against, with what each offers.
fn plugins() -> Vec<(&'static str, Kind, std::path::PathBuf)> {
    let go = lighthouse_test_support::lang_go();
    [
        ("lang-go", Kind::Language, go),
        (
            "lang-rust",
            Kind::Language,
            Some(lighthouse_test_support::lang_rust()),
        ),
        (
            "lang-typescript",
            Kind::Language,
            lighthouse_test_support::lang_typescript(),
        ),
        (
            "lang-python",
            Kind::Language,
            lighthouse_test_support::lang_python(),
        ),
        (
            "fake-model",
            Kind::Model,
            Some(lighthouse_test_support::fake_model()),
        ),
        (
            "model-local",
            Kind::Model,
            Some(lighthouse_test_support::model_local()),
        ),
        (
            "model-http",
            Kind::Model,
            Some(lighthouse_test_support::model_http()),
        ),
    ]
    .into_iter()
    .filter_map(|(name, kind, dir)| dir.map(|dir| (name, kind, dir)))
    .collect()
}

fn transcripts() -> Vec<(String, Transcript)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/conformance");
    let mut found: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    found.sort();
    found
        .into_iter()
        .map(|path| {
            let text = fs::read_to_string(&path).unwrap();
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            let transcript = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{} is not a transcript: {e}", path.display()));
            (name, transcript)
        })
        .collect()
}

fn matches(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::String(token), _) if token == "$any" => true,
        (Value::String(token), Value::String(_)) if token == "$string" => true,
        (Value::Object(e), Value::Object(a)) => {
            e.len() == a.len()
                && e.iter()
                    .all(|(k, v)| a.get(k).is_some_and(|x| matches(v, x)))
        }
        (Value::Array(e), Value::Array(a)) => {
            e.len() == a.len() && e.iter().zip(a).all(|(e, a)| matches(e, a))
        }
        _ => expected == actual,
    }
}

fn replay(plugin: &mut Plugin, name: &str, exchanges: &[Exchange]) {
    for exchange in exchanges {
        plugin.send(&exchange.send);
        let Some(expect) = &exchange.expect else {
            continue;
        };
        let reply = plugin.read().unwrap_or_else(|| panic!("{name}: no reply"));
        let (expect, reply) = (
            serde_json::to_value(expect).unwrap(),
            serde_json::to_value(reply).unwrap(),
        );
        assert!(
            matches(&expect, &reply),
            "{name}: want {expect}, got {reply}"
        );
    }
}

#[test]
fn every_plugin_replays_the_recorded_transcripts_of_its_kind() {
    let transcripts = transcripts();
    assert!(
        transcripts.len() >= 5,
        "conformance transcripts went missing"
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (plugin_name, kind, dir) in plugins() {
        for (name, transcript) in &transcripts {
            if !transcript.applies.contains(&kind) {
                continue;
            }
            let label = format!("{plugin_name} {name}");
            let mut plugin = Plugin::start(&dir, &root);
            replay(&mut plugin, &label, &transcript.exchanges);
            let exits = transcript
                .exchanges
                .last()
                .is_some_and(|last| last.send.method.as_deref() == Some("exit"));
            let code = plugin.close();
            if exits {
                assert_eq!(code, Some(0), "{label}: exit code");
            }
        }
    }
}

#[test]
fn the_matcher_reads_wildcards() {
    let expected = serde_json::json!({ "a": "$string", "b": ["$any"], "c": 1 });

    assert!(matches(
        &expected,
        &serde_json::json!({ "a": "x", "b": [null], "c": 1 })
    ));
    assert!(!matches(
        &expected,
        &serde_json::json!({ "a": 2, "b": [null], "c": 1 })
    ));
    assert!(!matches(
        &expected,
        &serde_json::json!({ "a": "x", "b": [], "c": 1 })
    ));
    assert!(!matches(
        &expected,
        &serde_json::json!({ "a": "x", "b": [1], "c": 1, "d": 0 })
    ));
}
