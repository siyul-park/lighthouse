//! What the expressions of a check ask for, found from their syntax trees.

use std::collections::BTreeSet;

use cel::{
    Program,
    common::ast::{
        Expr, IdedExpr, LiteralValue,
        operators::{INDEX, OPT_INDEX, OPT_SELECT},
    },
};
use lighthouse_spec::{Cel, TemplatePart};

/// The variables of an expression that are options, never nodes: a dynamic
/// index into them reads no field of the model.
const OPTIONS: &str = "options";

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
    /// The variables the expressions name.
    idents: BTreeSet<String>,
    /// Whether `file.annotations` is explicitly read (only available when
    /// judging annotation subjects after suppression).
    file_annotations: bool,
    /// The variables of the comprehensions being walked that run over
    /// `lists.range`: they hold positions in a list, never a field name.
    positions: Vec<String>,
    /// The variables bound, being walked, to a list of positions (what
    /// `sortBy` binds its input to): a comprehension over one runs over
    /// positions too.
    ranges: Vec<String>,
}

impl Needs {
    /// What `expressions` read, and the holes of `templates`. A source that
    /// does not compile asks for nothing: compiling it is an error elsewhere.
    pub(crate) fn of<'a>(
        cel: &Cel,
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
        let programs: Vec<Program> = expressions
            .into_iter()
            .map(str::to_owned)
            .chain(holes)
            .filter_map(|source| cel.compile(&source).ok())
            .collect();
        Self::reading(&programs)
    }

    /// What the compiled `programs` read.
    pub(crate) fn reading<'a>(programs: impl IntoIterator<Item = &'a Program>) -> Self {
        let mut needs = Self::default();
        for program in programs {
            needs.read(program);
        }
        needs
    }

    /// Whether some expression reads the field `name` of a value (`n.callers`,
    /// `n.?callers`, `n['callers']`): the way a field of the model is asked
    /// for, so that it is built only for the expressions that use it.
    pub(crate) fn field(&self, name: &str) -> bool {
        self.dynamic || self.fields.contains(name)
    }

    /// Whether some expression names the variable `name`.
    pub(crate) fn ident(&self, name: &str) -> bool {
        self.idents.contains(name)
    }

    /// Whether the source explicitly reads `file.annotations`.
    pub(crate) fn reads_file_annotations(&self) -> bool {
        self.file_annotations
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
            idents: BTreeSet::new(),
            file_annotations: self.file_annotations,
            positions: Vec::new(),
            ranges: Vec::new(),
        }
    }

    fn read(&mut self, program: &Program) {
        self.walk(program.expression());
    }

    fn walk(&mut self, expr: &IdedExpr) {
        match &expr.expr {
            Expr::Ident(name) => {
                self.idents.insert(name.clone());
            }
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
            Expr::Comprehension(comprehension) => {
                let positional = self.is_range(&comprehension.iter_range);
                let binds_range = is_empty_list(&comprehension.iter_range)
                    && self.is_range(&comprehension.accu_init);
                if positional {
                    self.positions.push(comprehension.iter_var.clone());
                }
                if binds_range {
                    self.ranges.push(comprehension.accu_var.clone());
                }
                lighthouse_spec::children(expr)
                    .into_iter()
                    .for_each(|child| self.walk(child));
                if positional {
                    self.positions.pop();
                }
                if binds_range {
                    self.ranges.pop();
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
        if name == "annotations" && matches!(&operand.expr, Expr::Ident(name) if name == "file") {
            self.file_annotations = true;
        }
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
            _ if self.is_position(key) => self.walk(key),
            _ => {
                let any = !rooted_in_options(operand);
                self.dynamic |= any;
                self.related_dynamic |= any && !is_subject(operand);
                self.walk(key);
            }
        }
        self.walk(operand);
    }

    /// Whether the key is a position in a list, by its syntax: `size(l) - 1`
    /// names no field, nor does a variable that runs over `lists.range`.
    fn is_position(&self, key: &IdedExpr) -> bool {
        match &key.expr {
            Expr::Literal(LiteralValue::Int(_) | LiteralValue::UInt(_)) => true,
            Expr::Ident(name) => self.positions.contains(name),
            Expr::Call(call) => match call.func_name.as_str() {
                "size" => true,
                "_+_" | "_-_" | "_*_" | "_/_" | "_%_" => {
                    call.args.iter().all(|a| self.is_position(a))
                }
                _ => false,
            },
            _ => false,
        }
    }
}

impl Needs {
    /// Whether the expression is `lists.range(n)`, a `filter` of numbers like
    /// it, a variable bound to one, or the `sortBy` of one: a list of
    /// positions.
    fn is_range(&self, expr: &IdedExpr) -> bool {
        match &expr.expr {
            Expr::Ident(name) => self.ranges.contains(name),
            Expr::Call(call) => {
                call.func_name == "lists.range"
                    || call.func_name == "misplaced"
                    || (call.func_name == "range"
                        && matches!(call.target.as_deref().map(|t| &t.expr), Some(Expr::Ident(n)) if n == "lists"))
            }
            Expr::Comprehension(c) if is_empty_list(&c.iter_range) => {
                // `list.sortBy(e, key)` is `cel.bind(input, list, input.@sortByAssociatedKeys(..))`.
                self.is_range(&c.accu_init)
                    && matches!(&c.result.expr, Expr::Call(call)
                        if call.func_name.ends_with("sortByAssociatedKeys"))
            }
            Expr::Comprehension(c) => {
                self.is_range(&c.iter_range) && keeps_variable(&c.loop_step, &c.iter_var)
            }
            _ => false,
        }
    }
}

/// Whether the expression is the list `[]`.
fn is_empty_list(expr: &IdedExpr) -> bool {
    matches!(&expr.expr, Expr::List(list) if list.elements.is_empty())
}

/// Whether the step of a comprehension appends the iteration variable itself
/// (a `filter`), as opposed to something computed from it (a `map`).
fn keeps_variable(step: &IdedExpr, variable: &str) -> bool {
    match &step.expr {
        Expr::List(list) => {
            matches!(list.elements.as_slice(), [e] if matches!(&e.expr, Expr::Ident(n) if n == variable))
        }
        _ => lighthouse_spec::children(step)
            .into_iter()
            .any(|child| keeps_variable(child, variable)),
    }
}

/// Whether the expression is the variable of the node the check is about.
fn is_subject(expr: &IdedExpr) -> bool {
    matches!(&expr.expr, Expr::Ident(name) if lighthouse_spec::SUBJECT_VARIABLES.contains(&name.as_str()))
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
