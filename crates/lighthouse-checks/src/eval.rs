//! Evaluating the CEL of a check: compiled programs, message templates, and
//! the frame in which one selected value is judged.

use std::{collections::HashMap, sync::Arc};

use cel::{
    Context, Program,
    common::{
        types::{CelMap, CelMapKey, CelNull, CelString},
        value::Val,
    },
    context::VariableResolver,
};
use lighthouse_plugin::Error as PluginError;
use serde_json::{Map, Value};

use lighthouse_spec::TemplatePart;

use crate::Error;

/// A piece of a message template.
#[derive(Debug)]
enum Piece {
    Text(String),
    Hole(Program),
}

/// Text with `{{ cel }}` holes, compiled.
#[derive(Debug)]
pub(crate) struct Template(Vec<Piece>);

impl Template {
    pub(crate) fn compile(rule: &str, what: &str, text: &str) -> Result<Self, Error> {
        let parts = lighthouse_spec::parse_template(text)
            .map_err(|e| Error::invalid(rule, format!("{what}: {e}")))?;
        let mut pieces = Vec::new();
        for part in parts {
            pieces.push(match part {
                TemplatePart::Text(text) => Piece::Text(text),
                TemplatePart::Hole(source) => Piece::Hole(
                    lighthouse_spec::compile(&source)
                        .map_err(|e| Error::invalid(rule, format!("{what} `{source}`: {e}")))?,
                ),
            });
        }
        Ok(Self(pieces))
    }

    /// The programs of the holes.
    pub(crate) fn programs(&self) -> impl Iterator<Item = &Program> {
        self.0.iter().filter_map(|piece| match piece {
            Piece::Hole(program) => Some(program),
            Piece::Text(_) => None,
        })
    }

    /// The text with each hole replaced by what `run` makes of its program.
    pub(crate) fn render(
        &self,
        run: impl Fn(&Program) -> Result<cel::Value, PluginError>,
    ) -> Result<String, PluginError> {
        let mut out = String::new();
        for piece in &self.0 {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Hole(program) => out.push_str(&render(&run(program)?)),
            }
        }
        Ok(out)
    }
}

/// The variables of one judgment, over a context that has the library.
pub(crate) struct Frame<'a> {
    rule: &'a str,
    context: Context<'a, 'a>,
}

impl<'a> Frame<'a> {
    pub(crate) fn new(rule: &'a str, base: &'a Context<'static, 'static>) -> Self {
        Self {
            rule,
            context: base.new_inner_scope(),
        }
    }

    /// Lets the frame read `shared` as variables, without a copy.
    pub(crate) fn share(&mut self, shared: &'a dyn VariableResolver) {
        self.context.set_variable_resolver(shared);
    }

    pub(crate) fn set_fact(&mut self, name: &str, value: Fact) {
        self.context.add_variable_as_val(name, value);
    }

    pub(crate) fn run(&self, program: &Program) -> Result<cel::Value, PluginError> {
        program.execute(&self.context).map_err(|e| self.fail(e))
    }

    pub(crate) fn truth(&self, program: &Program) -> Result<bool, PluginError> {
        match self.run(program)? {
            cel::Value::Bool(b) => Ok(b),
            other => Err(self.fail(format!("`where` is {}, not a bool", render(&other)))),
        }
    }

    pub(crate) fn text(&self, template: &Template) -> Result<String, PluginError> {
        template.render(|program| self.run(program))
    }

    pub(crate) fn string(&self, program: &Program, what: &str) -> Result<String, PluginError> {
        match self.run(program)? {
            cel::Value::String(s) => Ok(s.to_string()),
            other => Err(self.fail(format!("`{what}` is {}, not text", render(&other)))),
        }
    }

    pub(crate) fn int(&self, program: &Program, what: &str) -> Result<i64, PluginError> {
        match self.run(program)? {
            cel::Value::Int(i) => Ok(i),
            cel::Value::UInt(u) => Ok(i64::try_from(u).unwrap_or(i64::MAX)),
            other => Err(self.fail(format!("`{what}` is {}, not a number", render(&other)))),
        }
    }

    pub(crate) fn evidence(&self, programs: &[(String, Program)]) -> Result<Value, PluginError> {
        let mut evidence = Map::new();
        for (name, program) in programs {
            let value = to_json(&self.run(program)?);
            if !value.is_null() {
                evidence.insert(name.clone(), value);
            }
        }
        Ok(Value::Object(evidence))
    }

    pub(crate) fn fail(&self, message: impl std::fmt::Display) -> PluginError {
        PluginError::Incomplete(format!("{}: execution error: {message}", self.rule))
    }
}

/// A variable as the CEL runtime holds it. Cloning one copies what it holds,
/// which is cheaper than converting a JSON value anew for every judgment.
pub(crate) type Fact = Box<dyn Val>;

