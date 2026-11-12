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
    Value(&'a IdedExpr, Scope<'a>, Used),
    /// A variable with parameters, applied by name.
    Macro(&'a [String], &'a IdedExpr),
    /// A member of a child list.
    Binder(usize),
}

type Scope<'a> = Option<Rc<Node<'a>>>;

/// Whether a bound value was read: a value the body never reads may fail in
/// the CEL, which the query would not.
type Used = Option<Rc<Cell<bool>>>;

/// A body read with its names bound, and the values to check once it is read.
type Applied<'a> = (&'a IdedExpr, Scope<'a>, Vec<Rc<Cell<bool>>>);

fn mark(used: &Used) {
    if let Some(flag) = used {
        flag.set(true);
    }
}

/// `read`, unless a value it bound was never read.
fn settled<T>(flags: &[Rc<Cell<bool>>], read: R<T>) -> R<T> {
    let read = read?;
    if flags.iter().any(|f| !f.get()) {
        return Err(Reject::new("a bound value that is never read"));
    }
    Ok(read)
}

/// How a call is named in a rejection: by the construct, not the operator.
fn named(name: &str, target: Option<&IdedExpr>) -> String {
    let operator = match name {
        "_?_:_" => "ternary",
        "_+_" => "concatenation or arithmetic",
        "_-_" | "-_" => "subtraction",
        "_*_" => "multiplication",
        "_/_" => "division",
        "_%_" => "modulo",
        "_[_]" | "_[?_]" => "index",
        "@in" => "in",
        _ => "",
    };
    if !operator.is_empty() {
        return operator.to_owned();
    }
    match target.map(|t| &t.expr) {
        Some(Expr::Ident(space)) => format!("the call `{space}.{name}`"),
        _ => format!("the call `{name}`"),
    }
}

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
        steps: Cell::new(0),
    };
    let cqs = translator.formula(condition.expression(), &scope)?.dnf()?;
    Ok(Ucq {
        cqs,
        binders: translator.next.get(),
    })
}

fn binding<'a>(variable: &'a Variable, program: &'a Program, scope: &Scope<'a>) -> Binding<'a> {
    if variable.params.is_empty() {
        Binding::Value(program.expression(), scope.clone(), None)
    } else {
        Binding::Macro(&variable.params, program.expression())
    }
}

struct Translator<'s> {
    subject: &'s str,
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

/// How a comprehension is written.
enum Shape<'a> {
    Bind(Scope<'a>, Rc<Cell<bool>>),
    Exists(usize, Path, F),
    Filter(usize, Path, F),
}

impl<'a> Translator<'_> {
    fn step(&self) -> R<()> {
        self.steps.set(self.steps.get() + 1);
        if self.steps.get() > MAX_STEPS {
            return Err(Reject::new("a condition that expands too far"));
        }
        Ok(())
    }

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
    fn apply(&self, name: &str, args: &'a [IdedExpr], scope: &Scope<'a>) -> Option<R<Applied<'a>>> {
        let Some(Binding::Macro(params, body)) = lookup(scope, name) else {
            return None;
        };
        if params.len() != args.len() {
            return Some(Err(Reject::new(format!(
                "`{name}` called with other arguments"
            ))));
        }
        let mut inner: Scope = None;
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

