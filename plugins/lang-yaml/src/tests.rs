use lighthouse_protocol::{Source, coalesce_text_map};
use serde_json::{Value, json};

use super::*;

/// The tree of one YAML document, as the host sees it.
fn tree_of(host: &str) -> HostTree {
    tree::parse(host).unwrap()
}

fn document(host: &str) -> Value {
    tree_of(host).ast["documents"][0].clone()
}

#[test]
fn serialized_ast_keeps_decoded_scalar_and_json_pointer_source_map() {
    let host = "where: \"x > 1\"\nlimit: 10\n";
    let tree = tree_of(host);
    let root = &tree.ast["documents"][0];
    assert_eq!(root.pointer("/where").unwrap(), "x > 1");
    let entry = tree.mapping.get("/documents/0/where").unwrap();
    let start = host.find("x > 1").unwrap();
    assert_eq!(entry.range, Some([7, 14]));
    let map = entry.text_map.as_ref().unwrap();
    assert_eq!(map.first().unwrap().source_range[0] as usize, start);
    assert_eq!(
        map.last().unwrap().source_range[1] as usize,
        start + "x > 1".len()
    );
    assert_eq!(root.pointer("/limit").unwrap(), 10);
}

#[test]
fn a_comment_after_a_quoted_scalar_is_not_part_of_it() {
    let host = "model:\n  prompt: \"Rename, do not delete.\"   # optional: added\n  rpc: {}\n";
    let tree = tree_of(host);
    let entry = tree.mapping.get("/documents/0/model/prompt").unwrap();
    let segments = entry.text_map.as_ref().unwrap();
    let end = segments.last().unwrap().source_range[1] as usize;
    assert_eq!(&host[..end], &host[..host.find("\"   #").unwrap()]);
    assert_eq!(
        tree.ast["documents"][0]["model"]["prompt"],
        "Rename, do not delete."
    );
}

#[test]
fn escaped_unicode_keeps_a_non_linear_segment_separate() {
    let text = "雪x";
    let starts = [1, 1, 1, 7];
    let ends = [7, 7, 7, 8];
    let segments = coalesce_text_map(text, &starts, &ends).unwrap();

    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].text_range, [0, 3]);
    assert_eq!(segments[0].source_range, [1, 7]);
    assert_eq!(segments[1].text_range, [3, 4]);
    assert_eq!(segments[1].source_range, [7, 8]);
}

#[test]
fn unicode_before_a_scalar_keeps_every_range_in_utf8_bytes() {
    let host = "label: 雪\nnote: Cross-reference to a minivm document.\n";
    let tree = tree_of(host);
    let entry = tree.mapping.get("/documents/0/note").unwrap();
    let [start, end] = entry.range.unwrap().map(|value| value as usize);
    assert_eq!(&host[start..end], "Cross-reference to a minivm document.");
    let segments = entry.text_map.as_ref().unwrap();
    assert_eq!(segments.first().unwrap().source_range[0] as usize, start);
    assert_eq!(segments.last().unwrap().source_range[1] as usize, end);

    let source = Source {
        path: "unicode.yaml".to_owned(),
        ast: tree.ast,
        mapping: tree.mapping,
    };
    source.validate(host).unwrap();
}

#[test]
fn plain_scalars_resolve_as_the_core_schema_of_yaml_1_2() {
    let root = document(
        "plus: +12\noctal: 0o17\nhex: 0xff\nfloat: 1e3\nnothing: ~\nword: Null\nflag: TRUE\ntext: 0b11\n",
    );
    assert_eq!(root["plus"], 12);
    assert_eq!(root["octal"], 15);
    assert_eq!(root["hex"], 255);
    assert_eq!(root["float"], json!(1000.0));
    assert_eq!(root["nothing"], Value::Null);
    assert_eq!(root["word"], Value::Null);
    assert_eq!(root["flag"], true);
    assert_eq!(root["text"], "0b11");
}

#[test]
fn a_float_json_cannot_hold_stays_its_text() {
    let root = document("big: .inf\nnone: .nan\n");
    assert_eq!(root["big"], ".inf");
    assert_eq!(root["none"], ".nan");
}

#[test]
fn block_scalars_map_each_decoded_byte_to_its_line() {
    let host = "literal: |\n  one\n  two\nfolded: >-\n  three\n  four\nlast: 1\n";
    let tree = tree_of(host);
    let root = &tree.ast["documents"][0];
    assert_eq!(root["literal"], "one\ntwo\n");
    assert_eq!(root["folded"], "three four");
    for (name, text) in [("literal", "one"), ("folded", "three")] {
        let entry = &tree.mapping[&format!("/documents/0/{name}")];
        let first = entry.text_map.as_ref().unwrap()[0];
        let at = first.source_range[0] as usize;
        assert_eq!(&host[at..at + text.len()], text, "{name}");
    }
    let source = Source {
        path: "block.yaml".to_owned(),
        ast: tree.ast.clone(),
        mapping: tree.mapping.clone(),
    };
    source.validate(host).unwrap();
}

#[test]
fn a_quoted_scalar_with_escapes_is_mapped_in_several_segments() {
    let host = "name: \"a\\u00e9b\\n\"\n";
    let tree = tree_of(host);
    assert_eq!(tree.ast["documents"][0]["name"], "aéb\n");
    let segments = tree.mapping["/documents/0/name"].text_map.as_ref().unwrap();
    assert!(segments.len() > 1, "{segments:?}");
    assert_eq!(
        segments.last().unwrap().text_range[1] as usize,
        "aéb\n".len()
    );
}

#[test]
fn an_escape_that_is_cut_short_is_a_gap_and_no_panic() {
    let host = "name: \"a\\u00\"\n";
    assert!(tree::parse(host).is_err());
}

#[test]
fn invalid_documents_and_non_string_keys_are_not_trees() {
    assert!(matches!(
        tree::parse("a: 1\na: 2\n"),
        Err(HostError::Syntax(_))
    ));
    assert!(matches!(
        tree::parse("? [a]\n: 1\n"),
        Err(HostError::Tree(_))
    ));
    assert!(matches!(tree::parse("a: [1\n"), Err(HostError::Syntax(_))));
}
