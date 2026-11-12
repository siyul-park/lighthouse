//! The evaluator of a [`Ucq`] over the JSON fact value of a subject: a
//! nested-loop join over the containers the atoms name.

use std::{borrow::Cow, cmp::Ordering};

use serde_json::Value;

use crate::ir::{Cmp, Const, Cq, Op, Operand, Path, Root, Ucq};

/// The values a query reads.
struct Facts<'v> {
    subject: &'v Value,
    file: &'v Value,
    options: &'v Value,
}

/// The members the atoms have bound, by binder: borrowed from the facts, or a
/// key of a map.
type Members<'a> = Vec<Option<Cow<'a, Value>>>;

impl Ucq {
    /// Whether the subject violates the decision. The query is false where the
    /// CEL gives no verdict (a path the facts lack, a comparison of values
    /// that do not compare); a run of the check reports those as incomplete.
    /// The one exception is a bound value that is read but fails in the CEL:
    /// the query is then true where the CEL errors. That is on the error side
    /// only, which a run reports as incomplete.
    pub fn holds(&self, subject: &Value, file: &Value, options: &Value) -> bool {
        let facts = Facts {
            subject,
            file,
            options,
        };
        let mut members: Members = vec![None; self.binders];
        self.cqs.iter().any(|cq| join(cq, 0, &facts, &mut members))
    }
}

fn join<'a>(cq: &Cq, at: usize, facts: &Facts<'a>, members: &mut Members<'a>) -> bool {
    let Some(atom) = cq.atoms.get(at) else {
        return cq
            .empties
            .iter()
            .all(|p| size(p, facts, members) == Some(0))
            && cq
                .filled
                .iter()
                .all(|p| size(p, facts, members).is_some_and(|n| n > 0))
            && cq.cmps.iter().all(|c| compares(c, facts, members));
    };
    let Some(items) = reach(&atom.list, facts, members).and_then(|c| elements(&c)) else {
        return false;
    };
    items.into_iter().any(|item| {
        members[atom.binder] = Some(item);
        join(cq, at + 1, facts, members)
    })
}

/// What `exists` and `filter` range over: the items of a list, the keys of a
/// map.
fn elements<'a>(container: &Cow<'a, Value>) -> Option<Vec<Cow<'a, Value>>> {
    match container {
        Cow::Borrowed(Value::Array(items)) => Some(items.iter().map(Cow::Borrowed).collect()),
        Cow::Borrowed(Value::Object(map)) => Some(
            map.keys()
                .map(|key| Cow::Owned(Value::String(key.clone())))
                .collect(),
        ),
        _ => None,
    }
}

fn reach<'a>(path: &Path, facts: &Facts<'a>, members: &Members<'a>) -> Option<Cow<'a, Value>> {
    let root = match path.root {
        Root::Subject => Cow::Borrowed(facts.subject),
        Root::File => Cow::Borrowed(facts.file),
        Root::Binder(id) => members.get(id)?.as_ref()?.clone(),
    };
    path.fields
        .iter()
        .try_fold(root, |value, field| match value {
            Cow::Borrowed(value) => value.get(field).map(Cow::Borrowed),
            Cow::Owned(value) => value.get(field).cloned().map(Cow::Owned),
        })
}

/// The size of a list, a map or a text; `None` when there is none to measure.
fn size(path: &Path, facts: &Facts, members: &Members) -> Option<usize> {
    match reach(path, facts, members)?.as_ref() {
        Value::Array(items) => Some(items.len()),
        Value::Object(map) => Some(map.len()),
        Value::String(text) => Some(text.chars().count()),
        _ => None,
    }
}

fn operand(side: &Operand, facts: &Facts, members: &Members) -> Option<Value> {
    match side {
        Operand::Const(value) => Some(json(value)),
        Operand::Option(fields) => fields
            .iter()
            .try_fold(facts.options, |value, field| value.get(field))
            .cloned(),
        Operand::Path(path) => {
            let value = reach(path, facts, members)?.into_owned();
            if path.offset == 0 {
                return Some(value);
            }
            value
                .as_i64()
                .and_then(|n| n.checked_add(path.offset))
                .map(Value::from)
        }
    }
}

fn json(value: &Const) -> Value {
    match value {
        Const::Null => Value::Null,
        Const::Bool(b) => Value::Bool(*b),
        Const::Int(i) => Value::from(*i),
        Const::Str(s) => Value::String(s.clone()),
        Const::List(items) => Value::Array(items.iter().map(json).collect()),
    }
}

/// The order of two numbers, as CEL compares them: integers as integers, an
/// integer and a double as doubles; two texts in order. Nothing else is
/// ordered.
fn order(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(x), Some(y)) => Some(x.cmp(&y)),
            _ => x.as_f64()?.partial_cmp(&y.as_f64()?),
        },
        _ => None,
    }
}

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(_), Value::Number(_)) => order(a, b) == Some(Ordering::Equal),
        _ => a == b,
    }
}

fn compares(cmp: &Cmp, facts: &Facts, members: &Members) -> bool {
    let (Some(left), Some(right)) = (
        operand(&cmp.left, facts, members),
        operand(&cmp.right, facts, members),
    ) else {
        return false;
    };
    match cmp.op {
        Op::Eq => same(&left, &right),
        Op::Ne => !same(&left, &right),
        Op::Lt | Op::Le | Op::Gt | Op::Ge => order(&left, &right).is_some_and(|o| match cmp.op {
            Op::Lt => o == Ordering::Less,
            Op::Le => o != Ordering::Greater,
            Op::Gt => o == Ordering::Greater,
            _ => o != Ordering::Less,
        }),
        Op::In | Op::NotIn => {
            let found = match &right {
                Value::Array(items) => items.iter().any(|i| same(&left, i)),
                Value::Object(map) => left.as_str().is_some_and(|key| map.contains_key(key)),
                _ => return false,
            };
            found == (cmp.op == Op::In)
        }
        Op::Starts | Op::NotStarts | Op::Ends | Op::NotEnds | Op::Contains | Op::NotContains => {
            let (Some(text), Some(part)) = (left.as_str(), right.as_str()) else {
                return false;
            };
            let found = match cmp.op {
                Op::Starts | Op::NotStarts => text.starts_with(part),
                Op::Ends | Op::NotEnds => text.ends_with(part),
                _ => text.contains(part),
            };
            found == matches!(cmp.op, Op::Starts | Op::Ends | Op::Contains)
        }
    }
}
