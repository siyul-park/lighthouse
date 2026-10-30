use super::*;

#[test]
fn coalescing_text_maps_keeps_non_linear_ranges_separate() {
    let text = "é\"b";
    let starts = [10, 10, 12, 14];
    let ends = [12, 12, 14, 15];
    let segments = coalesce_text_map(text, &starts, &ends).unwrap();
    assert_eq!(
        segments,
        vec![
            TextMapSegment {
                text_range: [0, 2],
                source_range: [10, 12],
            },
            TextMapSegment {
                text_range: [2, 3],
                source_range: [12, 14],
            },
            TextMapSegment {
                text_range: [3, 4],
                source_range: [14, 15],
            },
        ]
    );
    assert_eq!(coalesce_text_map("x", &[], &[]), None);
    assert_eq!(coalesce_text_map("x", &[2], &[1]), None);
}

#[test]
fn source_value_at_reads_the_canonical_pointer() {
    let source = Source {
        path: "sample.go".to_owned(),
        ast: serde_json::json!({"a/b": {"~key": "value"}}),
        mapping: BTreeMap::new(),
    };
    assert_eq!(
        source.value_at("/a~1b/~0key"),
        Some(&serde_json::json!("value"))
    );
}

#[test]
fn pointer_tokens_decodes_canonical_escapes() {
    assert_eq!(pointer_tokens("/a~1b/~0key").unwrap(), ["a/b", "~key"]);
    assert!(pointer_tokens("/bad~2token").is_none());
}

#[test]
fn pointer_value_resolves_an_array_element() {
    let value = serde_json::json!({"items": ["zero", "one"]});
    assert_eq!(
        pointer_value(&value, "/items/1"),
        Some(&serde_json::json!("one"))
    );
}

#[test]
fn pointer_child_escapes_pointer_tokens() {
    assert_eq!(pointer_child("/items", "a/b~c"), "/items/a~1b~0c");
}

#[test]
fn source_ast_validates_json_pointers_and_segment_maps() {
    let ast = Source {
        path: "config.yaml".to_owned(),
        ast: serde_json::json!({"a/b": {"~key": "éx"}}),
        mapping: BTreeMap::from([
            (
                "".to_owned(),
                Mapping {
                    range: Some([0, 3]),
                    ..Default::default()
                },
            ),
            (
                "/a~1b".to_owned(),
                Mapping {
                    range: Some([0, 3]),
                    text_pointer: Some("/a~1b/~0key".to_owned()),
                    ..Default::default()
                },
            ),
            (
                "/a~1b/~0key".to_owned(),
                Mapping {
                    range: Some([0, 3]),
                    text_map: Some(vec![
                        TextMapSegment {
                            text_range: [0, 2],
                            source_range: [0, 2],
                        },
                        TextMapSegment {
                            text_range: [2, 3],
                            source_range: [2, 3],
                        },
                    ]),
                    ..Default::default()
                },
            ),
        ]),
    };
    assert!(ast.validate("éx").is_ok());

    let mut malformed = ast.clone();
    malformed.mapping.get_mut("/a~1b/~0key").unwrap().text_map = Some(vec![TextMapSegment {
        text_range: [1, 2],
        source_range: [0, 1],
    }]);
    assert!(malformed.validate("éx").unwrap_err().contains("text_map"));

    let mut malformed = ast;
    malformed.mapping.get_mut("/a~1b").unwrap().text_pointer = Some("/missing".to_owned());
    assert!(
        malformed
            .validate("éx")
            .unwrap_err()
            .contains("text_pointer")
    );
}

#[test]
fn text_map_requires_the_whole_source_scalar_range() {
    let source = Source {
        path: "host.yaml".to_owned(),
        ast: serde_json::json!({"source":"x"}),
        mapping: BTreeMap::from([(
            "/source".to_owned(),
            Mapping {
                text_map: Some(vec![TextMapSegment {
                    text_range: [0, 1],
                    source_range: [0, 1],
                }]),
                ..Default::default()
            },
        )]),
    };
    assert!(
        source
            .validate("x")
            .unwrap_err()
            .contains("text_map at /source has no source range")
    );
}

#[test]
fn text_pointer_requires_a_ranged_owner_that_contains_its_source_scalar() {
    let source = || Source {
        path: "host.go".to_owned(),
        ast: serde_json::json!({"expr":{"source":"abc"}}),
        mapping: BTreeMap::from([
            (
                "".to_owned(),
                Mapping {
                    range: Some([0, 5]),
                    ..Default::default()
                },
            ),
            (
                "/expr".to_owned(),
                Mapping {
                    range: Some([0, 3]),
                    text_pointer: Some("/expr/source".to_owned()),
                    ..Default::default()
                },
            ),
            (
                "/expr/source".to_owned(),
                Mapping {
                    range: Some([0, 3]),
                    ..Default::default()
                },
            ),
        ]),
    };

    assert!(source().validate("abcde").is_ok());

    let mut missing_owner_range = source();
    missing_owner_range.mapping.get_mut("/expr").unwrap().range = None;
    assert!(
        missing_owner_range
            .validate("abcde")
            .unwrap_err()
            .contains("source-bearing node /expr has no source range")
    );

    let mut outside_owner = source();
    outside_owner.mapping.get_mut("/expr/source").unwrap().range = Some([2, 5]);
    assert!(
        outside_owner
            .validate("abcde")
            .unwrap_err()
            .contains("range is outside the source-bearing node /expr")
    );
}

#[test]
fn source_ast_rejects_bad_pointer_escapes_and_missing_scalar_ranges() {
    let ast = Source {
        path: "host.yaml".to_owned(),
        ast: serde_json::json!({"source": "x"}),
        mapping: BTreeMap::from([(
            "/~2".to_owned(),
            Mapping {
                range: Some([0, 1]),
                ..Default::default()
            },
        )]),
    };
    assert!(
        ast.validate("x")
            .unwrap_err()
            .contains("non-canonical JSON Pointer")
    );

    let ast = Source {
        path: "host.yaml".to_owned(),
        ast: serde_json::json!({"source": "x"}),
        mapping: BTreeMap::from([
            (
                "".to_owned(),
                Mapping {
                    range: Some([0, 1]),
                    text_pointer: Some("/source".to_owned()),
                    ..Default::default()
                },
            ),
            ("/source".to_owned(), Mapping::default()),
        ]),
    };
    assert!(ast.validate("x").unwrap_err().contains("no source range"));
}
