use super::index::{index_with_sources, project};
use serde_json::json;

#[test]
fn requested_source_ast_keeps_native_index_results() {
    let dir = project(&[
        (
            "Cargo.toml",
            "[package]\nname = \"source-ast-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub fn greet() { println!(\"hello\"); }\n"),
    ]);
    let result = index_with_sources(dir.path(), json!({}), None, true);

    let ast = result
        .sources
        .iter()
        .find(|ast| ast.path == "src/lib.rs")
        .expect("requested Rust source AST");
    assert_eq!(ast.value_at("/items/0/kind"), Some(&json!("ItemFn")));
    assert_eq!(
        ast.value_at("/items/0/sig/ident/name"),
        Some(&json!("greet"))
    );
    assert_eq!(
        ast.mapping
            .get("/items/0")
            .and_then(|entry| entry.text_pointer.as_deref()),
        Some("/items/0/source")
    );
    assert!(
        ast.validate("pub fn greet() { println!(\"hello\"); }\n")
            .is_ok()
    );
    assert!(
        result
            .fragments
            .iter()
            .any(|fragment| fragment.file.path == "src/lib.rs"),
        "source-AST response must keep normal Rust indexing results"
    );
}
