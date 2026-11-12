//! A cel check's `where`, with its `variables` inlined, as a [`Ucq`].
//!
//! Accepted: literals and `options.*`, field paths of the subject, the file
//! and the members of its child lists, comparisons of a field with a constant
//! (`x + c` folded into the constant), `in`, `startsWith`, `endsWith`,
//! `contains`, `&&`, `||`, `!`, `?:`, `cel.bind`, `exists` and `filter` read
//! as "some member", and `size(l)` read as "has a member" or "has none".
//! Everything else is a [`Reject`] naming the construct, so what widening the
//! fragment would buy can be counted.

mod formula;
mod reject;
#[cfg(test)]
mod tests;

use std::{cell::Cell, rc::Rc};

/// The calls that compare, and the operator each is.
const COMPARISONS: [(&str, Op); 9] = [
    (EQUALS, Op::Eq),
    (NOT_EQUALS, Op::Ne),
    (LESS, Op::Lt),
    (LESS_EQUALS, Op::Le),
    (GREATER, Op::Gt),
    (GREATER_EQUALS, Op::Ge),
    ("startsWith", Op::Starts),
    ("endsWith", Op::Ends),
    ("contains", Op::Contains),
];

/// The most steps of reading one decision: variables are inlined, so a
/// chain of variables that each use the one before twice grows by powers.
const MAX_STEPS: usize = 20_000;

use cel::{
    Program,
    common::ast::{
        ComprehensionExpr, Expr, IdedExpr, LiteralValue,
        operators::{
            ADD, CONDITIONAL, EQUALS, GREATER, GREATER_EQUALS, IN, LESS, LESS_EQUALS, LOGICAL_AND,
            LOGICAL_NOT, LOGICAL_OR, NEGATE, NOT_EQUALS, SUBSTRACT,
        },
    },
};
use lighthouse_spec::{CelCheck, Env, Variable};

use crate::ir::{Cmp, Const, Op, Operand, Path, Root, Ucq};
use formula::F;
pub use reject::{Construct, Reject};

type R<T> = Result<T, Reject>;

struct Node<'a> {
    name: &'a str,
    binding: Binding<'a>,
    parent: Scope<'a>,
}

/// What a name stands for while an expression is read.
#[derive(Clone)]
enum Binding<'a> {
    /// An expression, read in the scope it was written in.
    Value(&'a IdedExpr, Scope<'a>, Used),
    /// A variable with parameters, applied by name; its body starts from the
    /// roots (the file and the subject) and its parameters.
    Macro(&'a [String], &'a IdedExpr, Scope<'a>),
    /// What a path starts from: the subject, the file or a child member.
    Root(Root),
}

/// How a comprehension is written.
enum Shape<'a> {
    Bind(Scope<'a>, Rc<Cell<bool>>),
    Exists(usize, Path, F),
    Filter(usize, Path, F),
}

/// A body read with its names bound, and the values to check once it is read.
type Applied<'a> = (&'a IdedExpr, Scope<'a>, Vec<Rc<Cell<bool>>>);

type Scope<'a> = Option<Rc<Node<'a>>>;

/// Whether a bound value was read: a value the body never reads may fail in
/// the CEL, which the query would not.
type Used = Option<Rc<Cell<bool>>>;

struct Translator {
    next: Cell<usize>,
    steps: Cell<usize>,
}

/// A value that is a list.
enum Members {
    Path(Path),
    Literal(i64),
    Choice(Box<F>, Box<Members>, Box<Members>),
    Filter { binder: usize, list: Path, cond: F },
}

