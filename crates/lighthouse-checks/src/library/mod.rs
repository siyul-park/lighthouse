//! The functions of CEL expressions that the model needs besides the standard
//! library and the official extensions (see `lighthouse_spec`'s `ext`): the
//! module path helpers `path.matchesGlob(pattern)` and
//! `module.layerIndex(layers)`. What the code model says is a field of a
//! node, built only for the expressions that read it; see [`Needs`].

use std::sync::Arc;

use cel::{
    Context, Value,
    extractors::{IntoFunction, This},
};

use crate::glob;

mod needs;
pub(crate) use needs::Needs;

/// A context of the shared CEL environment with the library installed.
pub(crate) fn context() -> Result<Context<'static, 'static>, String> {
    let mut context = lighthouse_spec::context()?;
    declare(
        &mut context,
        "matchesGlob",
        |This(path): This<Arc<String>>, pattern: Arc<String>| glob::matches(&path, &pattern),
    )?;
    declare(&mut context, "layerIndex", layer_index)?;
    Ok(context)
}

/// Declares `function` as `name`, or says why the environment refuses it.
fn declare<T: 'static, F>(
    context: &mut Context<'static, 'static>,
    name: &str,
    function: F,
) -> Result<(), String>
where
    F: IntoFunction<T> + 'static + Send + Sync,
{
    context
        .add_function(name, function)
        .map_err(|e| format!("the CEL function `{name}`: {e}"))
}

/// `module.layerIndex(layers)`: the index of the first of the layers, each a
/// list of module globs, that holds a glob matching the module; -1 when none.
fn layer_index(This(module): This<Arc<String>>, layers: Arc<Vec<Value>>) -> i64 {
    let globs: Vec<Vec<String>> = layers.iter().map(strings).collect();
    glob::layer_of(
        &module,
        globs.iter().map(|layer| layer.iter().map(String::as_str)),
    )
}

/// The strings of a list value; anything else in it is skipped.
fn strings(value: &Value) -> Vec<String> {
    match value {
        Value::List(items) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}
