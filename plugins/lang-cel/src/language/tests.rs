use std::path::PathBuf;

use lighthouse_model::ucm::File;
use lighthouse_plugin::{Source, Workspace};
use serde_json::json;

use super::*;

#[test]
fn sibling_source_fields_share_their_logical_document_identity() {
    let context = json!({
        "identity": {"kind": "Decision", "name": "design/no-unused-symbol"}
    });
    let path = PathBuf::from("decisions/design/no-unused-symbol.yaml");
    let document = "Decision:design/no-unused-symbol";
    assert_eq!(
        source_document(&context, &path, "/spec/check/where"),
        document
    );
    assert_eq!(
        source_document(&context, &path, "/spec/check/message"),
        document
    );
    assert_eq!(
        source_document(
            &json!({"identity": {"tenant": "test", "name": "example"}}),
            &path,
            "/where"
        ),
        r#"{"name":"example","tenant":"test"}"#
    );
    assert_eq!(
        source_document(&json!({"document": "explicit"}), &path, "/where"),
        "explicit"
    );
    assert_eq!(
        source_document(&json!({}), &path, "/where"),
        "decisions/design/no-unused-symbol.yaml:/where"
    );
}

#[test]
fn undeclared_source_expressions_keep_edges_without_becoming_functions() {
    let host = "where: node.name != \"\"\n";
    let expression = "node.name != \"\"";
    let start = host.find(expression).unwrap();
    let file = File {
        path: PathBuf::from("decisions/example.yaml"),
        lang: "yaml".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
        facts: Default::default(),
    };
    let embedded = EmbeddedFile {
        file,
        text: host.to_owned(),
        sources: vec![EmbeddedSource {
            id: "decision-where:decisions/example.yaml:0:$.where".to_owned(),
            rule_id: "decision-where".to_owned(),
            node_pointer: "/where".to_owned(),
            source_pointer: "/where".to_owned(),
            provider: "cel".to_owned(),
            syntax: None,
            context: json!({"identity":{"kind":"Decision","name":"example/check"}}),
            text: expression.to_owned(),
            source_range: [start as u32, (start + expression.len()) as u32],
            text_map: Some(vec![lighthouse_plugin::TextMapSegment {
                text_range: [0, expression.len() as u32],
                source_range: [start as u32, (start + expression.len()) as u32],
            }]),
        }],
    };
    let source = Source {
        file: &embedded.file,
        text: &embedded.text,
        regions: &embedded.sources,
    };
    let indexed = Language::new()
        .index(&Workspace::new("/tmp"), &[source])
        .unwrap();
    assert!(indexed.incomplete.is_empty(), "{:?}", indexed.incomplete);
    assert!(
        indexed
            .fragments
            .iter()
            .all(|fragment| fragment.functions.is_empty())
    );
    assert!(
        indexed
            .fragments
            .iter()
            .any(|fragment| !fragment.edges.is_empty())
    );
}