impl<'a> Translator {
    fn term(&self, e: &'a IdedExpr, scope: &Scope<'a>) -> R<Operand> {
        self.step()?;
        match &e.expr {
            Expr::Literal(value) => Ok(Operand::Const(Const::try_from(value)?)),
            Expr::Ident(name) => match lookup(scope, name) {
                Some(Binding::Value(x, inner, used)) => {
                    mark(&used);
                    self.term(x, &inner)
                }
                Some(Binding::Root(root)) => Ok(Operand::Path(Path::of(root))),
                Some(Binding::Macro(..)) => Err(Construct::Bare(name.to_owned()).into()),
                None if name == "options" => Ok(Operand::Option(Vec::new())),
                None => Err(Construct::PackVariable(name.to_owned()).into()),
            },
            Expr::Select(select) if !select.test => match self.term(&select.operand, scope)? {
                Operand::Path(path) if path.offset != 0 => Err(Construct::Arithmetic.into()),
                Operand::Path(mut path) => {
                    path.fields.push(select.field.clone());
                    Ok(Operand::Path(path))
                }
                Operand::Option(mut fields) => {
                    fields.push(select.field.clone());
                    Ok(Operand::Option(fields))
                }
                Operand::Const(_) => Err(Construct::ConstantField.into()),
            },
            Expr::List(list) => {
                let items = list
                    .elements
                    .iter()
                    .map(|item| match self.term(item, scope)? {
                        Operand::Const(value) => Ok(value),
                        _ => Err(Construct::ComputedList.into()),
                    })
                    .collect::<R<Vec<_>>>()?;
                Ok(Operand::Const(Const::List(items)))
            }
            Expr::Call(call) => self.term_call(
                call.func_name.as_str(),
                call.target.as_deref(),
                &call.args,
                scope,
            ),
            Expr::Comprehension(c) => match self.shape(c, scope)? {
                Shape::Bind(inner, used) => settled(&[used], self.term(&c.result, &inner)),
                _ => Err(Construct::ComputedComprehension.into()),
            },
            Expr::Select(_) => Err(Construct::Has.into()),
            _ => Err(Construct::Expression.into()),
        }
    }

    fn step(&self) -> R<()> {
        self.steps.set(self.steps.get() + 1);
        if self.steps.get() > MAX_STEPS {
            return Err(Construct::Expansion.into());
        }
        Ok(())
    }

    fn term_call(
        &self,
        name: &str,
        target: Option<&'a IdedExpr>,
        args: &'a [IdedExpr],
        scope: &Scope<'a>,
    ) -> R<Operand> {
        if let Some(read) = self.inlined(target, name, args, scope, |b, s| self.term(b, s)) {
            return read;
        }
        let sign: i64 = match name {
            ADD => 1,
            SUBSTRACT => -1,
            NEGATE => {
                return match args.first().map(|a| self.term(a, scope)) {
                    Some(Ok(Operand::Const(Const::Int(n)))) => {
                        n.checked_neg().map(Operand::int).ok_or_else(overflow)
                    }
                    _ => Err(Construct::NegatedField.into()),
                };
            }
            other => return Err(named(other, target)),
        };
        let [a, b] = args else {
            return Err(Construct::Arithmetic.into());
        };
        let shift = |k: i64| k.checked_mul(sign).ok_or_else(overflow);
        match (self.term(a, scope)?, self.term(b, scope)?) {
            (Operand::Const(Const::Int(x)), Operand::Const(Const::Int(y))) => x
                .checked_add(shift(y)?)
                .map(Operand::int)
                .ok_or_else(overflow),
            (Operand::Path(p), Operand::Const(Const::Int(k))) => {
                p.shifted(shift(k)?).map(Operand::Path).ok_or_else(overflow)
            }
            (Operand::Const(Const::Int(k)), Operand::Path(p)) if sign == 1 => {
                p.shifted(k).map(Operand::Path).ok_or_else(overflow)
            }
            (Operand::Const(Const::Str(_)), _) | (_, Operand::Const(Const::Str(_))) => {
                Err(Construct::Concatenation.into())
            }
            _ => Err(Construct::Arithmetic.into()),
        }
    }

    /// A call of `cel.bind` or of a variable with parameters: its body, read
    /// by `read` with the names bound, and checked for values never read.
    fn inlined<T>(
        &self,
        target: Option<&'a IdedExpr>,
        name: &str,
        args: &'a [IdedExpr],
        scope: &Scope<'a>,
        read: impl FnOnce(&'a IdedExpr, &Scope<'a>) -> R<T>,
    ) -> Option<R<T>> {
        let applied = match Self::bound(target, name, args, scope) {
            Some(bound) => Ok(bound),
            None if target.is_none() => self.apply(name, args, scope)?,
            None => return None,
        };
        Some(applied.and_then(|(body, inner, flags)| settled(&flags, read(body, &inner))))
    }

    /// The body of the variable `name` applied to `args`, with its scope.
    fn apply(&self, name: &str, args: &'a [IdedExpr], scope: &Scope<'a>) -> Option<R<Applied<'a>>> {
        let Some(Binding::Macro(params, body, roots)) = lookup(scope, name) else {
            return None;
        };
        if params.len() != args.len() {
            return Some(Err(Construct::Arity(name.to_owned()).into()));
        }
        let mut inner = roots;
        let mut flags = Vec::new();
        for (param, arg) in params.iter().zip(args) {
            let flag = Rc::new(Cell::new(false));
            flags.push(flag.clone());
            inner = bind(
                &inner,
                param,
                Binding::Value(arg, scope.clone(), Some(flag)),
            );
        }
        Some(Ok((body, inner, flags)))
    }

    /// `cel.bind(name, value, body)` written as a call: the body, with the
    /// name standing for the value.
    fn bound(
        target: Option<&'a IdedExpr>,
        name: &str,
        args: &'a [IdedExpr],
        scope: &Scope<'a>,
    ) -> Option<Applied<'a>> {
        let named = matches!(target.map(|t| &t.expr), Some(Expr::Ident(n)) if n == "cel");
        let [var, value, body] = args else {
            return None;
        };
        let Expr::Ident(var) = &var.expr else {
            return None;
        };
        (named && name == "bind").then(|| {
            let flag = Rc::new(Cell::new(false));
            let inner = bind(
                scope,
                var,
                Binding::Value(value, scope.clone(), Some(flag.clone())),
            );
            (body, inner, vec![flag])
        })
    }

    fn formula(&self, e: &'a IdedExpr, scope: &Scope<'a>) -> R<F> {
        self.step()?;
        match &e.expr {
            Expr::Literal(LiteralValue::Boolean(b)) => Ok(F::Lit(*b.inner())),
            Expr::Ident(name) => match lookup(scope, name) {
                Some(Binding::Value(x, inner, used)) => {
                    mark(&used);
                    self.formula(x, &inner)
                }
                _ => self.truth(e, scope),
            },
            Expr::Select(_) => self.truth(e, scope),
            Expr::Call(call) => self.call(
                call.func_name.as_str(),
                call.target.as_deref(),
                &call.args,
                scope,
            ),
            Expr::Comprehension(c) => match self.shape(c, scope)? {
                Shape::Bind(inner, used) => settled(&[used], self.formula(&c.result, &inner)),
                Shape::Exists(binder, list, cond) => Ok(F::Has {
                    binder,
                    list,
                    cond: Box::new(cond),
                }),
                Shape::Filter(..) => Err(Construct::ListCondition.into()),
            },
            _ => Err(Construct::Expression.into()),
        }
    }

    /// A boolean field or option, as a comparison with `true`.
    fn truth(&self, e: &'a IdedExpr, scope: &Scope<'a>) -> R<F> {
        let yes = Operand::Const(Const::Bool(true));
        compare(self.term(e, scope)?, Op::Eq, yes)
    }

    fn call(
        &self,
        name: &str,
        target: Option<&'a IdedExpr>,
        args: &'a [IdedExpr],
        scope: &Scope<'a>,
    ) -> R<F> {
        if let Some(read) = self.inlined(target, name, args, scope, |b, s| self.formula(b, s)) {
            return read;
        }
        let operands: Vec<&'a IdedExpr> = target.into_iter().chain(args).collect();
        match (name, operands.as_slice()) {
            (LOGICAL_AND, [a, b]) => Ok(F::And(vec![
                self.formula(a, scope)?,
                self.formula(b, scope)?,
            ])),
            (LOGICAL_OR, [a, b]) => Ok(F::Or(vec![
                self.formula(a, scope)?,
                self.formula(b, scope)?,
            ])),
            (LOGICAL_NOT, [a]) => self.negate(self.formula(a, scope)?),
            (CONDITIONAL, [c, a, b]) => {
                let c = self.formula(c, scope)?;
                self.choice(&c, self.formula(a, scope)?, self.formula(b, scope)?)
            }
            (IN, [a, b]) => {
                let (left, right) = (self.term(a, scope)?, self.term(b, scope)?);
                if matches!(right, Operand::Path(_)) {
                    return Err(Construct::Membership.into());
                }
                compare(left, Op::In, right)
            }
            (_, [a, b]) if let Some((_, op)) = COMPARISONS.iter().find(|(n, _)| *n == name) => {
                self.comparison(*op, a, b, scope)
            }
            (other, _) => Err(named(other, target)),
        }
    }

    /// `(c and a) or (not c and b)`.
    fn choice(&self, c: &F, a: F, b: F) -> R<F> {
        Ok(F::Or(vec![
            F::And(vec![c.clone(), a]),
            F::And(vec![self.negate(c.clone())?, b]),
        ]))
    }

    fn negate(&self, f: F) -> R<F> {
        match f {
            F::Lit(b) => Ok(F::Lit(!b)),
            F::Cmp(c) => Ok(F::Cmp(Cmp {
                op: c.op.negated(),
                ..c
            })),
            F::Empty(path) => Ok(F::Filled(path)),
            F::Filled(path) => Ok(F::Empty(path)),
            F::Has { list, cond, .. } if matches!(*cond, F::Lit(true)) => Ok(F::Empty(list)),
            F::Has { .. } => Err(Construct::NegatedExists.into()),
            F::And(parts) => Ok(F::Or(
                parts
                    .into_iter()
                    .map(|p| self.negate(p))
                    .collect::<R<_>>()?,
            )),
            F::Or(parts) => Ok(F::And(
                parts
                    .into_iter()
                    .map(|p| self.negate(p))
                    .collect::<R<_>>()?,
            )),
        }
    }

    fn comparison(&self, op: Op, a: &'a IdedExpr, b: &'a IdedExpr, scope: &Scope<'a>) -> R<F> {
        let (counted, other, op) = match (size_of(a), size_of(b)) {
            (Some(x), _) => (Some(x), b, op),
            (None, Some(x)) => (Some(x), a, op.flipped().ok_or(Construct::Size)?),
            (None, None) => (None, b, op),
        };
        if let Some(counted) = counted {
            let Operand::Const(Const::Int(k)) = self.term(other, scope)? else {
                return Err(Construct::Size.into());
            };
            return self.sized(self.members(counted, scope)?, op, k);
        }
        compare(self.term(a, scope)?, op, self.term(b, scope)?)
    }

    /// `size(list) op k` where only "has a member" and "has none" are
    /// conditions of conjunctive queries.
    fn sized(&self, list: Members, op: Op, k: i64) -> R<F> {
        let holds = |n: i64| match op {
            Op::Eq => n == k,
            Op::Ne => n != k,
            Op::Lt => n < k,
            Op::Le => n <= k,
            Op::Gt => n > k,
            _ => n >= k,
        };
        match list {
            Members::Literal(n) => Ok(F::Lit(holds(n))),
            Members::Choice(c, a, b) => {
                let (a, b) = (self.sized(*a, op, k)?, self.sized(*b, op, k)?);
                self.choice(&c, a, b)
            }
            Members::Path(path) => self.counted(
                F::Filled(path.clone()),
                || Ok(F::Empty(path)),
                [holds(0), holds(1), holds(2)],
            ),
            Members::Filter { binder, list, cond } => {
                let has = F::Has {
                    binder,
                    list,
                    cond: Box::new(cond),
                };
                let none = || self.negate(has.clone());
                self.counted(has.clone(), none, [holds(0), holds(1), holds(2)])
            }
        }
    }

    fn counted(&self, has: F, none: impl FnOnce() -> R<F>, holds: [bool; 3]) -> R<F> {
        match holds {
            [false, true, true] => Ok(has),
            [true, false, false] => none(),
            _ => Err(Construct::Count.into()),
        }
    }

    fn members(&self, e: &'a IdedExpr, scope: &Scope<'a>) -> R<Members> {
        self.step()?;
        match &e.expr {
            Expr::List(list) => {
                if list
                    .elements
                    .iter()
                    .any(|item| !matches!(item.expr, Expr::Literal(_)))
                {
                    return Err(Construct::ComputedList.into());
                }
                Ok(Members::Literal(
                    i64::try_from(list.elements.len()).map_err(|_| overflow())?,
                ))
            }
            Expr::Ident(name) => match lookup(scope, name) {
                Some(Binding::Value(x, inner, used)) => {
                    mark(&used);
                    self.members(x, &inner)
                }
                _ => self.members_path(e, scope),
            },
            Expr::Select(_) => self.members_path(e, scope),
            Expr::Call(call) if call.func_name == CONDITIONAL && call.args.len() == 3 => {
                Ok(Members::Choice(
                    Box::new(self.formula(&call.args[0], scope)?),
                    Box::new(self.members(&call.args[1], scope)?),
                    Box::new(self.members(&call.args[2], scope)?),
                ))
            }
            Expr::Call(call) => {
                let (target, name) = (call.target.as_deref(), call.func_name.as_str());
                match self.inlined(target, name, &call.args, scope, |b, s| self.members(b, s)) {
                    Some(read) => read,
                    None if target.is_none() => Err(named(name, None)),
                    None => Err(Construct::ComputedList.into()),
                }
            }
            Expr::Comprehension(c) => match self.shape(c, scope)? {
                Shape::Bind(inner, used) => settled(&[used], self.members(&c.result, &inner)),
                Shape::Filter(binder, list, cond) => Ok(Members::Filter { binder, list, cond }),
                Shape::Exists(..) => Err(Construct::ConditionList.into()),
            },
            _ => Err(Construct::ComputedList.into()),
        }
    }

    fn members_path(&self, e: &'a IdedExpr, scope: &Scope<'a>) -> R<Members> {
        match self.term(e, scope)? {
            Operand::Path(path) if path.offset != 0 => Err(Construct::Arithmetic.into()),
            Operand::Path(path) => Ok(Members::Path(path)),
            _ => Err(Construct::ComputedList.into()),
        }
    }

    /// Reads a comprehension as `cel.bind`, `exists` or `filter`.
    fn shape(&self, c: &'a ComprehensionExpr, scope: &Scope<'a>) -> R<Shape<'a>> {
        let literal = |e: &IdedExpr, want: bool| matches!(&e.expr, Expr::Literal(LiteralValue::Boolean(b)) if *b.inner() == want);
        if lighthouse_spec::is_empty_list(&c.iter_range) && literal(&c.loop_cond, false) {
            let flag = Rc::new(Cell::new(false));
            let inner = bind(
                scope,
                &c.accu_var,
                Binding::Value(&c.accu_init, scope.clone(), Some(flag.clone())),
            );
            return Ok(Shape::Bind(inner, flag));
        }
        let step = match &c.loop_step.expr {
            Expr::Call(call) => call,
            _ => return Err(Construct::Comprehension.into()),
        };
        let wanted = if literal(&c.accu_init, false) && step.func_name == LOGICAL_OR {
            step.args.get(1)
        } else if lighthouse_spec::is_empty_list(&c.accu_init) && step.func_name == CONDITIONAL {
            step.args.first()
        } else {
            let all = literal(&c.accu_init, true) && step.func_name == LOGICAL_AND;
            return Err(if all {
                Construct::All
            } else {
                Construct::Comprehension
            }
            .into());
        };
        let Some(cond) = wanted else {
            return Err(Construct::Comprehension.into());
        };
        let list = match self.term(&c.iter_range, scope)? {
            Operand::Path(list) if list.offset == 0 => list,
            Operand::Path(_) => {
                return Err(Construct::Arithmetic.into());
            }
            _ => {
                return Err(Construct::ComputedComprehension.into());
            }
        };
        let binder = self.fresh();
        let inner = bind(scope, &c.iter_var, Binding::Root(Root::Binder(binder)));
        let cond = self.formula(cond, &inner)?;
        Ok(if literal(&c.accu_init, false) {
            Shape::Exists(binder, list, cond)
        } else {
            Shape::Filter(binder, list, cond)
        })
    }

    fn fresh(&self) -> usize {
        let id = self.next.get();
        self.next.set(id + 1);
        id
    }
}

