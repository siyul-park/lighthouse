use super::*;

#[test]
fn non_finite_numbers_are_not_silently_serialized_as_null() {
    let document = Document::parse("value = nan\n").unwrap();
    let error = collect_table(document.as_table(), &mut Vec::new(), &mut Vec::new()).unwrap_err();
    assert!(error.contains("cannot be represented in a JSON AST"));
}

#[test]
fn serialized_ast_keeps_decoded_scalar_and_json_pointer_source_map() {
    let host = "where = \"x > 1\"\nlimit = 10\n";
    let document = Document::parse(host).unwrap();
    let mut parsed = vec![Node {
        path: Vec::new(),
        kind: Kind::Object,
        span: Some(0..host.len()),
    }];
    collect_table(document.as_table(), &mut Vec::new(), &mut parsed).unwrap();
    let ast = source_ast(&parsed, host, "config.toml").unwrap();

    assert_eq!(ast.value_at("/where").unwrap(), "x > 1");
    let start = host.find("x > 1").unwrap();
    let entry = ast.mapping.get("/where").unwrap();
    assert_eq!(entry.range, Some([8, 15]));
    let map = entry.text_map.as_ref().unwrap();
    assert_eq!(map.first().unwrap().source_range[0] as usize, start);
    assert_eq!(
        map.last().unwrap().source_range[1] as usize,
        start + "x > 1".len()
    );
    assert_eq!(ast.value_at("/limit").unwrap(), 10);
}

#[test]
fn invalid_text_maps_are_rejected() {
    assert!(coalesce_text_map("x", &[], &[]).is_none());
    assert!(coalesce_text_map("x", &[2], &[1]).is_none());
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