#[test]
fn indexes_embedded_cel_at_original_host_coordinates() {
    let host = "where: node.name != \"\"\n";
    let expression = "node.name != \"\"";
    let start = host.find(expression).unwrap();
    let file = File {
        path: PathBuf::from("decisions/example.yaml"),
        lang: "yaml".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
        facts: Default::default(),
    };
    let embedded = EmbeddedFile {
        file,
        text: host.to_owned(),
        sources: vec![EmbeddedSource {
            id: "decision-where:decisions/example.yaml:0:$.where".to_owned(),
            rule_id: "decision-where".to_owned(),
            node_pointer: "/where".to_owned(),
            source_pointer: "/where".to_owned(),
            provider: "cel".to_owned(),
            syntax: None,
            context: json!({"identity":{"kind":"Decision","name":"example/check"}, "declared":true}),
            text: expression.to_owned(),
            source_range: [start as u32, (start + expression.len()) as u32],
            text_map: Some(
                expression
                    .char_indices()
                    .map(|(offset, ch)| lighthouse_plugin::TextMapSegment {
                        text_range: [offset as u32, (offset + ch.len_utf8()) as u32],
                        source_range: [
                            (start + offset) as u32,
                            (start + offset + ch.len_utf8()) as u32,
                        ],
                    })
                    .collect(),
            ),
        }],
    };
    let source = Source {
        file: &embedded.file,
        text: &embedded.text,
        regions: &embedded.sources,
    };
    let indexed = Language::new()
        .index(&Workspace::new("/tmp"), &[source])
        .unwrap();
    assert!(indexed.incomplete.is_empty(), "{:?}", indexed.incomplete);
    let project = lighthouse_model::ucm::Project::merge(indexed.fragments);
    assert_eq!(project.files.len(), 1);
    assert_eq!(project.files[0].lang, "yaml");
    let expression_symbol = project
        .symbols
        .iter()
        .find(|symbol| symbol.name == "where")
        .unwrap();
    assert_eq!(expression_symbol.span.start.line, 1);
    assert_eq!(expression_symbol.span.start.col, 8);
    assert!(
        project
            .functions
            .iter()
            .any(|function| function.statements > 0)
    );

    let mut unsupported = embedded.clone();
    unsupported.sources[0].syntax = Some("sql".to_owned());
    let source = Source {
        file: &unsupported.file,
        text: &unsupported.text,
        regions: &unsupported.sources,
    };
    let indexed = Language::new()
        .index(&Workspace::new("/tmp"), &[source])
        .unwrap();
    assert!(
        indexed
            .incomplete
            .iter()
            .any(|gap| gap.reason.contains("does not support embedded syntax sql")),
        "{:?}",
        indexed.incomplete
    );
    assert!(
        indexed
            .fragments
            .iter()
            .all(|fragment| fragment.functions.is_empty()),
        "unsupported syntax must not be analyzed as direct CEL"
    );
}

#[test]
fn a_decision_resolves_the_shared_variable_of_its_pack() {
    let host = "pack: 'size(x) > 1'\nwhere: 'shared'\n";
    let at = |text: &str| {
        let start = host.find(text).unwrap();
        [start as u32, (start + text.len()) as u32]
    };
    let region = |pointer: &str, text: &str, context: serde_json::Value| EmbeddedSource {
        id: format!("rule:{pointer}"),
        rule_id: "rule".to_owned(),
        node_pointer: pointer.to_owned(),
        source_pointer: pointer.to_owned(),
        provider: "cel".to_owned(),
        syntax: None,
        context,
        text: text.to_owned(),
        source_range: at(text),
        text_map: None,
    };
    let sources = vec![
        region(
            "/pack",
            "size(x) > 1",
            json!({"identity": {"kind": "Pack", "name": "design"},
                   "variable": {"name": "shared", "shared": true}}),
        ),
        region(
            "/where",
            "shared",
            json!({"identity": {"kind": "Decision", "name": "design/x"}}),
        ),
    ];
    let file = File {
        path: PathBuf::from("decisions/x.yaml"),
        lang: "yaml".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
        facts: Default::default(),
    };
    let source = Source {
        file: &file,
        text: host,
        regions: &sources,
    };
    let indexed = Language::new()
        .index(&Workspace::new("/tmp"), &[source])
        .unwrap();
    let project = lighthouse_model::ucm::Project::merge(indexed.fragments);
    let shared = project
        .symbols
        .iter()
        .find(|symbol| symbol.name == "shared")
        .expect("the pack declares the variable");
    assert!(
        !project.references(&shared.id).is_empty(),
        "no reference to {}",
        shared.id.as_str()
    );
}

#[test]
fn template_syntax_rejects_unclosed_or_unmatched_delimiters() {
    assert_eq!(template_parts("hello {{ value }}!").unwrap().len(), 1);
    assert!(template_parts("hello {{ value").is_err());
    assert!(template_parts("hello value }}").is_err());
    assert!(template_parts("{{ }}").is_err());
}
