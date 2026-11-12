//! The evaluator of a [`Ucq`] over the JSON fact value of a subject: a
//! nested-loop join over the lists the atoms name.

use serde_json::Value;

use crate::ir::{Cmp, Const, Cq, Op, Operand, Path, Root, Ucq};

/// The values a query reads.
struct Facts<'v> {
    subject: &'v Value,
    file: &'v Value,
    options: &'v Value,
}

impl Ucq {
    /// Whether the subject violates the decision. A path that the facts lack
    /// makes its comparison false, as an execution error is no violation.
    pub fn holds(&self, subject: &Value, file: &Value, options: &Value) -> bool {
        let facts = Facts {
            subject,
            file,
            options,
        };
        let mut members: Vec<Option<&Value>> = vec![None; self.binders];
        self.cqs.iter().any(|cq| join(cq, 0, &facts, &mut members))
    }
}

fn join<'v>(cq: &Cq, at: usize, facts: &Facts<'v>, members: &mut Vec<Option<&'v Value>>) -> bool {
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
    let Some(Value::Array(items)) = reach(&atom.list, facts, members) else {
        return false;
    };
    items.iter().any(|item| {
        members[atom.binder] = Some(item);
        join(cq, at + 1, facts, members)
    })
}

fn reach<'v>(path: &Path, facts: &Facts<'v>, members: &[Option<&'v Value>]) -> Option<&'v Value> {
    let root = match path.root {
        Root::Subject => facts.subject,
        Root::File => facts.file,
        Root::Binder(id) => (*members.get(id)?)?,
    };
    path.fields
        .iter()
        .try_fold(root, |value, field| value.get(field))
}

/// The size of a list, a map or a text; `None` when there is none to measure.
fn size(path: &Path, facts: &Facts, members: &[Option<&Value>]) -> Option<usize> {
    match reach(path, facts, members)? {
        Value::Array(items) => Some(items.len()),
        Value::Object(map) => Some(map.len()),
        Value::String(text) => Some(text.chars().count()),
        _ => None,
    }
}

fn operand(side: &Operand, facts: &Facts, members: &[Option<&Value>]) -> Option<Value> {
    match side {
        Operand::Const(value) => Some(json(value)),
        Operand::Option(fields) => fields
            .iter()
            .try_fold(facts.options, |value, field| value.get(field))
            .cloned(),
        Operand::Path(path) => {
            let value = reach(path, facts, members)?;
            if path.offset == 0 {
                return Some(value.clone());
            }
            value.as_i64().map(|n| Value::from(n + path.offset))
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

fn same(a: &Value, b: &Value) -> bool {
    match (a.as_i64(), b.as_i64()) {
        (Some(x), Some(y)) => x == y,
        _ => match (a, b) {
            (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
            _ => a == b,
        },
    }
}

fn compares(cmp: &Cmp, facts: &Facts, members: &[Option<&Value>]) -> bool {
    let (Some(left), Some(right)) = (
        operand(&cmp.left, facts, members),
        operand(&cmp.right, facts, members),
    ) else {
        return false;
    };
    match cmp.op {
        Op::Eq => same(&left, &right),
        Op::Ne => !same(&left, &right),
        Op::Lt | Op::Le | Op::Gt | Op::Ge => match (left.as_i64(), right.as_i64()) {
            (Some(x), Some(y)) => match cmp.op {
                Op::Lt => x < y,
                Op::Le => x <= y,
                Op::Gt => x > y,
                _ => x >= y,
            },
            _ => false,
        },
        Op::In | Op::NotIn => match &right {
            Value::Array(items) => items.iter().any(|i| same(&left, i)) == (cmp.op == Op::In),
            _ => false,
        },
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