/// The query for `check`, whose selected value is the variable `subject`
/// (`symbol`, `func`, ...).
pub fn translate(env: &Env, check: &CelCheck, subject: &str) -> R<Ucq> {
    let programs: Vec<Program> = check
        .variables
        .iter()
        .map(|v| env.compile(&v.expression))
        .collect::<Result<_, _>>()
        .map_err(Construct::Compile)?;
    let condition = env.compile(&check.condition).map_err(Construct::Compile)?;
    let roots = bind(
        &bind(&None, "file", Binding::Root(Root::File)),
        subject,
        Binding::Root(Root::Subject),
    );
    let mut scope = roots.clone();
    for (variable, program) in check.variables.iter().zip(&programs) {
        scope = bind(
            &scope,
            &variable.name,
            binding(variable, program, &scope, &roots),
        );
    }
    let translator = Translator {
        next: Cell::new(0),
        steps: Cell::new(0),
    };
    let cqs = translator.formula(condition.expression(), &scope)?.dnf()?;
    Ok(Ucq {
        cqs,
        binders: translator.next.get(),
    })
}

fn mark(used: &Used) {
    if let Some(flag) = used {
        flag.set(true);
    }
}

/// `read`, unless a value it bound was never read.
fn settled<T>(flags: &[Rc<Cell<bool>>], read: R<T>) -> R<T> {
    let read = read?;
    if flags.iter().any(|f| !f.get()) {
        return Err(Construct::Unused.into());
    }
    Ok(read)
}

