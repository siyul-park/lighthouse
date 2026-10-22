//! What the expressions of a check ask for, found from their syntax trees.

use std::collections::BTreeSet;

use cel::{
    Program,
    common::ast::{
        Expr, IdedExpr, LiteralValue,
        operators::{INDEX, OPT_INDEX, OPT_SELECT},
    },
};
use lighthouse_spec::TemplatePart;

/// The variables of an expression that are options, never nodes: a dynamic
/// index into them reads no field of the model.
const OPTIONS: &str = "options";

/// The variables that hold the node a check is about; a field read from
/// anything else (a related node, a comprehension variable, a `variables`
/// entry) is read from a related node.
const SUBJECTS: [&str; 7] = ["symbol", "func", "edge", "module", "file", "test", "node"];

/// The fields of the model that the expressions read. Found once, when the
/// check is compiled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Needs {
    fields: BTreeSet<String>,
    /// The fields read from something other than the node itself: what a
    /// related node needs of its own.
    related: BTreeSet<String>,
    /// Some expression indexes a value by a key it computes, so any field may
    /// be read.
    dynamic: bool,
    /// Some key computed for a value other than the node itself.
    related_dynamic: bool,
}

impl Needs {
    /// What `expressions` read, and the holes of `templates`. A source that
    /// does not compile asks for nothing: compiling it is an error elsewhere.
    pub(crate) fn of<'a>(
        expressions: impl IntoIterator<Item = &'a str>,
        templates: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let holes = templates
            .into_iter()
            .filter_map(|text| lighthouse_spec::parse_template(text).ok())
            .flatten()
            .filter_map(|part| match part {
                TemplatePart::Hole(source) => Some(source),
                TemplatePart::Text(_) => None,
            });
        let mut needs = Self::default();
        for source in expressions.into_iter().map(str::to_owned).chain(holes) {
            if let Ok(program) = lighthouse_spec::compile(&source) {
                needs.read(&program);
            }
        }
        needs
    }

    /// Whether some expression reads the field `name` of a value (`n.callers`,
    /// `n.?callers`, `n['callers']`): the way a field of the model is asked
    /// for, so that it is built only for the expressions that use it.
    pub(crate) fn field(&self, name: &str) -> bool {
        self.dynamic || self.fields.contains(name)
    }

    /// What a node related to another one needs: the fields read from
    /// anything but the subject of the check, and none of the others, so a
    /// related node is not built with what only the subject is asked for.
    pub(crate) fn of_related(&self) -> Self {
        Self {
            fields: self.related.clone(),
            related: self.related.clone(),
            dynamic: self.related_dynamic,
            related_dynamic: self.related_dynamic,
        }
    }

    fn read(&mut self, program: &Program) {
        self.walk(program.expression());
    }

    fn walk(&mut self, expr: &IdedExpr) {
        match &expr.expr {
            Expr::Select(select) => {
                self.insert(&select.field, &select.operand);
                self.walk(&select.operand);
            }
            Expr::Call(call) => {
                let operands = call.target.iter().map(|t| &**t).chain(&call.args);
                match call.func_name.as_str() {
                    OPT_SELECT | INDEX | OPT_INDEX => self.keyed(operands),
                    _ => operands.for_each(|operand| self.walk(operand)),
                }
            }
            _ => lighthouse_spec::children(expr)
                .into_iter()
                .for_each(|child| self.walk(child)),
        }
    }

    /// The field `name` of `operand` is read.
    fn insert(&mut self, name: &str, operand: &IdedExpr) {
        self.fields.insert(name.to_owned());
        if !is_subject(operand) {
            self.related.insert(name.to_owned());
        }
    }

    /// A selection or an index: `operand.?key`, `operand['key']` or
    /// `operand[expr]`. A string key is a field; a key that is computed may be
    /// any field, unless the operand is the options.
    fn keyed<'a>(&mut self, operands: impl Iterator<Item = &'a IdedExpr>) {
        let operands: Vec<&IdedExpr> = operands.collect();
        let [operand, key] = operands.as_slice() else {
            operands.into_iter().for_each(|o| self.walk(o));
            return;
        };
        match &key.expr {
            Expr::Literal(LiteralValue::String(name)) => self.insert(name, operand),
            Expr::Literal(_) => {}
            _ if is_position(key) => self.walk(key),
            _ => {
                let any = !rooted_in_options(operand);
                self.dynamic |= any;
                self.related_dynamic |= any && !is_subject(operand);
                self.walk(key);
            }
        }
        self.walk(operand);
    }
}

/// Whether the key is a position in a list, by its syntax: `size(l) - 1`
/// names no field.
fn is_position(key: &IdedExpr) -> bool {
    match &key.expr {
        Expr::Literal(LiteralValue::Int(_) | LiteralValue::UInt(_)) => true,
        Expr::Call(call) => match call.func_name.as_str() {
            "size" => true,
            "_+_" | "_-_" | "_*_" | "_/_" | "_%_" => call.args.iter().all(is_position),
            _ => false,
        },
        _ => false,
    }
}

/// Whether the expression is the variable of the node the check is about.
fn is_subject(expr: &IdedExpr) -> bool {
    matches!(&expr.expr, Expr::Ident(name) if SUBJECTS.contains(&name.as_str()))
}

/// Whether the expression is the options or a field of them.
fn rooted_in_options(expr: &IdedExpr) -> bool {
    match &expr.expr {
        Expr::Ident(name) => name == OPTIONS,
        Expr::Select(select) => rooted_in_options(&select.operand),
        Expr::Call(call) if matches!(call.func_name.as_str(), OPT_SELECT | INDEX | OPT_INDEX) => {
            call.args.first().is_some_and(rooted_in_options)
                || call.target.as_deref().is_some_and(rooted_in_options)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
