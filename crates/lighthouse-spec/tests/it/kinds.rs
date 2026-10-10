//! The kinds of documents this crate defines, and the small types they are
//! made of.

use std::collections::BTreeMap;

use lighthouse_model::RunScope;
use lighthouse_spec::{
    CelCheck, Check, CheckKind, Decision, OptionSchema, OptionType, OptionsSchema, Select, Subject,
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
    assert_eq!(select, Some(Select::Function));
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
        OptionSchema::new(OptionType::Integer, json!(3), "Limit."),
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

#[test]
fn select_of() {
    assert_eq!(Select::of(Subject::Symbol), Some(Select::Symbol));
    assert_eq!(Select::of(Subject::File), Some(Select::File));
    assert_eq!(Select::of(Subject::Module), Some(Select::Module));
    assert_eq!(Select::of(Subject::Test), Some(Select::Test));
    assert_eq!(Select::of(Subject::Project), None);
}

#[test]
fn cel_check_selects() {
    let check = |select: Option<Select>| CelCheck {
        select,
        bindings: Vec::new(),
        condition: "true".to_owned(),
        message: "m".to_owned(),
        evidence: BTreeMap::new(),
        at: None,
        identity: None,
    };

    assert_eq!(check(None).selects(Subject::File), Some(Select::File));
    assert_eq!(
        check(Some(Select::Function)).selects(Subject::Symbol),
        Some(Select::Function)
    );
    assert_eq!(check(None).selects(Subject::Project), None);
}

#[test]
fn shape_of() {
    let shape = lighthouse_spec::Shape::of(OptionType::Boolean);
    assert!(shape.check(&json!(true), "x").is_ok());
    assert_eq!(
        shape.check(&json!(1), "x").unwrap_err(),
        "x must be a boolean"
    );
}

#[test]
fn definitions_name_the_limit_shape() {
    let limit = lighthouse_spec::definitions()["limit"].clone();
    assert_eq!(limit.one_of.len(), 2);
    let object = &limit.one_of[1];
    for role in lighthouse_spec::LIMIT_ROLES.into_iter().chain(["default"]) {
        assert!(object.properties.contains_key(role), "{role}");
    }
    assert!(
        limit
            .check(&json!({ "test": -1, "default": 1 }), "x")
            .is_ok()
    );
}

#[test]
fn option_schema_holds_its_shape() {
    let option = OptionSchema::new(OptionType::Integer, json!(3), "Limit.");
    assert_eq!(option.shape.kind, Some(OptionType::Integer));
    assert!(option.shape.one_of.is_empty());
}

#[test]
fn shape_check() {
    let limit = lighthouse_spec::definitions()["limit"].clone();
    assert!(limit.check(&json!(3), "x").is_ok());
    assert!(
        limit
            .check(&json!(-2), "x")
            .unwrap_err()
            .contains("at least -1")
    );
}

#[test]
fn option_schema_check() {
    let option = OptionSchema::new(OptionType::Integer, json!(3), "Limit.");
    assert!(option.check(&json!(4), "x").is_ok());
    assert!(option.check(&json!("4"), "x").is_err());
}
