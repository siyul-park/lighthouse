//! The facts of one subject, as the value of a node of the graph: a tree that
//! encodes and hashes by its content, and the form the CEL runtime holds it
//! in, made once for the node and not for each judgment.

use std::{collections::BTreeMap, sync::OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::eval::Var;

/// The kind of the facts of a subject, which [`lighthouse_plugin::Queries`]
/// answers by name.
pub const FACT: &str = "fact";

/// The sorts of subject a fact is about.
pub const SYMBOL: &str = "symbol";
/// The sort of a function.
pub const FUNCTION: &str = "function";
/// The sort of a test.
pub const TEST: &str = "test";

/// The value of a fact node: the tree, or none when the subject has no fact
/// of this sort, and the tree in the form the runtime holds it, made when
/// first read.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Built {
    fact: Option<Fact>,
    #[serde(skip)]
    var: OnceLock<Option<Var>>,
}

/// What a rule's expression sees of a subject: a value of the language of
/// expressions, held as a tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Fact {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    List(Vec<Fact>),
    Map(BTreeMap<String, Fact>),
}

impl From<&Value> for Fact {
    /// Whole numbers are `int` however they were written, so that arithmetic
    /// between a fact and a literal never meets a `uint`.
    fn from(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(b) => Self::Bool(*b),
            Value::Number(n) => n
                .as_i64()
                .map(Self::Int)
                .or_else(|| n.as_f64().map(Self::Float))
                .unwrap_or(Self::Null),
            Value::String(s) => Self::Text(s.clone()),
            Value::Array(items) => Self::List(items.iter().map(Self::from).collect()),
            Value::Object(map) => Self::Map(
                map.iter()
                    .map(|(k, v)| (k.clone(), Self::from(v)))
                    .collect(),
            ),
        }
    }
}

impl Fact {
    /// The tree as a value of the CEL runtime.
    fn cel(&self) -> cel::Value {
        use std::{collections::HashMap, sync::Arc};
        match self {
            Self::Null => cel::Value::Null,
            Self::Bool(b) => cel::Value::Bool(*b),
            Self::Int(i) => cel::Value::Int(*i),
            Self::Float(f) => cel::Value::Float(*f),
            Self::Text(s) => cel::Value::String(Arc::new(s.clone())),
            Self::List(items) => cel::Value::List(Arc::new(items.iter().map(Self::cel).collect())),
            Self::Map(map) => cel::Value::Map(cel::objects::Map::from(
                map.iter()
                    .map(|(k, v)| (k.clone(), v.cel()))
                    .collect::<HashMap<String, cel::Value>>(),
            )),
        }
    }

    /// The value as the JSON the builder wrote it in.
    pub(crate) fn json(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(b) => Value::Bool(*b),
            Self::Int(i) => Value::from(*i),
            Self::Float(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
            Self::Text(s) => Value::String(s.clone()),
            Self::List(items) => Value::Array(items.iter().map(Self::json).collect()),
            Self::Map(map) => {
                Value::Object(map.iter().map(|(k, v)| (k.clone(), v.json())).collect())
            }
        }
    }
}

impl Built {
    /// The fact of a subject, if it has one.
    pub fn of(fact: Option<Fact>) -> Self {
        Self {
            fact,
            var: OnceLock::new(),
        }
    }

    /// The tree.
    #[cfg(test)]
    pub fn fact(&self) -> Option<&Fact> {
        self.fact.as_ref()
    }

    /// A copy of the fact as a variable of the runtime.
    pub(crate) fn var(&self) -> Option<Var> {
        self.var
            .get_or_init(|| {
                self.fact.as_ref().map(|f| {
                    Var::try_from(f.cel()).unwrap_or_else(|_| Box::new(cel::common::types::CelNull))
                })
            })
            .as_ref()
            .map(|v| v.clone_as_boxed())
    }
}

/// The key of the fact of the subject `id` of the sort `subject`.
pub fn subject_key(subject: &str, id: &str) -> String {
    format!("{subject}\0{id}")
}

/// The sort and the id a [`subject_key`] names.
pub fn subject_of(key: &str) -> (&str, &str) {
    key.split_once('\0').unwrap_or_default()
}
