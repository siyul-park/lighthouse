use std::path::Path;

use lighthouse_model::{Format, documents, kind_of, resource, to_yaml};
use serde_json::{Value, json};

use crate::resource::{JSON, TOML, Toy, YAML, toy};

#[test]
fn one_resource_reads_from_yaml_toml_and_json() {
    for (format, text) in [
        (Format::Yaml, YAML),
        (Format::Toml, TOML),
        (Format::Json, JSON),
    ] {
        let docs = documents(format, "toy", text).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(kind_of(&docs[0]), Some("ToyKind"));
        assert_eq!(resource::<Toy>("toy", &docs[0]).unwrap(), toy());
    }
}

#[test]
fn yaml_may_hold_several_documents_and_skips_empty_ones() {
    let text = format!("# only a comment\n---\n{YAML}---\n---\n{YAML}");
    assert_eq!(documents(Format::Yaml, "toy", &text).unwrap().len(), 2);
}

#[test]
fn written_yaml_reads_back() {
    let text = to_yaml(&serde_json::to_value(toy()).unwrap());
    assert_eq!(text, YAML);
}

#[test]
fn the_format_follows_the_extension() {
    assert_eq!(Format::of_path(Path::new("a/b.yaml")), Some(Format::Yaml));
    assert_eq!(Format::of_path(Path::new("b.yml")), Some(Format::Yaml));
    assert_eq!(
        Format::of_path(Path::new("lighthouse.toml")),
        Some(Format::Toml)
    );
    assert_eq!(Format::of_path(Path::new("x.json")), Some(Format::Json));
    assert_eq!(Format::of_path(Path::new("x.md")), None);
    assert_eq!(Format::of_path(Path::new("Makefile")), None);
}

fn roundtrip(value: &Value) -> Value {
    let text = to_yaml(value);
    let docs = documents(Format::Yaml, "test", &text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    assert_eq!(docs.len(), 1, "{text}");
    docs.into_iter().next().unwrap()
}

#[test]
fn awkward_scalars_read_back_unchanged() {
    let value = json!({
        "words": ["true", "no", "null", "~", "1", "1.5", ".inf", "a: b", "# c", "- d", "", " lead", "trail ", "é", "x # y"],
        "key with space": "plain value",
        "lighthouse/pack": "core",
        "n": 1, "f": 1.5, "b": false, "z": null,
        "empty": [], "empty-map": {},
    });
    assert_eq!(roundtrip(&value), value);
}

#[test]
fn long_sentences_fold_and_multi_line_text_is_literal() {
    let sentence = "A file SHOULD stay at or below the configured line limit and a symbol MUST name one concept only, never two at once.";
    let value = json!({
        "spec": {
            "requirement": sentence,
            "body": "package x\n\nfunc f() {}",
            "kept": "line\n",
            "list": [{"a": "one", "b": ["x", "y"]}, {"a": "two", "nested": {"k": sentence}}],
            "table": [["a", "b"], "c"],
        }
    });
    let text = to_yaml(&value);
    assert!(text.contains("requirement: >-\n"), "{text}");
    assert!(text.contains("body: |-\n"), "{text}");
    assert!(text.lines().all(|l| l.chars().count() <= 90), "{text}");
    assert_eq!(roundtrip(&value), value);
}

#[test]
fn awkward_multi_line_text_falls_back_to_quotes() {
    let value = json!({"a": " starts with a space\nsecond", "b": "tab\there", "c": "two\n\n", "d": "a  double space that is also quite long indeed, long enough to want folding across lines"});
    assert_eq!(roundtrip(&value), value);
}
