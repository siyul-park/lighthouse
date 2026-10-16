use lighthouse_protocol::canonical;
use serde_json::json;

#[test]
fn members_are_sorted_and_whitespace_is_dropped() {
    let value = json!({ "b": [1, 2], "a": { "z": true, "y": null }, "\u{e9}": "x" });
    assert_eq!(
        canonical(&value),
        "{\"a\":{\"y\":null,\"z\":true},\"b\":[1,2],\"\u{e9}\":\"x\"}"
    );
}

#[test]
fn numbers_print_as_ecmascript_does() {
    let text = |v: serde_json::Value| canonical(&v);
    assert_eq!(text(json!(1.0)), "1");
    assert_eq!(text(json!(0.7)), "0.7");
    assert_eq!(text(json!(-0.0)), "0");
    assert_eq!(text(json!(1e21)), "1e+21");
    assert_eq!(text(json!(1e-7)), "1e-7");
    assert_eq!(text(json!(123456789u64)), "123456789");
}

#[test]
fn strings_escape_only_what_json_requires() {
    assert_eq!(
        canonical(&json!("a\"b\\c\n\u{1}\u{7f}")),
        "\"a\\\"b\\\\c\\n\\u0001\u{7f}\""
    );
}
