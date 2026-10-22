//! What the expressions of a check ask for, found from their syntax trees.

use std::collections::BTreeSet;

use cel::{
    Program,
    common::ast::{
        EntryExpr, Expr, IdedExpr, LiteralValue,
        operators::{INDEX, OPT_INDEX, OPT_SELECT},
    },
};
use lighthouse_spec::TemplatePart;

/// The variables of an expression that are options, never nodes: a dynamic
/// index into them reads no field of the model.
const OPTIONS: &str = "options";

/// The fields of the model that the expressions read. Found once, when the
/// check is compiled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Needs {
    fields: BTreeSet<String>,
    /// Some expression indexes a value by a key it computes, so any field may
    /// be read.
    dynamic: bool,
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

    fn read(&mut self, program: &Program) {
        self.walk(program.expression());
    }

    fn walk(&mut self, expr: &IdedExpr) {
        match &expr.expr {
            Expr::Select(select) => {
                self.fields.insert(select.field.clone());
                self.walk(&select.operand);
            }
            Expr::Call(call) => {
                let operands = call.target.iter().map(|t| &**t).chain(&call.args);
                match call.func_name.as_str() {
                    OPT_SELECT | INDEX | OPT_INDEX => self.keyed(operands),
                    _ => operands.for_each(|operand| self.walk(operand)),
                }
            }
            Expr::Comprehension(comprehension) => {
                for part in [
                    &comprehension.iter_range,
                    &comprehension.accu_init,
                    &comprehension.loop_cond,
                    &comprehension.loop_step,
                    &comprehension.result,
                ] {
                    self.walk(part);
                }
            }
            Expr::List(list) => list.elements.iter().for_each(|e| self.walk(e)),
            Expr::Map(map) => self.entries(&map.entries),
            Expr::Struct(structure) => self.entries(&structure.entries),
            Expr::Ident(_) | Expr::Literal(_) | Expr::Unspecified => {}
        }
    }

    fn entries(&mut self, entries: &[cel::common::ast::IdedEntryExpr]) {
        for entry in entries {
            match &entry.expr {
                EntryExpr::StructField(field) => self.walk(&field.value),
                EntryExpr::MapEntry(entry) => {
                    self.walk(&entry.key);
                    self.walk(&entry.value);
                }
            }
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
            Expr::Literal(LiteralValue::String(name)) => {
                self.fields.insert(name.to_string());
            }
            Expr::Literal(_) => {}
            _ => {
                self.dynamic |= !rooted_in_options(operand);
                self.walk(key);
            }
        }
        self.walk(operand);
    }
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
