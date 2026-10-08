//! Evaluating the CEL of a check: compiled programs, message templates, and
//! the frame in which one selected value is judged.

use cel::{Context, Program};
use lighthouse_plugin::Error as PluginError;
use serde_json::{Map, Value};

use lighthouse_spec::TemplatePart;

use crate::Error;

/// A piece of a message template.
#[derive(Debug)]
pub(crate) enum Piece {
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
                    Program::compile(&source)
                        .map_err(|e| Error::invalid(rule, format!("{what} `{source}`: {e}")))?,
                ),
            });
        }
        Ok(Self(pieces))
    }
}

/// The variables of one judgment, over a context that has the library.
pub(crate) struct Frame<'a> {
    rule: &'a str,
    context: Context<'a>,
}

impl<'a> Frame<'a> {
    pub(crate) fn new(rule: &'a str, base: &'a Context<'static>) -> Self {
        Self {
            rule,
            context: base.new_inner_scope(),
        }
    }

    pub(crate) fn set(&mut self, name: &str, value: &Value) -> Result<(), PluginError> {
        self.context.add_variable_from_value(name, cel_value(value));
        Ok(())
    }

    pub(crate) fn set_cel(&mut self, name: &str, value: cel::Value) {
        self.context.add_variable_from_value(name, value);
    }

    pub(crate) fn options(&mut self, options: &Map<String, Value>) -> Result<(), PluginError> {
        self.set("options", &Value::Object(options.clone()))
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
        let mut out = String::new();
        for piece in &template.0 {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Hole(program) => out.push_str(&render(&self.run(program)?)),
            }
        }
        Ok(out)
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
            evidence.insert(name.clone(), to_json(&self.run(program)?));
        }
        Ok(Value::Object(evidence))
    }

    pub(crate) fn fail(&self, message: impl std::fmt::Display) -> PluginError {
        PluginError::Incomplete(format!("{}: execution error: {message}", self.rule))
    }
}

/// Compiles `source`, naming `rule` and `what` in the error.
pub(crate) fn compile(rule: &str, what: &str, source: &str) -> Result<Program, Error> {
    Program::compile(source).map_err(|e| Error::invalid(rule, format!("{what}: {e}")))
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
