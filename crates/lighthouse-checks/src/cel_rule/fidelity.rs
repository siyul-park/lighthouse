//! The query a tier-F decision translates to agrees with the CEL it came
//! from: both are run on random fact values generated for the fields the query
//! reads, and must give the same verdict.

use std::collections::BTreeMap;

use lighthouse_logic::{Const, Cq, Op, Operand, Path, Root, Ucq};
use lighthouse_model::{Options, RunScope};
use lighthouse_spec::CheckKind;
use serde_json::{Map, Value, json};

use super::{CelRule, Frame, Judged, Shared};
use crate::{eval::cel_fact, fact::Subject, manifest::rule_options};

const SAMPLES: usize = 300;
/// The fewest samples of a decision that must run for it to count as compared.
const MIN_COMPARED: usize = 30;

/// A small deterministic generator, so a failure is the same on every run.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n.max(1) as u64).unwrap_or(0)
    }
}

/// The shape of a fact value as the query reads it.
#[derive(Default)]
struct Shape {
    fields: BTreeMap<String, Shape>,
    values: Vec<Value>,
    /// Members of the list this is, shaped by the paths of its binder.
    members: Option<Box<Shape>>,
    /// How many members the list has, when the conjunction needs it.
    count: Option<usize>,
}

impl Shape {
    fn at(&mut self, fields: &[String]) -> &mut Shape {
        fields
            .iter()
            .fold(self, |shape, f| shape.fields.entry(f.clone()).or_default())
    }

    /// A value of this shape. With `lax`, fields are sometimes missing.
    fn make(&self, dice: &mut Dice, lax: bool) -> Value {
        // A container read both as a list and by its fields is a record that is
        // sometimes empty.
        let record = self.members.is_some() && !self.fields.is_empty();
        if record && self.count.map_or(dice.below(4) == 0, |n| n == 0) {
            return json!({});
        }
        // A path read by its fields and compared with a constant is sometimes
        // that constant (`module.parent == null`).
        if !self.fields.is_empty() && !self.values.is_empty() && dice.below(2) == 0 {
            return self.values[dice.below(self.values.len())].clone();
        }
        if let Some(members) = &self.members
            && self.fields.is_empty()
        {
            let count = self.count.unwrap_or_else(|| dice.below(3));
            let plain =
                members.fields.is_empty() && members.values.is_empty() && members.members.is_none();
            // Only its size is read: a list, a map or a text.
            if plain {
                return match dice.below(3) {
                    0 => Value::Array(vec![json!(0); count]),
                    1 => Value::Object((0..count).map(|i| (format!("k{i}"), json!(i))).collect()),
                    _ => json!("a".repeat(count)),
                };
            }
            return Value::Array((0..count).map(|_| members.make(dice, lax)).collect());
        }
        if !self.fields.is_empty() {
            let mut object = Map::new();
            for (name, shape) in &self.fields {
                if lax && dice.below(10) == 0 {
                    continue;
                }
                object.insert(name.clone(), shape.make(dice, lax));
            }
            return Value::Object(object);
        }
        let pool: Vec<&Value> = self.values.iter().collect();
        pool.get(dice.below(pool.len()))
            .map_or(json!(0), |v| (*v).clone())
    }
}

fn walk(options: &Value, fields: &[String]) -> Option<Value> {
    fields.iter().try_fold(options, |v, f| v.get(f)).cloned()
}

/// Values near a constant, of its type.
fn around(constant: &Value) -> Vec<Value> {
    match constant {
        Value::Number(n) => {
            let k = n.as_i64().unwrap_or(0);
            let f = k as f64;
            vec![
                json!(k - 1),
                json!(k),
                json!(k + 1),
                json!(0),
                json!(1000),
                json!(f),
                json!(f + 0.5),
                json!(f - 0.5),
            ]
        }
        Value::String(s) => vec![
            json!(s),
            json!(format!("{s}x")),
            json!(format!("x{s}")),
            json!(s.chars().take(1).collect::<String>()),
            json!("zz"),
            json!(""),
        ],
        Value::Array(items) => items.iter().flat_map(around).chain([json!("zz")]).collect(),
        other => vec![other.clone(), json!(!other.as_bool().unwrap_or(false))],
    }
}

fn constant(value: &Const) -> Value {
    match value {
        Const::Null => Value::Null,
        Const::Bool(b) => json!(b),
        Const::Int(i) => json!(i),
        Const::Str(s) => json!(s),
        Const::List(items) => Value::Array(items.iter().map(constant).collect()),
    }
}

