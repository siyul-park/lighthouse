//! Reads one fact the way a check's expression would, for the tests that hold
//! the result cache to what the facts depend on. Not part of the API.

use lighthouse_model::ucm::{File, Project, Symbol};
use lighthouse_plugin::{Context, Error};
use serde_json::{Map, Value};

use lighthouse_spec::Cel;

use crate::{Composition, builder::Builder, facts, library::Needs};

/// The value of `fact` of `symbol`, as an expression that mentions it sees it.
pub fn symbol_fact(
    cel: &Cel,
    ctx: &Context,
    symbol: &Symbol,
    fact: &'static str,
) -> Result<Value, Error> {
    let needs = Needs::of(cel, [format!("x.{fact}").as_str()], []);
    let options = Map::new();
    let builder = Builder::new(ctx, &needs, &options)?;
    Ok(builder.symbol(symbol)[fact].clone())
}

/// The value of `fact` of `file`, whose text is `text`.
pub fn file_fact(
    cel: &Cel,
    ctx: &Context,
    file: &File,
    text: &str,
    fact: &'static str,
) -> Result<Value, Error> {
    let needs = Needs::of(cel, [format!("x.{fact}").as_str()], []);
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

/// The libraries (id and version) a decision's expressions call, and its
/// check revision with `versions` standing in for the versions of those
/// libraries: what the stored findings of the rule depend on.
pub fn revision(
    comp: &Composition,
    decision: &lighthouse_spec::Decision,
    versions: impl Fn(&str) -> Option<String>,
) -> Result<(std::collections::BTreeMap<String, String>, String), String> {
    let rule = crate::check::Check::new(comp, decision)
        .map_err(|e| e.to_string())?
        .ok_or("the decision has no program check")?;
    let used = rule.libraries();
    let shown: std::collections::BTreeMap<String, String> = used
        .iter()
        .map(|(id, version)| (id.clone(), versions(id).unwrap_or_else(|| version.clone())))
        .collect();
    let revision = crate::check::revision(decision, &shown).map_err(|e| e.to_string())?;
    Ok((used, revision))
}
