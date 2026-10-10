//! The kinds of documents this crate defines, and the small types they are
//! made of.

use std::collections::BTreeMap;

use lighthouse_model::RunScope;
use lighthouse_spec::{
    CelCheck, Check, CheckKind, Decision, OptionSchema, OptionType, OptionsSchema, Select,
    descriptors,
};
use serde_json::json;

#[test]
fn every_kind_of_the_catalog_has_a_schema() {
    let kinds: Vec<&str> = descriptors().iter().map(|d| d.kind).collect();

    assert_eq!(kinds, ["Decision", "Pack", "Project", "SourceMap"]);
    for descriptor in descriptors() {
        assert_eq!(
            descriptor.schema["properties"]["kind"]["const"],
            descriptor.kind
        );
    }
}

#[test]
fn what_a_cel_check_selects_decides_its_scope_and_its_variable() {
    for (select, scope, variable) in [
        (Select::Symbol, RunScope::File, "symbol"),
        (Select::Function, RunScope::File, "func"),
        (Select::File, RunScope::File, "file"),
        (Select::Test, RunScope::File, "test"),
        (Select::Edge, RunScope::Project, "edge"),
        (Select::Module, RunScope::Project, "module"),
    ] {
        assert_eq!(select.scope(), scope, "{}", select.name());
        assert_eq!(select.variable(), variable, "{}", select.name());
    }
}

#[test]
fn a_cel_check_reads_its_select_where_message_and_evidence() {
    let check: Check = serde_json::from_value(json!({
        "type": "cel",
        "select": "function",
        "where": "func.statements > 5",
        "message": "{{ func.name }} is long",
        "evidence": { "name": "func.name" },
    }))
    .unwrap();

    let CheckKind::Cel(CelCheck {
        select,
        condition,
        message,
        evidence,
        ..
    }) = check.kind
    else {
        panic!("cel expected");
    };
    assert_eq!(select, Select::Function);
    assert_eq!(condition, "func.statements > 5");
    assert_eq!(message, "{{ func.name }} is long");
    assert_eq!(evidence["name"], "func.name");
}

#[test]
fn the_short_name_of_a_decision_is_its_name_inside_the_pack() {
    let decision = lighthouse_spec::Catalog::bundled()
        .decision("design/minimal-names")
        .unwrap();

    assert_eq!(decision.short_name(), "minimal-names");
    assert_eq!(decision.id(), "design/minimal-names");
    let id: &str = Decision::id(decision);
    assert!(id.starts_with(decision.pack()));
}

#[test]
fn a_set_of_option_properties_is_a_closed_object_schema() {
    let schema = OptionsSchema::new(BTreeMap::from([(
        "max".to_owned(),
        OptionSchema {
            kind: OptionType::Integer,
            default: json!(3),
            description: "Limit.".to_owned(),
        },
    )]));

    let value = serde_json::to_value(&schema).unwrap();

    assert_eq!(value["type"], "object");
    assert_eq!(value["additionalProperties"], false);
    assert_eq!(value["properties"]["max"]["type"], "integer");
    assert_eq!(
        serde_json::from_value::<OptionsSchema>(value).unwrap(),
        schema
    );
}
