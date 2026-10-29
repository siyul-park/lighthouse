//! The check of a selected source region against its host text.

use lighthouse_protocol::{EmbeddedSource, TextMapSegment};
use serde_json::Value;

fn region(text: &str, range: [u32; 2], map: Option<Vec<TextMapSegment>>) -> EmbeddedSource {
    EmbeddedSource {
        id: "r".to_owned(),
        rule_id: "rule".to_owned(),
        node_pointer: "/a".to_owned(),
        source_pointer: "/a".to_owned(),
        provider: "cel".to_owned(),
        syntax: None,
        context: Value::Null,
        text: text.to_owned(),
        source_range: range,
        text_map: map,
    }
}

fn segment(text: [u32; 2], source: [u32; 2]) -> TextMapSegment {
    TextMapSegment {
        text_range: text,
        source_range: source,
    }
}

#[test]
fn a_region_inside_the_host_is_valid() {
    let host = "a: 'x > 1'\n";
    assert!(region("x > 1", [4, 9], None).validate(host).is_ok());
    let mapped = region("x > 1", [4, 9], Some(vec![segment([0, 5], [4, 9])]));
    assert!(mapped.validate(host).is_ok());
}

#[test]
fn a_range_outside_the_host_or_inside_a_character_is_not() {
    let host = "a: '雪'\n";
    assert!(region("x", [4, 99], None).validate(host).is_err());
    assert!(region("x", [5, 6], None).validate(host).is_err());
    assert!(region("x", [6, 4], None).validate(host).is_err());
}

#[test]
fn a_text_map_must_cover_the_text_in_order_inside_the_range() {
    let host = "a: 'x > 1'\n";
    let gap = region("x > 1", [4, 9], Some(vec![segment([0, 2], [4, 6])]));
    assert!(gap.validate(host).is_err());
    let outside = region("x > 1", [4, 9], Some(vec![segment([0, 5], [3, 9])]));
    assert!(outside.validate(host).is_err());
    let backwards = region(
        "x > 1",
        [4, 9],
        Some(vec![segment([0, 2], [6, 8]), segment([2, 5], [4, 7])]),
    );
    assert!(backwards.validate(host).is_err());
}
