//! Reads one fact the way a check's expression would, for the tests that hold
//! the result cache to what the facts depend on. Not part of the API.

use lighthouse_model::ucm::{File, Project, Symbol};
use lighthouse_plugin::{Context, Error};
use serde_json::{Map, Value};

use crate::{builder::Builder, facts, library::Needs};

/// The value of `fact` of `symbol`, as an expression that mentions it sees it.
pub fn symbol_fact(ctx: &Context, symbol: &Symbol, fact: &'static str) -> Result<Value, Error> {
    let needs = Needs::of([format!("x.{fact}").as_str()], []);
    let options = Map::new();
    let builder = Builder::new(ctx, &needs, &options)?;
    Ok(builder.symbol(symbol)[fact].clone())
}

/// The value of `fact` of `file`, whose text is `text`.
pub fn file_fact(
    ctx: &Context,
    file: &File,
    text: &str,
    fact: &'static str,
) -> Result<Value, Error> {
    let needs = Needs::of([format!("x.{fact}").as_str()], []);
    let options = Map::new();
    let builder = Builder::new(ctx, &needs, &options)?;
    Ok(builder.file(file, text)[fact].clone())
}

/// The fields every node of a list carries about a symbol.
pub fn node_fields(project: &Project, symbol: &Symbol) -> Vec<String> {
    facts::node(project, symbol)
        .as_object()
        .map(|fields| fields.keys().cloned().collect())
        .unwrap_or_default()
}