/// Values that satisfy `op` against `given`: what makes one comparison true.
fn satisfying(op: Op, given: &Value, offset: i64) -> Vec<Value> {
    let int = given.as_i64();
    let text = given.as_str().unwrap_or_default();
    let found = match op {
        Op::Eq => vec![given.clone()],
        Op::Ne if given.is_null() => vec![json!({"x": 1})],
        Op::Ne => around(given).into_iter().filter(|v| v != given).collect(),
        Op::Lt => int.map(|k| json!(k - 1)).into_iter().collect(),
        Op::Le | Op::Ge => vec![given.clone()],
        Op::Gt => int.map(|k| json!(k + 1)).into_iter().collect(),
        Op::In => given.as_array().cloned().unwrap_or_default(),
        Op::NotIn => vec![json!("zz")],
        Op::Starts => vec![json!(format!("{text}x"))],
        Op::Ends => vec![json!(format!("x{text}"))],
        Op::Contains => vec![json!(format!("x{text}x"))],
        Op::NotStarts | Op::NotEnds | Op::NotContains => vec![json!("zz")],
    };
    if offset == 0 {
        return found;
    }
    found
        .iter()
        .filter_map(Value::as_i64)
        .map(|n| json!(n - offset))
        .collect()
}

/// The shapes of the subject and the file a query reads. With `only`, the
/// values of the shape satisfy that conjunction, which a random value rarely
/// does.
fn shapes(ucq: &Ucq, options: &Value, only: Option<&Cq>) -> (Shape, Shape) {
    let (mut subject, mut file) = (Shape::default(), Shape::default());
    for cq in &ucq.cqs {
        place(
            cq,
            options,
            only.is_some_and(|o| std::ptr::eq(o, cq)),
            &mut subject,
            &mut file,
        );
    }
    (subject, file)
}

fn locate<'s>(
    path: &Path,
    lists: &BTreeMap<usize, &Path>,
    subject: &'s mut Shape,
    file: &'s mut Shape,
) -> &'s mut Shape {
    match path.root {
        Root::Subject => subject.at(&path.fields),
        Root::File => file.at(&path.fields),
        Root::Binder(id) => {
            let node = locate(lists[&id], lists, subject, file);
            node.members
                .get_or_insert_with(Default::default)
                .at(&path.fields)
        }
    }
}

fn place(cq: &Cq, options: &Value, satisfy: bool, subject: &mut Shape, file: &mut Shape) {
    // A binder's list: the path of its atom.
    let lists: BTreeMap<usize, &Path> = cq.atoms.iter().map(|a| (a.binder, &a.list)).collect();
    for atom in &cq.atoms {
        let node = locate(&atom.list, &lists, subject, file);
        node.members.get_or_insert_with(Default::default);
        if satisfy {
            node.count = Some(1);
        }
    }
    for (paths, count) in [(&cq.empties, 0), (&cq.filled, 1)] {
        for path in paths {
            let node = locate(path, &lists, subject, file);
            node.members.get_or_insert_with(Default::default);
            if satisfy {
                node.count = Some(count);
            }
        }
    }
    for cmp in &cq.cmps {
        let (Operand::Path(path), other) = (&cmp.left, &cmp.right) else {
            continue;
        };
        let given = match other {
            Operand::Const(c) => Some(constant(c)),
            Operand::Option(fields) => walk(options, fields),
            Operand::Path(_) => None,
        };
        let Some(given) = given else { continue };
        let node = locate(path, &lists, subject, file);
        if satisfy {
            node.values.extend(satisfying(cmp.op, &given, path.offset));
            continue;
        }
        let mut values = around(&given);
        if path.offset != 0 {
            values = values
                .iter()
                .filter_map(Value::as_i64)
                .map(|n| json!(n - path.offset))
                .collect();
        }
        if matches!(cmp.op, Op::Eq | Op::Ne) && given.is_null() {
            values.push(json!({"x": 1}));
        }
        node.values.extend(values);
    }
}

/// The options moved one way: numbers by one, flags flipped, a list gaining or
/// losing an item and a text growing or shrinking.
fn moved(options: &Map<String, Value>, up: bool) -> Value {
    let step = |v: &Value| match v {
        Value::Number(n) if n.is_i64() => json!(n.as_i64().unwrap_or(0) + if up { 1 } else { -1 }),
        Value::Bool(b) => json!(!b),
        Value::Array(items) => {
            let mut items = items.clone();
            if up {
                items.push(json!("zz"));
            } else if !items.is_empty() {
                items.remove(0);
            }
            Value::Array(items)
        }
        Value::String(text) => {
            let mut text = text.clone();
            if up {
                text.push('x');
            } else {
                text.pop();
            }
            json!(text)
        }
        other => other.clone(),
    };
    Value::Object(options.iter().map(|(k, v)| (k.clone(), step(v))).collect())
}

