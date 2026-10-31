use std::collections::BTreeMap;

use lighthouse_protocol::{Mapping, Source, TextMapSegment, pointer_child};
use quote::ToTokens;
use serde_json::{Map, Value};
use syn::{Expr, Fields, FnArg, ImplItem, Item, Pat, Stmt, Type, Variant, spanned::Spanned};

use crate::project::SourceFile;

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
mod tests;
