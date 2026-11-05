//! The facts of one subject, as the value of a node of the graph: a tree that
//! encodes and hashes by its content, and the form the CEL runtime holds it
//! in, made once for the node and not for each judgment.

use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

use cel::common::{types::CelNull, value::Val};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use lighthouse_plugin::Reads;

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
/// The sort of a comment: its id is the file and the place among the file's
/// comments. The sort of file is not here: its facts hold the directives, which
/// the engine judges after the rules, so they are made for each run.
pub const COMMENT: &str = "comment";

/// What a held fact without a tree reads as.
const NOTHING: CelNull = CelNull;

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

    /// The fact as a variable of the runtime, made when first read and held
    /// by the node: what judges it borrows it, and copies nothing.
    pub(crate) fn val(&self) -> Option<&dyn Val> {
        self.var
            .get_or_init(|| {
                self.fact
                    .as_ref()
                    .map(|f| Var::try_from(f.cel()).unwrap_or_else(|_| Box::new(CelNull)))
            })
            .as_deref()
    }
}

/// The facts of the subjects of one sort in one file, by id, as one node of
/// the graph holds them.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Facts(Vec<(String, Built)>);

impl Facts {
    /// The facts of subjects, which are put in order of their ids.
    pub fn of(mut subjects: Vec<(String, Built)>) -> Self {
        subjects.sort_by(|a, b| a.0.cmp(&b.0));
        Self(subjects)
    }

    /// The place of the subject `id`, if the file has one.
    pub(crate) fn position(&self, id: &str) -> Option<usize> {
        self.0.binary_search_by(|(at, _)| at.as_str().cmp(id)).ok()
    }

    /// The facts of the subject at `place`.
    pub(crate) fn at(&self, place: usize) -> Option<&Built> {
        self.0.get(place).map(|(_, built)| built)
    }

    /// The facts of the subject `id`.
    #[cfg(test)]
    pub fn get(&self, id: &str) -> Option<&Built> {
        self.at(self.position(id)?)
    }
}

/// What a judgment reads as its selected value: one it made, or the facts of
/// a subject a node of the graph holds.
pub(crate) enum Subject {
    Made(Var),
    Held(Arc<Facts>, usize),
}

impl Subject {
    /// The value, as the runtime reads it.
    pub(crate) fn val(&self) -> &dyn Val {
        match self {
            Self::Made(var) => var.as_ref(),
            Self::Held(facts, place) => facts.at(*place).and_then(Built::val).unwrap_or(&NOTHING),
        }
    }
}

impl From<Var> for Subject {
    fn from(var: Var) -> Self {
        Self::Made(var)
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

/// The key of the facts of the subjects of the sort `sort` in the file `file`
/// for what the rules of the class `class` read: see [`class_of`].
pub fn fact_key(sort: &str, class: &str, file: &str) -> String {
    subject_key(sort, &format!("{class}\0{file}"))
}

/// The sort, the class and the file a [`fact_key`] names.
pub fn fact_of(key: &str) -> (&str, &str, &str) {
    let mut parts = key.splitn(3, '\0');
    (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    )
}

/// The name of what rules read of a sort of subject: rules that read the same
/// are of one class and share the facts they are made of, and a rule reads
/// the same fields whatever the other rules read.
pub fn class_of(reads: &Reads) -> String {
    let bytes = serde_json::to_vec(reads).unwrap_or_default();
    blake3::hash(&bytes).to_hex()[..16].to_owned()
}