#[test]
fn the_query_of_every_translated_decision_agrees_with_its_cel() {
    let comp = crate::compose(&lighthouse_test_support::libraries()).unwrap();
    let bundled = comp.bundled().unwrap();
    let (mut translated, mut rejected, mut compared) =
        (Vec::new(), BTreeMap::<String, String>::new(), 0usize);
    for decision in bundled.decisions() {
        let Some(CheckKind::Cel(check)) = decision.check.as_ref().map(|c| &c.kind) else {
            continue;
        };
        let Some(select) = check.selects(decision.scope.subject) else {
            continue;
        };
        let id = decision.id().to_owned();
        let ucq = match lighthouse_logic::translate(comp.cel(), check, select.variable()) {
            Ok(ucq) => ucq,
            Err(reject) => {
                rejected.insert(id, reject.0);
                continue;
            }
        };
        let rule = CelRule::new(&comp, decision, check).unwrap();
        let languages: Vec<Option<String>> = match decision.languages.keys().next() {
            None => vec![None],
            Some(_) => decision.languages.keys().cloned().map(Some).collect(),
        };
        let trivial = ucq.cqs.is_empty() || ucq.cqs.iter().any(|cq| *cq == Cq::default());
        let mut witnessed = vec![false; ucq.cqs.len()];
        let mut checked = 0;
        let mut seen = [false, false];
        for language in &languages {
            let defaults = rule_options(decision, &Options::default(), language.as_deref())
                .unwrap_or_else(|e| panic!("{id}: {e}"));
            let sets = [
                Value::Object(defaults.clone()),
                moved(&defaults, true),
                moved(&defaults, false),
            ];
            for (round, options) in sets.iter().enumerate() {
                let mut dice = Dice(0x9E37_79B9_7F4A_7C15 ^ (round as u64 + 1));
                for at in 0..SAMPLES {
                    // Half of the samples are built to satisfy one conjunction.
                    let only = (at % 2 == 1 && !ucq.cqs.is_empty())
                        .then(|| &ucq.cqs[dice.below(ucq.cqs.len())]);
                    let lax = at % 5 == 4;
                    let (subject, file) = shapes(&ucq, options, only);
                    let (s, f) = (subject.make(&mut dice, lax), file.make(&mut dice, lax));
                    let expected = judge(&rule, &id, select.variable(), &s, &f, options);
                    let Some(expected) = expected else { continue };
                    checked += 1;
                    seen[usize::from(expected)] = true;
                    assert_eq!(
                        ucq.holds(&s, &f, options),
                        expected,
                        "{id}: the query and the CEL differ on subject {s} file {f} options {options}\n{ucq}"
                    );
                    if expected {
                        for (at, cq) in ucq.cqs.iter().enumerate() {
                            let one = Ucq {
                                cqs: vec![cq.clone()],
                                binders: ucq.binders,
                            };
                            witnessed[at] |= one.holds(&s, &f, options);
                        }
                    }
                }
            }
        }
        assert!(
            checked >= MIN_COMPARED,
            "{id}: only {checked} samples ran in the CEL; the generator does not reach its fields\n{ucq}"
        );
        assert!(
            trivial || seen == [true, true],
            "{id} {seen:?} {checked}: the samples reached only one verdict, so they cannot tell the query from the CEL\n{ucq}"
        );
        assert!(
            trivial || witnessed.iter().all(|w| *w),
            "{id} {witnessed:?}: a conjunction was never the witness of a violation\n{ucq}"
        );
        compared += checked;
        translated.push(id);
    }
    eprintln!("translated {}: {translated:?}", translated.len());
    eprintln!("rejected {}: {rejected:#?}", rejected.len());
    assert!(!translated.is_empty() && compared > 0);
}

#[test]
fn the_forms_production_parses_are_translated() {
    let comp = crate::compose(&lighthouse_test_support::libraries()).unwrap();
    let program = comp.cel().compile("cel.bind(k, symbol.n, k > 2)").unwrap();
    // `cel.bind` is a macro in the composed environment, not a call.
    assert!(matches!(
        program.expression().expr,
        cel::common::ast::Expr::Comprehension(_)
    ));
    let check = |condition: &str| -> lighthouse_spec::CelCheck {
        serde_json::from_value(json!({"where": condition, "message": "m"})).unwrap()
    };

    let bound =
        lighthouse_logic::translate(comp.cel(), &check("cel.bind(k, symbol.n, k > 2)"), "symbol");
    let regex = lighthouse_logic::translate(
        comp.cel(),
        &check("regex.matches(symbol.name, 'a')"),
        "symbol",
    );

    assert!(
        bound
            .unwrap()
            .holds(&json!({"n": 3}), &json!(null), &json!({}))
    );
    assert_eq!(regex.unwrap_err().0, "the call `regex.matches`");
}

/// What the CEL says of the subject, `None` when it fails on it (an
/// execution error is no verdict).
fn judge(
    rule: &CelRule,
    id: &str,
    subject: &str,
    value: &Value,
    file: &Value,
    options: &Value,
) -> Option<bool> {
    let shared = Shared::new(rule, RunScope::File, cel_fact(options));
    // A check that selects files reads the file it judges as `file`.
    let file = if subject == "file" { value } else { file };
    let _ = shared.file.set(cel_fact(file));
    let judged = Judged::new(&shared, Subject::from(cel_fact(value)));
    let mut frame = Frame::new(id, &rule.library);
    frame.share(&judged);
    frame.truth(&rule.condition).ok()
}
