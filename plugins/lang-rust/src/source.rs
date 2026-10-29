use std::collections::BTreeMap;

use lighthouse_protocol::{Mapping, Source, TextMapSegment, pointer_child};
use quote::ToTokens;
use serde_json::{Map, Value};
use syn::{Expr, Fields, FnArg, ImplItem, Item, Pat, Stmt, Type, Variant, spanned::Spanned};

use crate::tree::SourceFile;

mod common;
mod expr;
mod item;
mod member;
mod signature;
mod types;

use common::*;
use expr::*;
use item::*;
use member::*;
use signature::*;
use types::*;

pub fn from_file(file: &SourceFile) -> Result<Source, String> {
    let mut source_map = BTreeMap::from([(
        String::new(),
        Mapping {
            range: Some([0, offset(file.text.len())?]),
            text_pointer: None,
            text_map: None,
        },
    )]);
    let mut root = Map::new();
    root.insert("kind".to_owned(), Value::String("File".to_owned()));
    root.insert(
        "shebang".to_owned(),
        file.ast.shebang.clone().map_or(Value::Null, Value::String),
    );
    root.insert(
        "attrs".to_owned(),
        attributes(file, "/attrs", &file.ast.attrs, &mut source_map)?,
    );
    let mut items = Vec::with_capacity(file.ast.items.len());
    for (index, item) in file.ast.items.iter().enumerate() {
        let pointer = pointer_child("/items", &index.to_string());
        items.push(item_ast(file, &pointer, item, &mut source_map)?);
    }
    root.insert("items".to_owned(), Value::Array(items));
    Ok(Source {
        path: file.rel.clone(),
        ast: Value::Object(root),
        mapping: source_map,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_file;

    fn file(text: &str) -> SourceFile {
        SourceFile {
            rel: "src/lib.rs".to_owned(),
            text: text.to_owned(),
            ast: parse_file(text).unwrap(),
            generated: false,
            line_starts: crate::tree::line_starts(text),
        }
    }

    #[test]
    fn rust_ast_keeps_nested_items_and_pointer_ranges() {
        let text = "pub fn greet(person: &str) { let value = person.trim(); println!(\"hello {value}\"); }\nstruct Person { name: String }\n";
        let file = file(text);
        let serialized = from_file(&file).unwrap();
        let function = serialized.value_at("/items/0").unwrap();
        assert_eq!(function["kind"], "ItemFn");
        assert_eq!(function.pointer("/sig/ident/name").unwrap(), "greet");
        assert_eq!(
            function.pointer("/sig/inputs/0/pat/ident/name").unwrap(),
            "person"
        );
        assert_eq!(function.pointer("/block/stmts/0/kind").unwrap(), "Local");
        let source_map = serialized.mapping.get("/items/0").unwrap();
        assert_eq!(source_map.text_pointer.as_deref(), Some("/items/0/source"));
        assert!(serialized.mapping.contains_key("/items/0/block/stmts/0"));
        assert!(
            serialized.validate(text).is_ok(),
            "{:?}",
            serialized.validate(text)
        );
    }

    #[test]
    fn identical_nested_expressions_keep_distinct_pointers_and_ranges() {
        let text = "fn pair() { consume(\"same\", \"same\"); }\n";
        let file = file(text);
        let ast = from_file(&file).unwrap();
        let first = ast
            .mapping
            .get("/items/0/block/stmts/0/expr/args/0")
            .unwrap()
            .range
            .unwrap();
        let second = ast
            .mapping
            .get("/items/0/block/stmts/0/expr/args/1")
            .unwrap()
            .range
            .unwrap();
        assert!(first[0] < second[0], "{first:?} {second:?}");
        assert_eq!(
            ast.value_at("/items/0/block/stmts/0/expr/args/0").unwrap()["kind"],
            "Lit"
        );
    }
}
