//! The kinds of documents this crate defines, and the small types they are
//! made of.

use std::collections::BTreeMap;

use lighthouse_plugin::Scope as RunScope;
use lighthouse_spec::{
    CelCheck, Check, Decision, DecisionOverrideSpec, OptionSchema, OptionType, OptionsSchema,
    Select, descriptors, is_resource,
};
use serde_json::json;
use serde_norway::Value;

#[test]
fn every_kind_of_the_catalog_has_a_schema() {
    let kinds: Vec<&str> = descriptors().iter().map(|d| d.kind).collect();

    assert_eq!(kinds, ["Decision", "DecisionOverride", "Pack", "SourceMap"]);
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

    let Check::Cel(CelCheck {
        select,
        condition,
        message,
        evidence,
    }) = check
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
fn a_document_is_a_resource_once_it_has_an_api_version() {
    let old: Value = serde_norway::from_str("id: p/a\ntitle: A\n").unwrap();
    let new: Value =
        serde_norway::from_str("apiVersion: lighthouse/v1alpha1\nkind: Decision\n").unwrap();

    assert!(!is_resource(&old));
    assert!(is_resource(&new));
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

#[test]
fn an_override_names_the_decision_it_adjusts_and_what_it_changes() {
    let patch: DecisionOverrideSpec = serde_json::from_value(json!({
        "extends": "p/a",
        "severity": "warn",
        "options": { "max": 9 },
    }))
    .unwrap();

    assert_eq!(patch.extends, "p/a");
    assert_eq!(patch.severity, Some(lighthouse_model::Severity::Warn));
    assert_eq!(patch.options["max"], 9);
    assert!(patch.examples.is_empty());
    let typo =
        serde_json::from_value::<DecisionOverrideSpec>(json!({ "extends": "p/a", "title": "x" }));
    assert!(typo.is_err());
}