/// How a call is named in a rejection: by the construct, not the operator.
fn named(name: &str, target: Option<&IdedExpr>) -> Reject {
    match (name, target.map(|t| &t.expr)) {
        ("_?_:_", _) => Construct::Ternary,
        ("_+_", _) => Construct::Concatenation,
        ("_-_" | "-_" | "_*_" | "_/_" | "_%_", _) => Construct::Arithmetic,
        ("_[_]" | "_[?_]", _) => Construct::Index,
        ("@in", _) => Construct::In,
        (_, Some(Expr::Ident(space))) => Construct::Call(format!("{space}.{name}")),
        _ => Construct::Call(name.to_owned()),
    }
    .into()
}

fn bind<'a>(scope: &Scope<'a>, name: &'a str, binding: Binding<'a>) -> Scope<'a> {
    Some(Rc::new(Node {
        name,
        binding,
        parent: scope.clone(),
    }))
}

fn lookup<'a>(scope: &Scope<'a>, name: &str) -> Option<Binding<'a>> {
    let mut at = scope.as_ref();
    while let Some(node) = at {
        if node.name == name {
            return Some(node.binding.clone());
        }
        at = node.parent.as_ref();
    }
    None
}

fn binding<'a>(
    variable: &'a Variable,
    program: &'a Program,
    scope: &Scope<'a>,
    roots: &Scope<'a>,
) -> Binding<'a> {
    if variable.params.is_empty() {
        Binding::Value(program.expression(), scope.clone(), None)
    } else {
        Binding::Macro(&variable.params, program.expression(), roots.clone())
    }
}

