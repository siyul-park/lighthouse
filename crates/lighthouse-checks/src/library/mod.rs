//! The standard functions of CEL expressions over the code model:
//! `metrics(n)`, `callers(n)`, `owner(n)`, `tests(n)`, `rank(n, key)`, `exposed(n, internal)`,
//! `unit(func)`, `reach(n)`, `limit(n, max)` and `counted(n, count, what)`,
//! the module path helpers `globMatch(path, glob)` and
//! `layerOf(module, layers)`, and the text helpers `join`, `lines`, `trim`,
//! `trimPrefixes`, `trimSuffixes`, `trimLeft`, `trimRight`, `leadingRun`,
//! `drop` and `words`.
//!
//! A function reads a fact that was computed for the value `n` before the
//! expression ran, so an expression is pure and the facts are the same however
//! often it asks. Facts are computed only for the functions an expression
//! names; see [`Needs`].

use std::{collections::HashMap, sync::Arc};

use crate::glob;

mod limits;
use cel::{
    Context, ExecutionError, FunctionContext, ResolveResult, Value,
    common::{
        types::{CelBool, CelMap, CelMapKey, CelString},
        value::Val,
    },
    extractors::This,
    objects::Map,
};

/// The functions the library defines and the hidden field of a node each one
/// reads.
pub(crate) const FUNCTIONS: [(&str, &str); 7] = [
    ("metrics", "__metrics"),
    ("callers", "__callers"),
    ("owner", "__owner"),
    ("tests", "__tests"),
    ("rank", "__ranks"),
    ("unit", "__unit"),
    ("reach", "__reach"),
];

/// What the expressions of a check ask for, found from their source text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Needs {
    names: Vec<&'static str>,
    sources: Vec<String>,
}

impl Needs {
    pub(crate) fn of<'a>(sources: impl IntoIterator<Item = &'a str>) -> Self {
        let mut needs = Self {
            names: Vec::new(),
            sources: sources.into_iter().map(str::to_owned).collect(),
        };
        needs.names = FUNCTIONS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| needs.calls(name))
            .collect();
        needs
    }

    /// Whether some expression calls a function `name`, as a function or as a
    /// method of its first argument (`n.metrics()`).
    pub(crate) fn calls(&self, name: &str) -> bool {
        self.sources
            .iter()
            .any(|source| occurrences(source, name).any(|rest| rest.starts_with('(')))
    }

    /// Whether some expression mentions the fact `word`; the one way the
    /// builder asks, so that a fact without a declared reach is caught.
    pub(crate) fn fact(&self, word: &'static str) -> bool {
        debug_assert!(
            crate::reach::fact_declared(word),
            "fact `{word}` has no declared reach"
        );
        self.mentions(word)
    }

    /// Whether some expression calls a function of the library that reads a
    /// fact of the node; [`Needs::calls`], asked once for all expressions.
    pub(crate) fn function(&self, name: &str) -> bool {
        self.names.contains(&name)
    }

    /// Whether some expression mentions `word`, such as a fact field: as a
    /// whole identifier, so `role` is not found in `test_role`.
    pub(crate) fn mentions(&self, word: &str) -> bool {
        self.sources
            .iter()
            .any(|source| occurrences(source, word).next().is_some())
    }

    /// The single-quoted or double-quoted string literals of the expressions
    /// that contain a `/` (the shape of an order key id).
    pub(crate) fn keys(&self) -> Vec<String> {
        let mut found = Vec::new();
        for source in &self.sources {
            for quote in ['\'', '"'] {
                for (at, piece) in source.split(quote).enumerate() {
                    if at % 2 == 1 && piece.contains('/') && !piece.contains(' ') {
                        found.push(piece.to_owned());
                    }
                }
            }
        }
        found.sort();
        found.dedup();
        found
    }
}

/// A library function as the runtime calls it, over arguments it has already
/// resolved. Reading a field of the node argument in place spares converting
/// the whole node, with its lists of related nodes, on every call.
pub(super) type Builtin = Box<dyn Fn(&mut FunctionContext) -> ResolveResult + Send + Sync>;

/// A context with the library installed.
pub(crate) fn context() -> Context<'static> {
    let mut context = Context::default();
    for (name, field) in FUNCTIONS {
        if name != "rank" {
            context.add_function(name, field_reader(name, field));
        }
    }
    context.add_function("rank", rank());
    context.add_function("exposed", exposed());
    limits::install(&mut context);
    context.add_function("globMatch", |path: Arc<String>, glob: Arc<String>| {
        glob::matches(&path, &glob)
    });
    context.add_function("layerOf", layer_of);
    text_helpers(&mut context);
    context
}

/// The `N` arguments of a call, the receiver of a method call first.
pub(super) fn arguments<'a, const N: usize>(
    ftx: &'a FunctionContext,
) -> Result<[&'a dyn Val; N], ExecutionError> {
    let args: Vec<&dyn Val> = ftx
        .this
        .iter()
        .chain(&ftx.args)
        .map(AsRef::as_ref)
        .collect();
    <[&dyn Val; N]>::try_from(args)
        .map_err(|args| ExecutionError::invalid_argument_count(N, args.len()))
}

