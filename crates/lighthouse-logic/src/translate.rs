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
#[cfg(test)]
mod tests;

use std::{cell::Cell, fmt, rc::Rc};

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

type R<T> = Result<T, Reject>;

/// The construct that leaves the fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject(pub String);

impl Reject {
    fn new(construct: impl Into<String>) -> Self {
        Self(construct.into())
    }
}

impl fmt::Display for Reject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "outside the fragment: {}", self.0)
    }
}

impl std::error::Error for Reject {}

/// What a name stands for while an expression is read.
#[derive(Clone)]
enum Binding<'a> {
    /// An expression, read in the scope it was written in.
    Value(&'a IdedExpr, Scope<'a>),
    /// A variable with parameters, applied by name.
    Macro(&'a [String], &'a IdedExpr),
    /// A member of a child list.
    Binder(usize),
}

type Scope<'a> = Option<Rc<Node<'a>>>;

struct Node<'a> {
    name: &'a str,
    binding: Binding<'a>,
    parent: Scope<'a>,
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

/// The query for `check`, whose selected value is the variable `subject`
/// (`symbol`, `func`, ...).
pub fn translate(env: &Env, check: &CelCheck, subject: &str) -> R<Ucq> {
    let programs: Vec<Program> = check
        .variables
        .iter()
        .map(|v| env.compile(&v.expression))
        .collect::<Result<_, _>>()
        .map_err(|e| Reject::new(format!("an expression that does not compile: {e}")))?;
    let condition = env
        .compile(&check.condition)
        .map_err(|e| Reject::new(format!("an expression that does not compile: {e}")))?;
    let mut scope: Scope = None;
    for (variable, program) in check.variables.iter().zip(&programs) {
        scope = bind(&scope, &variable.name, binding(variable, program, &scope));
    }
    let translator = Translator {
        subject,
        next: Cell::new(0),
    };
    let cqs = translator.formula(condition.expression(), &scope)?.dnf()?;
    Ok(Ucq {
        cqs,
        binders: translator.next.get(),
    })
}

fn binding<'a>(variable: &'a Variable, program: &'a Program, scope: &Scope<'a>) -> Binding<'a> {
    if variable.params.is_empty() {
        Binding::Value(program.expression(), scope.clone())
    } else {
        Binding::Macro(&variable.params, program.expression())
    }
}

struct Translator<'s> {
    subject: &'s str,
    next: Cell<usize>,
}

/// A value that is a list.
enum Members {
    Path(Path),
    Literal(usize),
    Choice(Box<F>, Box<Members>, Box<Members>),
    Filter { binder: usize, list: Path, cond: F },
}

/// How a comprehension is written.
enum Shape<'a> {
    Bind(Scope<'a>),
    Exists(usize, Path, F),
    Filter(usize, Path, F),
}

impl<'a> Translator<'_> {
    fn fresh(&self) -> usize {
        let id = self.next.get();
        self.next.set(id + 1);
        id
    }

    fn path(root: Root) -> Path {
        Path {
            root,
            fields: Vec::new(),
            offset: 0,
        }
    }

    /// The body of the variable `name` applied to `args`, with its scope.
    fn apply(
        &self,
        name: &str,
        args: &'a [IdedExpr],
        scope: &Scope<'a>,
    ) -> Option<R<(&'a IdedExpr, Scope<'a>)>> {
        let Some(Binding::Macro(params, body)) = lookup(scope, name) else {
            return None;
        };
        if params.len() != args.len() {
            return Some(Err(Reject::new(format!(
                "`{name}` called with other arguments"
            ))));
        }
        let mut inner: Scope = None;
        for (param, arg) in params.iter().zip(args) {
            inner = bind(&inner, param, Binding::Value(arg, scope.clone()));
        }
        Some(Ok((body, inner)))
    }

    /// `cel.bind(name, value, body)` written as a call: the body, with the
    /// name standing for the value.
    fn bound(
        target: Option<&'a IdedExpr>,
        name: &str,
        args: &'a [IdedExpr],
        scope: &Scope<'a>,
    ) -> Option<(&'a IdedExpr, Scope<'a>)> {
        let named = matches!(target.map(|t| &t.expr), Some(Expr::Ident(n)) if n == "cel");
        let [var, value, body] = args else {
            return None;
        };
        let Expr::Ident(var) = &var.expr else {
            return None;
        };
        (named && name == "bind")
            .then(|| (body, bind(scope, var, Binding::Value(value, scope.clone()))))
    }