    fn term(&self, e: &'a IdedExpr, scope: &Scope<'a>) -> R<Operand> {
        self.step()?;
        match &e.expr {
            Expr::Literal(value) => Ok(Operand::Const(constant(value)?)),
            Expr::Ident(name) => match lookup(scope, name) {
                Some(Binding::Value(x, inner, used)) => {
                    mark(&used);
                    self.term(x, &inner)
                }
                Some(Binding::Binder(id)) => Ok(Operand::Path(Self::path(Root::Binder(id)))),
                Some(Binding::Macro(..)) => Err(Reject::new(format!("`{name}` without arguments"))),
                None if name == self.subject => Ok(Operand::Path(Self::path(Root::Subject))),
                None if name == "file" => Ok(Operand::Path(Self::path(Root::File))),
                None if name == "options" => Ok(Operand::Option(Vec::new())),
                None => Err(Reject::new(format!(
                    "undefined name or pack variable `{name}`"
                ))),
            },
            Expr::Select(select) if !select.test => match self.term(&select.operand, scope)? {
                Operand::Path(path) if path.offset != 0 => {
                    Err(Reject::new("arithmetic outside a comparison"))
                }
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
                Shape::Bind(inner, used) => settled(&[used], self.term(&c.result, &inner)),
                _ => Err(Reject::new("a comprehension as a value")),
            },
            Expr::Select(_) => Err(Reject::new("has()")),
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
        if let Some((body, inner, flags)) = Self::bound(target, name, args, scope) {
            return settled(&flags, self.term(body, &inner));
        }
        if target.is_none()
            && let Some(applied) = self.apply(name, args, scope)
        {
            let (body, inner, flags) = applied?;
            return settled(&flags, self.term(body, &inner));
        }
        let sign: i64 = match name {
            ADD => 1,
            SUBSTRACT => -1,
            NEGATE => {
                return match args.first().map(|a| self.term(a, scope)) {
                    Some(Ok(Operand::Const(Const::Int(n)))) => n
                        .checked_neg()
                        .map(|n| Operand::Const(Const::Int(n)))
                        .ok_or_else(overflow),
                    _ => Err(Reject::new("negation of a field")),
                };
            }
            other => return Err(Reject::new(named(other, target))),
        };
        let [a, b] = args else {
            return Err(Reject::new("arithmetic"));
        };
        let shift = |k: i64| k.checked_mul(sign).ok_or_else(overflow);
        match (self.term(a, scope)?, self.term(b, scope)?) {
            (Operand::Const(Const::Int(x)), Operand::Const(Const::Int(y))) => x
                .checked_add(shift(y)?)
                .map(|n| Operand::Const(Const::Int(n)))
                .ok_or_else(overflow),
            (Operand::Path(mut p), Operand::Const(Const::Int(k))) => {
                p.offset = p.offset.checked_add(shift(k)?).ok_or_else(overflow)?;
                Ok(Operand::Path(p))
            }
            (Operand::Const(Const::Int(k)), Operand::Path(mut p)) if sign == 1 => {
                p.offset = p.offset.checked_add(k).ok_or_else(overflow)?;
                Ok(Operand::Path(p))
            }
            (Operand::Const(Const::Str(_)), _) | (_, Operand::Const(Const::Str(_))) => {
                Err(Reject::new("concatenation"))
            }
            _ => Err(Reject::new("arithmetic of fields")),
        }
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
        if let Some((body, inner, flags)) = Self::bound(target, name, args, scope) {
            return settled(&flags, self.formula(body, &inner));
        }
        if target.is_none()
            && let Some(applied) = self.apply(name, args, scope)
        {
            let (body, inner, flags) = applied?;
            return settled(&flags, self.formula(body, &inner));
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
                    return Err(Reject::new("membership in a field"));
                }
                compare(left, Op::In, right)
            }
            ("startsWith", [a, b]) => self.comparison(Op::Starts, a, b, scope),
            ("endsWith", [a, b]) => self.comparison(Op::Ends, a, b, scope),
            ("contains", [a, b]) => self.comparison(Op::Contains, a, b, scope),
            (other, _) => Err(Reject::new(named(other, target))),
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
                return Err(Reject::new("a size compared with a non-literal"));
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
            _ => Err(Reject::new("a count of members other than none or some")),
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
                    return Err(Reject::new("a list literal of computed elements"));
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
            Expr::Call(call)
                if let Some((body, inner, flags)) =
                    Self::bound(call.target.as_deref(), &call.func_name, &call.args, scope) =>
            {
                settled(&flags, self.members(body, &inner))
            }
            Expr::Call(call) if call.target.is_none() => {
                match self.apply(&call.func_name, &call.args, scope) {
                    Some(applied) => {
                        let (body, inner, flags) = applied?;
                        settled(&flags, self.members(body, &inner))
                    }
                    None => Err(Reject::new(named(&call.func_name, None))),
                }
            }
            Expr::Comprehension(c) => match self.shape(c, scope)? {
                Shape::Bind(inner, used) => settled(&[used], self.members(&c.result, &inner)),
                Shape::Filter(binder, list, cond) => Ok(Members::Filter { binder, list, cond }),
                Shape::Exists(..) => Err(Reject::new("a condition as a list")),
            },
            _ => Err(Reject::new("a list that is computed")),
        }
    }

    fn members_path(&self, e: &'a IdedExpr, scope: &Scope<'a>) -> R<Members> {
        match self.term(e, scope)? {
            Operand::Path(path) if path.offset != 0 => {
                Err(Reject::new("arithmetic outside a comparison"))
            }
            Operand::Path(path) => Ok(Members::Path(path)),
            _ => Err(Reject::new("a list that is not a field")),
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
            _ => return Err(Reject::new("a comprehension")),
        };
        let wanted = if literal(&c.accu_init, false) && step.func_name == LOGICAL_OR {
            step.args.get(1)
        } else if lighthouse_spec::is_empty_list(&c.accu_init) && step.func_name == CONDITIONAL {
            step.args.first()
        } else {
            let all = literal(&c.accu_init, true) && step.func_name == LOGICAL_AND;
            return Err(Reject::new(if all { "all" } else { "a comprehension" }));
        };
        let Some(cond) = wanted else {
            return Err(Reject::new("a comprehension"));
        };
        let list = match self.term(&c.iter_range, scope)? {
            Operand::Path(list) if list.offset == 0 => list,
            Operand::Path(_) => return Err(Reject::new("arithmetic outside a comparison")),
            _ => return Err(Reject::new("a comprehension over a computed list")),
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
    if let Operand::Path(path) = &mut left
        && path.offset != 0
    {
        match &mut right {
            Operand::Const(Const::Int(k)) => {
                *k = k.checked_sub(path.offset).ok_or_else(overflow)?;
                path.offset = 0;
            }
            Operand::Option(_) if !matches!(op, Op::In | Op::NotIn) => {}
            _ => return Err(Reject::new("arithmetic outside a comparison with a number")),
        }
    }
    if matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge)
        && matches!(&right, Operand::Const(c) if !matches!(c, Const::Int(_) | Const::Str(_)))
    {
        return Err(Reject::new(
            "an order between values that are not numbers or texts",
        ));
    }
    Ok(F::Cmp(Cmp { left, op, right }))
}

fn overflow() -> Reject {
    Reject::new("an integer that overflows")
}