/// The argument of `size(x)` or `x.size()`.
fn size_of(e: &IdedExpr) -> Option<&IdedExpr> {
    let Expr::Call(call) = &e.expr else {
        return None;
    };
    if call.func_name != "size" {
        return None;
    }
    match (&call.target, call.args.as_slice()) {
        (None, [x]) => Some(x),
        (Some(x), []) => Some(x),
        _ => None,
    }
}

/// `left op right` as a condition: a field is compared with a constant, and a
/// comparison of two fields is outside the fragment.
fn compare(mut left: Operand, mut op: Op, mut right: Operand) -> R<F> {
    match (&left, &right) {
        (Operand::Path(_), Operand::Path(_)) => {
            return Err(Construct::TwoFields.into());
        }
        (_, Operand::Path(_)) => {
            op = op.flipped().ok_or(Construct::Membership)?;
            std::mem::swap(&mut left, &mut right);
        }
        _ => {}
    }
    if let Operand::Path(path) = &mut left
        && path.offset != 0
    {
        match &mut right {
            Operand::Const(Const::Int(k)) => {
                *k = k.checked_sub(path.offset).ok_or_else(overflow)?;
                path.offset = 0;
            }
            Operand::Option(_) if !matches!(op, Op::In | Op::NotIn) => {}
            _ => {
                return Err(Construct::Arithmetic.into());
            }
        }
    }
    if matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge)
        && matches!(&right, Operand::Const(c) if !matches!(c, Const::Int(_) | Const::Str(_)))
    {
        return Err(Construct::Order.into());
    }
    Ok(F::Cmp(Cmp { left, op, right }))
}

fn overflow() -> Reject {
    Construct::Overflow.into()
}

impl TryFrom<&LiteralValue> for Const {
    type Error = Reject;

    fn try_from(value: &LiteralValue) -> R<Self> {
        match value {
            LiteralValue::Null => Ok(Self::Null),
            LiteralValue::Boolean(b) => Ok(Self::Bool(*b.inner())),
            LiteralValue::Int(i) => Ok(Self::Int(*i.inner())),
            LiteralValue::String(s) => Ok(Self::Str(s.inner().to_owned())),
            _ => Err(Construct::Literal.into()),
        }
    }
}
