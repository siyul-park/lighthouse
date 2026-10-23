//! The CEL expressions and `{{ cel }}` templates a decision's check and fix
//! carry: split once, and described when they do not hold.

use std::collections::BTreeSet;

use cel::common::ast::{EntryExpr, Expr, IdedEntryExpr, IdedExpr};

use crate::environment::{compile_shared, is_namespace};

/// The variables a check can name for the value it judges, by `select`, and
/// `node`, the one of a value related to it.
pub const SUBJECT_VARIABLES: [&str; 10] = [
    "symbol", "func", "edge", "module", "file", "test", "comment", "event", "decision", "node",
];

/// One part of a `{{ cel }}` template: literal text, or the source of a hole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplatePart {
    Text(String),
    Hole(String),
}

/// A function a program calls.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Call {
    pub name: String,
    /// The namespace of the function (`regex` of `regex.extract(..)`), if it has one.
    pub namespace: Option<String>,
    /// Whether it is called on a value (`list.size()`), not as `size(list)`.
    pub member: bool,
    /// The arguments it is given, not counting the value of a member call.
    pub args: usize,
}

/// The expressions directly inside `expr`: the operands of a call or a
/// selection, the parts of a comprehension, the elements and the entries of a
/// literal.
pub fn children(expr: &IdedExpr) -> Vec<&IdedExpr> {
    match &expr.expr {
        Expr::Select(select) => vec![&select.operand],
        Expr::Call(call) => call.target.iter().map(|t| &**t).chain(&call.args).collect(),
        Expr::Comprehension(c) => vec![
            &c.iter_range,
            &c.accu_init,
            &c.loop_cond,
            &c.loop_step,
            &c.result,
        ],
        Expr::List(list) => list.elements.iter().collect(),
        Expr::Map(map) => entries(&map.entries),
        Expr::Struct(structure) => entries(&structure.entries),
        Expr::Ident(_) | Expr::Literal(_) | Expr::Unspecified => Vec::new(),
    }
}

/// The identifiers a program names: its variables, the variables its
/// comprehensions bind, and the functions it calls.
pub fn program_identifiers(program: &cel::Program) -> BTreeSet<String> {
    fn collect(expr: &IdedExpr, into: &mut BTreeSet<String>) {
        match &expr.expr {
            Expr::Ident(name) => {
                into.insert(name.clone());
            }
            Expr::Call(call) => {
                into.insert(call.func_name.clone());
            }
            _ => {}
        }
        children(expr).into_iter().for_each(|c| collect(c, into));
    }
    let mut names = BTreeSet::new();
    collect(program.expression(), &mut names);
    names
}

/// The functions a program calls, each once.
pub fn program_calls(program: &cel::Program) -> BTreeSet<Call> {
    fn collect(expr: &IdedExpr, into: &mut BTreeSet<Call>) {
        if let Expr::Call(call) = &expr.expr {
            let namespace = call
                .target
                .as_deref()
                .and_then(|target| match &target.expr {
                    Expr::Ident(name) if is_namespace(name) => Some(name.clone()),
                    _ => None,
                });
            into.insert(Call {
                name: call.func_name.clone(),
                member: call.target.is_some() && namespace.is_none(),
                namespace,
                args: call.args.len(),
            });
        }
        children(expr).into_iter().for_each(|c| collect(c, into));
    }
    let mut calls = BTreeSet::new();
    collect(program.expression(), &mut calls);
    calls
}

/// The variables a program reads from its scope: the identifiers it names that
/// no comprehension binds and that are no namespace of a function.
pub fn free_variables(program: &cel::Program) -> BTreeSet<String> {
    fn collect(expr: &IdedExpr, bound: &mut Vec<String>, into: &mut BTreeSet<String>) {
        match &expr.expr {
            Expr::Ident(name) => {
                if !bound.contains(name) && !is_namespace(name) {
                    into.insert(name.clone());
                }
            }
            Expr::Comprehension(c) => {
                // The range and the initial value are read outside the variables
                // the comprehension binds.
                collect(&c.iter_range, bound, into);
                collect(&c.accu_init, bound, into);
                let before = bound.len();
                bound.push(c.iter_var.clone());
                bound.extend(c.iter_var2.clone());
                bound.push(c.accu_var.clone());
                collect(&c.loop_cond, bound, into);
                collect(&c.loop_step, bound, into);
                collect(&c.result, bound, into);
                bound.truncate(before);
            }
            _ => children(expr)
                .into_iter()
                .for_each(|c| collect(c, bound, into)),
        }
    }
    let mut names = BTreeSet::new();
    collect(program.expression(), &mut Vec::new(), &mut names);
    names
}

/// The identifiers an expression names; a source that does not compile names none.
pub fn identifiers(source: &str) -> BTreeSet<String> {
    compile_shared(source).map_or_else(|_| BTreeSet::new(), |p| program_identifiers(&p))
}

/// Splits a template into its text and its `{{ cel }}` holes; the one parser
/// every template of a decision (check messages, fix names and texts) goes
/// through. It does not compile the holes.
pub fn parse_template(text: &str) -> Result<Vec<TemplatePart>, String> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let (before, after) = rest.split_at(open);
        if !before.is_empty() {
            parts.push(TemplatePart::Text(before.to_owned()));
        }
        let Some(close) = after.find("}}") else {
            return Err("`{{` is never closed".to_owned());
        };
        parts.push(TemplatePart::Hole(after[2..close].trim().to_owned()));
        rest = &after[close + 2..];
    }
    if !rest.is_empty() {
        parts.push(TemplatePart::Text(rest.to_owned()));
    }
    Ok(parts)
}

/// Why `source`, written as `what`, is not a CEL expression.
pub(crate) fn compile_problem(what: &str, source: &str) -> Option<String> {
    compile_shared(source).err().map(|e| format!("{what}: {e}"))
}

/// Why the `{{ cel }}` template `text`, written as `what`, is not well formed.
pub(crate) fn template_problem(what: &str, text: &str) -> Option<String> {
    match parse_template(text) {
        Err(problem) => Some(format!("{what}: {problem}")),
        Ok(parts) => parts.iter().find_map(|part| match part {
            TemplatePart::Hole(source) => compile_problem(&format!("{what}: `{source}`"), source),
            TemplatePart::Text(_) => None,
        }),
    }
}

fn entries(entries: &[IdedEntryExpr]) -> Vec<&IdedExpr> {
    entries
        .iter()
        .flat_map(|entry| match &entry.expr {
            EntryExpr::StructField(field) => vec![&field.value],
            EntryExpr::MapEntry(entry) => vec![&entry.key, &entry.value],
        })
        .collect()
}
