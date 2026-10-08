//! The standard functions of CEL expressions over the code model:
//! `metrics(n)`, `callers(n)`, `callees(n)`, `edges(n, kind)`, `owner(n)`,
//! `tests(n)`, `annotations(n)`, `rank(n, key)` and `exposed(n, internal)`,
//! and the text helpers `lines`, `trim`, `trimPrefixes`, `trimSuffixes`,
//! `trimLeft`, `trimRight`, `leadingRun` and `drop`.
//!
//! A function reads a fact that was computed for the value `n` before the
//! expression ran, so an expression is pure and the facts are the same however
//! often it asks. Facts are computed only for the functions an expression
//! names; see [`Needs`].

use std::{collections::HashMap, sync::Arc};

use cel::{
    Context, ExecutionError, Value,
    objects::{Key, Map},
};

/// The functions the library defines and the hidden field of a node each one
/// reads.
pub(crate) const FUNCTIONS: [(&str, &str); 8] = [
    ("metrics", "__metrics"),
    ("callers", "__callers"),
    ("callees", "__callees"),
    ("owner", "__owner"),
    ("tests", "__tests"),
    ("annotations", "__annotations"),
    ("edges", "__edges"),
    ("rank", "__ranks"),
];

/// What the expressions of a check ask for, found from their source text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Needs {
    names: Vec<&'static str>,
    sources: Vec<String>,
}

impl Needs {
    pub(crate) fn of<'a>(sources: impl IntoIterator<Item = &'a str>) -> Self {
        let sources: Vec<String> = sources.into_iter().map(str::to_owned).collect();
        let names = FUNCTIONS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| {
                let call = format!("{name}(");
                sources.iter().any(|source| calls(source, &call))
            })
            .collect();
        Self { names, sources }
    }

    /// Whether some expression calls library function `name`.
    pub(crate) fn function(&self, name: &str) -> bool {
        self.names.contains(&name)
    }

    /// Whether some expression mentions `word`, such as a fact field.
    pub(crate) fn mentions(&self, word: &str) -> bool {
        self.sources.iter().any(|source| source.contains(word))
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

/// A context with the library installed.
pub(crate) fn context() -> Context<'static> {
    let mut context = Context::default();
    for (name, field) in FUNCTIONS {
        if name == "edges" || name == "rank" {
            continue;
        }
        context.add_function(name, move |node: Value| read(&node, field));
    }
    context.add_function("edges", |node: Value, kind: Value| {
        let Value::Map(edges) = read(&node, "__edges")? else {
            return Ok(Value::List(Vec::new().into()));
        };
        Ok(lookup(&edges, &text(&kind)).unwrap_or_else(|| Value::List(Vec::new().into())))
    });
    context.add_function("rank", |node: Value, key: Value| {
        let Value::Map(ranks) = read(&node, "__ranks")? else {
            return Ok(Value::Int(-1));
        };
        Ok(lookup(&ranks, &text(&key)).unwrap_or(Value::Int(-1)))
    });
    context.add_function("exposed", |node: Value, internal: bool| {
        let visibility = match &node {
            Value::Map(map) => lookup(map, "visibility").map(|v| text(&v)),
            _ => None,
        }
        .unwrap_or_default();
        visibility == "public" || (internal && visibility == "internal")
    });
    text_helpers(&mut context);
    context
}

/// Whether `source` calls the function whose name and `(` are `call`, as a
/// function and not as the tail of a longer name.
fn calls(source: &str, call: &str) -> bool {
    source.match_indices(call).any(|(at, _)| {
        source[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '.'))
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
    context.add_function("drop", |s: Arc<String>, n: i64| {
        s.chars()
            .skip(usize::try_from(n).unwrap_or(0))
            .collect::<String>()
    });
}

fn read(node: &Value, field: &str) -> Result<Value, ExecutionError> {
    let Value::Map(map) = node else {
        return Err(ExecutionError::function_error(
            field.trim_start_matches('_'),
            "needs a node",
        ));
    };
    Ok(lookup(map, field).unwrap_or_else(empty))
}

fn lookup(map: &Map, key: &str) -> Option<Value> {
    map.map.get(&Key::String(Arc::new(key.to_owned()))).cloned()
}

fn empty() -> Value {
    Value::Map(Map::from(HashMap::<String, Value>::new()))
}

fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.to_string(),
        _ => String::new(),
    }
}