    fn term(&self, e: &'a IdedExpr, scope: &Scope<'a>) -> R<Operand> {
        match &e.expr {
            Expr::Literal(value) => Ok(Operand::Const(constant(value)?)),
            Expr::Ident(name) => match lookup(scope, name) {
                Some(Binding::Value(x, inner)) => self.term(x, &inner),
                Some(Binding::Binder(id)) => Ok(Operand::Path(Self::path(Root::Binder(id)))),
                Some(Binding::Macro(..)) => Err(Reject::new(format!("`{name}` without arguments"))),
                None if name == self.subject => Ok(Operand::Path(Self::path(Root::Subject))),
                None if name == "file" => Ok(Operand::Path(Self::path(Root::File))),
                None if name == "options" => Ok(Operand::Option(Vec::new())),
                None => Err(Reject::new(format!("variable `{name}`"))),
            },
            Expr::Select(select) if !select.test => match self.term(&select.operand, scope)? {
                Operand::Path(mut path) => {
                    path.fields.push(select.field.clone());
                    Ok(Operand::Path(path))
                }
                Operand::Option(mut fields) => {
                    fields.push(select.field.clone());
                    Ok(Operand::Option(fields))
                }
                Operand::Const(_) => Err(Reject::new("a field of a constant")),
            },
            Expr::List(list) => {
                let items = list
                    .elements
                    .iter()
                    .map(|item| match self.term(item, scope)? {
                        Operand::Const(value) => Ok(value),
                        _ => Err(Reject::new("a list that is not constant")),
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
                Shape::Bind(inner) => self.term(&c.result, &inner),
                _ => Err(Reject::new("a comprehension as a value")),
            },
            _ => Err(Reject::new("an expression that is not a value")),
        }
    }

    fn term_call(
        &self,
        name: &str,
        target: Option<&'a IdedExpr>,
        args: &'a [IdedExpr],
        scope: &Scope<'a>,
    ) -> R<Operand> {
        if let Some((body, inner)) = Self::bound(target, name, args, scope) {
            return self.term(body, &inner);
        }
        if target.is_none()
            && let Some(applied) = self.apply(name, args, scope)
        {
            let (body, inner) = applied?;
            return self.term(body, &inner);
        }
        let sign = match name {
            ADD => 1,
            SUBSTRACT => -1,
            NEGATE => {
                return match args.first().map(|a| self.term(a, scope)) {
                    Some(Ok(Operand::Const(Const::Int(n)))) => Ok(Operand::Const(Const::Int(-n))),
                    _ => Err(Reject::new("arithmetic")),
                };
            }
            other => return Err(Reject::new(format!("the call `{other}`"))),
        };
        let [a, b] = args else {
            return Err(Reject::new("arithmetic"));
        };
        match (self.term(a, scope)?, self.term(b, scope)?) {
            (Operand::Const(Const::Int(x)), Operand::Const(Const::Int(y))) => {
                Ok(Operand::Const(Const::Int(x + sign * y)))
            }
            (Operand::Path(mut p), Operand::Const(Const::Int(k))) => {
                p.offset += sign * k;
                Ok(Operand::Path(p))
            }
            (Operand::Const(Const::Int(k)), Operand::Path(mut p)) if sign == 1 => {
                p.offset += k;
                Ok(Operand::Path(p))
            }
            _ => Err(Reject::new("arithmetic of fields")),
        }
    }

    fn formula(&self, e: &'a IdedExpr, scope: &Scope<'a>) -> R<F> {
        match &e.expr {
            Expr::Literal(LiteralValue::Boolean(b)) => Ok(F::Lit(*b.inner())),
            Expr::Ident(name) => match lookup(scope, name) {
                Some(Binding::Value(x, inner)) => self.formula(x, &inner),
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
                Shape::Bind(inner) => self.formula(&c.result, &inner),
                Shape::Exists(binder, list, cond) => Ok(F::Has {
                    binder,
                    list,
                    cond: Box::new(cond),
                }),
                Shape::Filter(..) => Err(Reject::new("a list as a condition")),
            },
            _ => Err(Reject::new("an expression that is not a condition")),
        }
    }

    /// A boolean field or option, as a comparison with `true`.
    fn truth(&self, e: &'a IdedExpr, scope: &Scope<'a>) -> R<F> {
        compare(
            self.term(e, scope)?,
            Op::Eq,
            Operand::Const(Const::Bool(true)),
        )
    }

    fn call(
        &self,
        name: &str,
        target: Option<&'a IdedExpr>,
        args: &'a [IdedExpr],
        scope: &Scope<'a>,
    ) -> R<F> {
        if let Some((body, inner)) = Self::bound(target, name, args, scope) {
            return self.formula(body, &inner);
        }
        if target.is_none()
            && let Some(applied) = self.apply(name, args, scope)
        {
            let (body, inner) = applied?;
            return self.formula(body, &inner);
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
            (EQUALS, [a, b]) => self.comparison(Op::Eq, a, b, scope),
            (NOT_EQUALS, [a, b]) => self.comparison(Op::Ne, a, b, scope),
            (LESS, [a, b]) => self.comparison(Op::Lt, a, b, scope),
            (LESS_EQUALS, [a, b]) => self.comparison(Op::Le, a, b, scope),
            (GREATER, [a, b]) => self.comparison(Op::Gt, a, b, scope),
            (GREATER_EQUALS, [a, b]) => self.comparison(Op::Ge, a, b, scope),
            (IN, [a, b]) => {
                let (left, right) = (self.term(a, scope)?, self.term(b, scope)?);
                if matches!(right, Operand::Path(_)) {
                    return Err(Reject::new("membership in a field list"));
                }
                compare(left, Op::In, right)
            }
            ("startsWith", [a, b]) => self.comparison(Op::Starts, a, b, scope),
            ("endsWith", [a, b]) => self.comparison(Op::Ends, a, b, scope),
            ("contains", [a, b]) => self.comparison(Op::Contains, a, b, scope),
            (other, _) => Err(Reject::new(format!("the call `{other}`"))),
        }
    }

    /// `(c and a) or (not c and b)`.
    fn choice(&self, c: &F, a: F, b: F) -> R<F> {
        Ok(F::Or(vec![
            F::And(vec![c.clone(), a]),
            F::And(vec![self.negate(c.clone())?, b]),
        ]))
    }

    fn comparison(&self, op: Op, a: &'a IdedExpr, b: &'a IdedExpr, scope: &Scope<'a>) -> R<F> {
        let (counted, other, op) = match (size_of(a), size_of(b)) {
            (Some(x), _) => (Some(x), b, op),
            (None, Some(x)) => (Some(x), a, op.flipped().ok_or_else(|| Reject::new("size"))?),
            (None, None) => (None, b, op),
        };
        if let Some(counted) = counted {
            let Operand::Const(Const::Int(k)) = self.term(other, scope)? else {
                return Err(Reject::new("size compared with a non-literal"));
            };
            return self.sized(self.members(counted, scope)?, op, k);
        }
        compare(self.term(a, scope)?, op, self.term(b, scope)?)
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
            F::Has { .. } => Err(Reject::new("negation of an exists with a condition")),
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
            Members::Literal(n) => Ok(F::Lit(holds(i64::try_from(n).unwrap_or(i64::MAX)))),
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
            _ => Err(Reject::new("a count of members other than none or some")),
        }
    }

    fn members(&self, e: &'a IdedExpr, scope: &Scope<'a>) -> R<Members> {
        match &e.expr {
            Expr::List(list) => Ok(Members::Literal(list.elements.len())),
            Expr::Ident(name) => match lookup(scope, name) {
                Some(Binding::Value(x, inner)) => self.members(x, &inner),
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
            Expr::Call(call)
                if let Some((body, inner)) =
                    Self::bound(call.target.as_deref(), &call.func_name, &call.args, scope) =>
            {
                self.members(body, &inner)
            }
            Expr::Call(call) if call.target.is_none() => {
                match self.apply(&call.func_name, &call.args, scope) {
                    Some(applied) => {
                        let (body, inner) = applied?;
                        self.members(body, &inner)
                    }
                    None => Err(Reject::new(format!("the call `{}`", call.func_name))),
                }
            }
            Expr::Comprehension(c) => match self.shape(c, scope)? {
                Shape::Bind(inner) => self.members(&c.result, &inner),
                Shape::Filter(binder, list, cond) => Ok(Members::Filter { binder, list, cond }),
                Shape::Exists(..) => Err(Reject::new("a condition as a list")),
            },
            _ => Err(Reject::new("a list that is computed")),
        }
    }

    fn members_path(&self, e: &'a IdedExpr, scope: &Scope<'a>) -> R<Members> {
        match self.term(e, scope)? {
            Operand::Path(path) => Ok(Members::Path(path)),
            _ => Err(Reject::new("a list that is not a field")),
        }
    }

    /// Reads a comprehension as `cel.bind`, `exists` or `filter`.
    fn shape(&self, c: &'a ComprehensionExpr, scope: &Scope<'a>) -> R<Shape<'a>> {
        let empty_list = |e: &IdedExpr| matches!(&e.expr, Expr::List(l) if l.elements.is_empty());
        let literal = |e: &IdedExpr, want: bool| matches!(&e.expr, Expr::Literal(LiteralValue::Boolean(b)) if *b.inner() == want);
        if empty_list(&c.iter_range) && literal(&c.loop_cond, false) {
            return Ok(Shape::Bind(bind(
                scope,
                &c.accu_var,
                Binding::Value(&c.accu_init, scope.clone()),
            )));
        }
        let step = match &c.loop_step.expr {
            Expr::Call(call) => call,
            _ => return Err(Reject::new("a comprehension")),
        };
        let wanted = if literal(&c.accu_init, false) && step.func_name == LOGICAL_OR {
            step.args.get(1)
        } else if empty_list(&c.accu_init) && step.func_name == CONDITIONAL {
            step.args.first()
        } else {
            let all = literal(&c.accu_init, true) && step.func_name == LOGICAL_AND;
            return Err(Reject::new(if all { "all" } else { "a comprehension" }));
        };
        let Some(cond) = wanted else {
            return Err(Reject::new("a comprehension"));
        };
        let Operand::Path(list) = self.term(&c.iter_range, scope)? else {
            return Err(Reject::new("a comprehension over a computed list"));
        };
        let binder = self.fresh();
        let inner = bind(scope, &c.iter_var, Binding::Binder(binder));
        let cond = self.formula(cond, &inner)?;
        Ok(if literal(&c.accu_init, false) {
            Shape::Exists(binder, list, cond)
        } else {
            Shape::Filter(binder, list, cond)
        })
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

fn constant(value: &LiteralValue) -> R<Const> {
    match value {
        LiteralValue::Null => Ok(Const::Null),
        LiteralValue::Boolean(b) => Ok(Const::Bool(*b.inner())),
        LiteralValue::Int(i) => Ok(Const::Int(*i.inner())),
        LiteralValue::String(s) => Ok(Const::Str(s.inner().to_owned())),
        _ => Err(Reject::new(
            "a literal that is not a bool, int, string or null",
        )),
    }
}

/// `left op right` as a condition: a field is compared with a constant, and a
/// comparison of two fields is outside the fragment.
fn compare(mut left: Operand, mut op: Op, mut right: Operand) -> R<F> {
    match (&left, &right) {
        (Operand::Path(_), Operand::Path(_)) => {
            return Err(Reject::new("a comparison of two fields"));
        }
        (_, Operand::Path(_)) => {
            op = op
                .flipped()
                .ok_or_else(|| Reject::new("a field on the right of `in`"))?;
            std::mem::swap(&mut left, &mut right);
        }
        _ => {}
    }
    if let (Operand::Path(path), Operand::Const(Const::Int(k))) = (&mut left, &mut right)
        && path.offset != 0
    {
        *k -= path.offset;
        path.offset = 0;
    }
    if matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge)
        && matches!(&right, Operand::Const(c) if !matches!(c, Const::Int(_)))
    {
        return Err(Reject::new("an order between values that are not numbers"));
    }
    Ok(F::Cmp(Cmp { left, op, right }))
}