/// The field `field` of `node`; `None` when the node has no such field and
/// an error when it is no node.
pub(super) fn member<'a>(
    node: &'a dyn Val,
    field: &str,
    function: &str,
) -> Result<Option<&'a dyn Val>, ExecutionError> {
    let map = node
        .downcast_ref::<CelMap>()
        .ok_or_else(|| ExecutionError::function_error(function, "needs a node"))?;
    Ok(map
        .inner()
        .get(&CelMapKey::String(CelString::from(field)))
        .map(AsRef::as_ref))
}

pub(super) fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.to_string(),
        _ => String::new(),
    }
}

/// `layerOf(module, layers)`: the index of the first of the layers, each a
/// list of module globs, that holds a glob matching the module; -1 when none.
fn layer_of(module: Arc<String>, layers: Arc<Vec<Value>>) -> i64 {
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

/// `name(node)`: the hidden field `field` of the node.
fn field_reader(name: &'static str, field: &'static str) -> Builtin {
    Box::new(move |ftx| {
        let [node] = arguments(ftx)?;
        match member(node, field, name)? {
            Some(value) => Value::try_from(value),
            None => Ok(empty()),
        }
    })
}

/// `rank(node, key)`: the rank the order key `key` gives the node, `-1` when
/// the node has none.
fn rank() -> Builtin {
    Box::new(|ftx| {
        let [node, key] = arguments(ftx)?;
        let key = text(&Value::try_from(key)?);
        let Some(map) = member(node, "__ranks", "rank")?.and_then(|v| v.downcast_ref::<CelMap>())
        else {
            return Ok(Value::Int(-1));
        };
        match map
            .inner()
            .get(&CelMapKey::String(CelString::from(key.as_str())))
        {
            Some(value) => Value::try_from(value.as_ref()),
            None => Ok(Value::Int(-1)),
        }
    })
}

/// `exposed(node, internal)`: whether the node is public, or internal as well
/// when `internal` says so.
fn exposed() -> Builtin {
    Box::new(|ftx| {
        let [node, internal] = arguments(ftx)?;
        let internal = internal
            .downcast_ref::<CelBool>()
            .map(|b| *b.inner())
            .ok_or_else(|| ExecutionError::function_error("exposed", "needs a bool"))?;
        let visibility = match member(node, "visibility", "exposed") {
            Ok(Some(value)) => text(&Value::try_from(value)?),
            _ => String::new(),
        };
        Ok(Value::Bool(
            visibility == "public" || (internal && visibility == "internal"),
        ))
    })
}

/// What follows each occurrence of `word` in `source` that is not part of a
/// longer identifier.
fn occurrences<'a>(source: &'a str, word: &'a str) -> impl Iterator<Item = &'a str> {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    source.match_indices(word).filter_map(move |(at, _)| {
        let before = source[..at].chars().next_back();
        let rest = &source[at + word.len()..];
        let starts = !word.starts_with(ident) || !before.is_some_and(ident);
        let ends = !word.ends_with(ident) || !rest.chars().next().is_some_and(ident);
        (starts && ends).then_some(rest)
    })
}

/// The text helpers: what CEL's standard strings lack for reading a comment.
fn text_helpers(context: &mut Context<'static>) {
    context.add_function("lines", |s: Arc<String>| {
        Arc::new(
            s.lines()
                .map(|l| Value::String(Arc::new(l.to_owned())))
                .collect::<Vec<_>>(),
        )
    });
    context.add_function("trim", |s: Arc<String>| s.trim().to_owned());
    context.add_function(
        "join",
        |This(items): This<Arc<Vec<Value>>>, separator: Arc<String>| {
            items
                .iter()
                .map(text)
                .collect::<Vec<_>>()
                .join(separator.as_str())
        },
    );
    context.add_function("trimPrefixes", |s: Arc<String>, p: Arc<String>| {
        let mut rest = s.as_str();
        while !p.is_empty() {
            let Some(next) = rest.strip_prefix(p.as_str()) else {
                break;
            };
            rest = next;
        }
        rest.to_owned()
    });
    context.add_function("trimSuffixes", |s: Arc<String>, p: Arc<String>| {
        let mut rest = s.as_str();
        while !p.is_empty() {
            let Some(next) = rest.strip_suffix(p.as_str()) else {
                break;
            };
            rest = next;
        }
        rest.to_owned()
    });
    context.add_function("trimLeft", |s: Arc<String>, chars: Arc<String>| {
        s.trim_start_matches(|c| chars.contains(c)).to_owned()
    });
    context.add_function("trimRight", |s: Arc<String>, chars: Arc<String>| {
        s.trim_end_matches(|c| chars.contains(c)).to_owned()
    });
    context.add_function("leadingRun", |s: Arc<String>, chars: Arc<String>| {
        s.chars().take_while(|c| chars.contains(*c)).count() as i64
    });
    // The words of a name: of an identifier, or of the part of a decision id
    // after its pack.
    context.add_function("words", |s: Arc<String>| {
        let name = s.rsplit('/').next().unwrap_or_default();
        Arc::new(
            crate::text::words(name)
                .into_iter()
                .map(|w| Value::String(Arc::new(w)))
                .collect::<Vec<_>>(),
        )
    });
    context.add_function("drop", |s: Arc<String>, n: i64| {
        s.chars()
            .skip(usize::try_from(n).unwrap_or(0))
            .collect::<String>()
    });
}

fn empty() -> Value {
    Value::Map(Map::from(HashMap::<String, Value>::new()))
}
