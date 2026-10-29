//! The Rust fences of a Markdown file: units of their own that use the crate
//! around the file, reported where the file shows them.

use std::path::Path;

use lighthouse_protocol::{
    EdgeKind, EmbeddedFile, EmbeddedSource, Fragment, IndexInput, IndexResult, Node, TextMapSegment,
};
use serde_json::{Value, json};

use super::index::{exchange, project};

const MANIFEST: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";
const LIB: &str = "pub fn greet() {}\n";
const GREET: &str = "demo::greet#function";
const CALLER: &str = "use demo::greet;\n\npub fn run() { greet(); }\n";
const HIDDEN: &str = "# use demo::greet;\n\npub fn run() { greet(); }\n";
const UNKNOWN: &str = "use other::thing;\n\npub fn run() { thing(1); }\n";

#[test]
fn a_fence_that_uses_the_crate_around_it_reaches_its_public_symbols() {
    let dir = crate_with(LIB);

    let result = fences(dir.path(), &[(CALLER, json!({}))]);

    let fragment = fragment(&result);
    let call = edge(fragment, EdgeKind::Calls, GREET);
    assert_eq!(call.site.map(|site| site.start.line), Some(4));
    assert_eq!(call.site.map(|site| site.start.col), Some(16));
    assert_eq!(
        edge(fragment, EdgeKind::Imports, "demo")
            .site
            .map(|site| site.start.line),
        Some(2)
    );
    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
}

#[test]
fn the_wrapper_of_statements_is_no_symbol_and_no_place() {
    let dir = crate_with(LIB);

    let result = fences(dir.path(), &[("let _ = 1;\ndemo::greet();\n", json!({}))]);

    let fragment = fragment(&result);
    assert!(fragment.symbols.iter().all(|symbol| symbol.name != "main"));
    let call = edge(fragment, EdgeKind::Calls, GREET);
    assert!(matches!(&call.from, Node::Module(module) if module == "README.md#1"));
    assert_eq!(call.site.map(|site| site.start.line), Some(3));
}

#[test]
fn items_in_a_fence_are_symbols_at_their_place() {
    let dir = crate_with(LIB);

    let result = fences(dir.path(), &[("pub struct Sample;\n", json!({}))]);

    let fragment = fragment(&result);
    let symbol = fragment
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Sample")
        .expect("the declared type");
    assert_eq!(symbol.span.start.line, 2);
    assert_eq!(symbol.file, "README.md");
}

#[test]
fn a_hidden_line_is_read_and_not_placed() {
    let dir = crate_with(LIB);

    let result = fences(dir.path(), &[(HIDDEN, json!({}))]);

    let fragment = fragment(&result);
    assert_eq!(edge(fragment, EdgeKind::Imports, "demo").site, None);
    assert_eq!(
        edge(fragment, EdgeKind::Calls, GREET)
            .site
            .map(|site| site.start.line),
        Some(4)
    );
}

#[test]
fn identical_fences_are_distinct_units() {
    let dir = crate_with(LIB);

    let result = fences(
        dir.path(),
        &[
            ("pub struct Same;\n", json!({})),
            ("pub struct Same;\n", json!({})),
        ],
    );

    let fragment = fragment(&result);
    let modules: Vec<&str> = fragment.modules.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(modules, ["README.md#1", "README.md#2"]);
    let ids: Vec<&str> = fragment.symbols.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["README.md#1::Same#type", "README.md#2::Same#type"]);
}

#[test]
fn a_fragment_that_names_what_the_prose_defines_is_not_a_gap() {
    let dir = crate_with(LIB);

    let result = fences(dir.path(), &[(UNKNOWN, json!({}))]);

    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    assert!(result.notices.iter().any(|n| n.contains("syntactic")));
}

#[test]
fn a_fence_that_is_not_rust_is_a_notice() {
    let dir = crate_with(LIB);

    let result = fences(dir.path(), &[("this is { not rust\n", json!({}))]);

    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    assert!(
        result
            .notices
            .iter()
            .any(|n| n.starts_with("README.md:2:") && n.contains("not valid Rust")),
        "{:?}",
        result.notices
    );
}

