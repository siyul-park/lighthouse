use super::*;

#[test]
fn selector_parses_keys_indices_wildcards_and_filters() {
    let selector = Selector::parse(
        "$['spec']['rules'][*].condition?(@.kind == 'Decision' && @.enabled != false)",
    )
    .unwrap();
    assert_eq!(
        selector.parts(),
        &[
            SelectorPart::Key("spec".to_owned()),
            SelectorPart::Key("rules".to_owned()),
            SelectorPart::Indices,
            SelectorPart::Key("condition".to_owned()),
            SelectorPart::Filter(SelectorFilter {
                clauses: vec![vec![
                    SelectorComparison {
                        path: Selector::parse("$.kind").unwrap(),
                        equal: true,
                        value: Value::String("Decision".to_owned()),
                    },
                    SelectorComparison {
                        path: Selector::parse("$.enabled").unwrap(),
                        equal: false,
                        value: Value::Bool(false),
                    },
                ]],
            }),
        ]
    );
    assert_eq!(
        Selector::parse("$.rules[12].condition").unwrap().parts(),
        &[
            SelectorPart::Key("rules".to_owned()),
            SelectorPart::Index(12),
            SelectorPart::Key("condition".to_owned()),
        ]
    );
    assert_eq!(
        Selector::parse("$[?(@.kind == 'Decision')]")
            .unwrap()
            .parts()
            .len(),
        1
    );
}

#[test]
fn selector_parses_the_descendant_segment() {
    assert_eq!(
        Selector::parse("$..name").unwrap().parts(),
        &[SelectorPart::Descend, SelectorPart::Key("name".to_owned())]
    );
    assert_eq!(
        Selector::parse("$..*").unwrap().parts(),
        &[SelectorPart::Descend, SelectorPart::Keys]
    );
    assert_eq!(
        Selector::parse("$..[0]").unwrap().parts(),
        &[SelectorPart::Descend, SelectorPart::Index(0)]
    );
    assert!(matches!(
        Selector::parse("$..?(@.kind == 'code_block').text")
            .unwrap()
            .parts(),
        [
            SelectorPart::Descend,
            SelectorPart::Filter(_),
            SelectorPart::Key(_)
        ]
    ));
}

#[test]
fn selector_rejects_ambiguous_syntax() {
    for selector in [
        "",
        "spec.rules",
        "$..",
        "$.[0]",
        "$[x]",
        "$['broken]",
        "$?(",
        "$[?(@.kind = 'x')]",
        "$?(@.kind ==)",
        "$.bad key",
    ] {
        assert!(Selector::parse(selector).is_err(), "{selector}");
    }
}

#[test]
fn selector_keys_support_unicode_without_slicing_mid_character() {
    assert_eq!(
        Selector::parse("$.名-稱").unwrap().parts(),
        &[SelectorPart::Key("名-稱".to_owned())]
    );
    assert_eq!(
        Selector::parse("$['雪\\'家']").unwrap().parts(),
        &[SelectorPart::Key("雪'家".to_owned())]
    );
    assert_eq!(
        Selector::parse(r#"$["line\nbreak"]"#).unwrap().parts(),
        &[SelectorPart::Key("line\nbreak".to_owned())]
    );
    assert_eq!(
        Selector::parse(r#"$["path\\name"]"#).unwrap().parts(),
        &[SelectorPart::Key(r"path\name".to_owned())]
    );
}
