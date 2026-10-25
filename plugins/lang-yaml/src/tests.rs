use super::*;

#[test]
fn serialized_ast_keeps_decoded_scalar_and_json_pointer_source_map() {
    let host = "where: \"x > 1\"\nlimit: 10\n";
    let documents = MarkedYaml::load_from_str(host).unwrap();
    let byte_offsets = char_byte_offsets(host);
    let mut source_map = BTreeMap::new();
    let root = yaml_value(
        &documents[0],
        "/documents/0",
        host,
        &byte_offsets,
        &mut source_map,
    )
    .unwrap();
    let expression = root.pointer("/where").unwrap();
    assert_eq!(expression, "x > 1");
    let pointer = "/documents/0/where";
    let entry = source_map.get(pointer).unwrap();
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
    let documents = MarkedYaml::load_from_str(host).unwrap();
    let byte_offsets = char_byte_offsets(host);
    let mut mapping = BTreeMap::from([
        (String::new(), range_entry(0, host.len())),
        ("/documents".to_owned(), range_entry(0, host.len())),
    ]);
    let value = yaml_value(
        &documents[0],
        "/documents/0",
        host,
        &byte_offsets,
        &mut mapping,
    )
    .unwrap();
    let note = "/documents/0/note";
    let entry = mapping.get(note).unwrap();
    let [start, end] = entry.range.unwrap().map(|value| value as usize);
    assert_eq!(&host[start..end], "Cross-reference to a minivm document.");
    let segments = entry.text_map.as_ref().unwrap();
    assert_eq!(segments.first().unwrap().source_range[0] as usize, start);
    assert_eq!(segments.last().unwrap().source_range[1] as usize, end);

    let source = Source {
        path: "unicode.yaml".to_owned(),
        ast: serde_json::json!({"kind": "Stream", "documents": [value]}),
        mapping,
    };
    source.validate(host).unwrap();
}
