use lighthouse_resource::{Format, documents, to_yaml};
use serde_json::{Value, json};

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