#[test]
fn a_fence_without_a_crate_around_it_uses_nothing_but_the_standard_library() {
    let dir = project(&[]);

    let result = fences(dir.path(), &[("use std::fmt;\nlet _ = 1;\n", json!({}))]);

    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    assert!(result.notices.is_empty(), "{:?}", result.notices);
}

#[test]
fn a_fence_in_a_module_sees_its_crate_and_super() {
    let dir = crate_with(LIB);

    let result = fences(
        dir.path(),
        &[("crate::greet();\n", json!({ "crate": "src/lib.rs" }))],
    );

    let fragment = fragment(&result);
    let call = edge(fragment, EdgeKind::Calls, GREET);
    assert_eq!(call.site.map(|site| site.start.line), Some(2));
    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
}

#[test]
fn a_module_the_package_does_not_have_is_a_gap() {
    let dir = crate_with(LIB);

    let result = fences(
        dir.path(),
        &[("crate::greet();\n", json!({ "crate": "src/none.rs" }))],
    );

    assert_eq!(result.incomplete.len(), 1);
    assert!(result.incomplete[0].reason.contains("src/none.rs"));
}

#[test]
fn places_are_bytes_of_the_host_whatever_the_line_endings_and_letters() {
    let dir = crate_with(LIB);

    let result = fences(
        dir.path(),
        &[("// \u{c548}\u{b155}\r\ndemo::greet();\r\n", json!({}))],
    );

    let call = edge(fragment(&result), EdgeKind::Calls, GREET);
    assert_eq!(call.site.map(|site| site.start.line), Some(3));
    assert_eq!(call.site.map(|site| site.start.col), Some(7));
}

fn crate_with(lib: &str) -> tempfile::TempDir {
    project(&[("Cargo.toml", MANIFEST), ("src/lib.rs", lib)])
}

/// The fences of `README.md`, each with the context its source rule gives.
fn fences(dir: &Path, bodies: &[(&str, Value)]) -> IndexResult {
    let mut host = String::new();
    let mut sources = Vec::new();
    for (at, (body, context)) in bodies.iter().enumerate() {
        host.push_str("```rust\n");
        let from = host.len();
        host.push_str(body);
        sources.push(source(at, body, from, context));
        host.push_str("```\n\n");
    }
    let file = EmbeddedFile {
        path: "README.md".to_owned(),
        hash: String::new(),
        text: host,
        generated: false,
        test: false,
        facts: Default::default(),
        sources,
    };
    let root = dir.canonicalize().unwrap();
    exchange(
        &root,
        vec![IndexInput::Regions { regions: file }],
        json!({}),
        None,
    )
}

fn source(at: usize, body: &str, from: usize, context: &Value) -> EmbeddedSource {
    let to = from + body.len();
    EmbeddedSource {
        id: format!("fence-{at}"),
        rule_id: "markdown-rust".to_owned(),
        node_pointer: format!("/children/{at}"),
        source_pointer: format!("/children/{at}/text"),
        provider: "rust".to_owned(),
        syntax: None,
        context: context.clone(),
        text: body.to_owned(),
        source_range: [from as u32, to as u32],
        text_map: Some(vec![TextMapSegment {
            text_range: [0, body.len() as u32],
            source_range: [from as u32, to as u32],
        }]),
    }
}

fn fragment(result: &IndexResult) -> &Fragment {
    assert_eq!(result.fragments.len(), 1, "{:?}", result.notices);
    &result.fragments[0]
}

fn edge<'a>(fragment: &'a Fragment, kind: EdgeKind, to: &str) -> &'a lighthouse_protocol::Edge {
    fragment
        .edges
        .iter()
        .find(|edge| edge.kind == kind && edge.to == to)
        .unwrap_or_else(|| panic!("no {kind:?} edge to {to}: {:?}", fragment.edges))
}