/// Compiles `source`, naming `rule` and `what` in the error.
pub(crate) fn compile(rule: &str, what: &str, source: &str) -> Result<Program, Error> {
    lighthouse_spec::compile(source).map_err(|e| Error::invalid(rule, format!("{what}: {e}")))
}

/// Like [`compile`], the program shared with whoever compiled the same source:
/// for the variables of a pack, which many decisions name.
pub(crate) fn compile_shared(rule: &str, what: &str, source: &str) -> Result<Arc<Program>, Error> {
    lighthouse_spec::compile_shared(source)
        .map_err(|e| Error::invalid(rule, format!("{what}: {e}")))
}

/// Compiled `name: expression` pairs.
pub(crate) fn compile_all(
    rule: &str,
    what: &str,
    sources: impl IntoIterator<Item = (impl AsRef<str>, impl AsRef<str>)>,
) -> Result<Vec<(String, Program)>, Error> {
    sources
        .into_iter()
        .map(|(name, source)| {
            let name = name.as_ref();
            Ok((
                name.to_owned(),
                compile(rule, &format!("{what} `{name}`"), source.as_ref())?,
            ))
        })
        .collect()
}

pub(crate) fn render(value: &cel::Value) -> String {
    match value {
        cel::Value::String(s) => s.to_string(),
        cel::Value::Int(i) => i.to_string(),
        cel::Value::UInt(u) => u.to_string(),
        cel::Value::Float(f) => f.to_string(),
        cel::Value::Bool(b) => b.to_string(),
        cel::Value::Null => "null".to_owned(),
        other => to_json(other).to_string(),
    }
}

pub(crate) fn to_json(value: &cel::Value) -> Value {
    match value {
        cel::Value::String(s) => Value::String(s.to_string()),
        cel::Value::Int(i) => Value::from(*i),
        cel::Value::UInt(u) => Value::from(*u),
        cel::Value::Float(f) => Value::from(*f),
        cel::Value::Bool(b) => Value::Bool(*b),
        cel::Value::List(items) => Value::Array(items.iter().map(to_json).collect()),
        cel::Value::Map(map) => Value::Object(
            map.map
                .iter()
                .map(|(k, v)| (k.to_string(), to_json(v)))
                .collect(),
        ),
        _ => Value::Null,
    }
}

/// A JSON value as CEL sees it. Whole numbers are `int` however they were
/// written, so arithmetic between a fact and a literal never meets a `uint`.
pub(crate) fn cel_value(value: &Value) -> cel::Value {
    use std::{collections::HashMap, sync::Arc};
    match value {
        Value::Null => cel::Value::Null,
        Value::Bool(b) => cel::Value::Bool(*b),
        Value::Number(n) => n
            .as_i64()
            .map(cel::Value::Int)
            .or_else(|| n.as_f64().map(cel::Value::Float))
            .unwrap_or(cel::Value::Null),
        Value::String(s) => cel::Value::String(Arc::new(s.clone())),
        Value::Array(items) => cel::Value::List(Arc::new(items.iter().map(cel_value).collect())),
        Value::Object(map) => cel::Value::Map(cel::objects::Map::from(
            map.iter()
                .map(|(k, v)| (k.clone(), cel_value(v)))
                .collect::<HashMap<String, cel::Value>>(),
        )),
    }
}

/// A JSON value as a variable of the CEL runtime.
pub(crate) fn cel_fact(value: &Value) -> Fact {
    Fact::try_from(cel_value(value)).unwrap_or_else(|_| Box::new(CelNull))
}

/// A copy of the map `base` with the entries of `extra` set over it; a copy
/// of `base` itself when `base` is no map or `extra` is empty.
pub(crate) fn with_entries(base: &(dyn Val + 'static), extra: &Map<String, Value>) -> Fact {
    let Some(map) = base.downcast_ref::<CelMap>().filter(|_| !extra.is_empty()) else {
        return base.clone_as_boxed();
    };
    let mut entries: HashMap<CelMapKey, Fact> = map
        .inner()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone_as_boxed()))
        .collect();
    for (key, value) in extra {
        entries.insert(
            CelMapKey::String(CelString::from(key.clone())),
            cel_fact(value),
        );
    }
    Box::new(CelMap::from(entries))
}

/// A copy of the map `base` with the entry `name` set to `value`; `base`
/// itself when it is no map.
pub(crate) fn with_fact(base: Fact, name: &str, value: Fact) -> Fact {
    let Some(map) = base.downcast_ref::<CelMap>() else {
        return base;
    };
    let mut entries: HashMap<CelMapKey, Fact> = map
        .inner()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone_as_boxed()))
        .collect();
    entries.insert(CelMapKey::String(CelString::from(name.to_owned())), value);
    Box::new(CelMap::from(entries))
}
