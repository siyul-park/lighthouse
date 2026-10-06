use std::collections::{BTreeSet, HashSet};

use lighthouse_model::{Edge, EdgeKind, Flow, FlowKind, FunctionSummary, Node, Resolution, Target};
use lighthouse_syntax::{Node as Ast, children_named, descendants, text};

use crate::{
    declarations::{Function, Receiver, Site, visibility},
    imports::Aliases,
    locals::{Locals, Names, type_path},
};

/// Predeclared identifiers that never name a package-level symbol. `min`,
/// `max` and `clear` are left out because packages commonly define them.
const PREDECLARED: &[&str] = &[
    "append",
    "cap",
    "close",
    "complex",
    "copy",
    "delete",
    "imag",
    "len",
    "make",
    "new",
    "panic",
    "print",
    "println",
    "real",
    "recover",
    "true",
    "false",
    "nil",
    "iota",
    "any",
    "bool",
    "byte",
    "comparable",
    "complex64",
    "complex128",
    "error",
    "float32",
    "float64",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "rune",
    "string",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
];

pub(crate) struct Analysis {
    pub summary: FunctionSummary,
    pub edges: Vec<Edge>,
    /// Distinct call and reference targets, in source order of first use.
    pub targets: Vec<Target>,
}

pub(crate) fn analyze(site: &Site, aliases: &Aliases, function: &Function) -> Analysis {
    let names = Names {
        source: site.source,
        module: site.module,
        aliases,
    };
    let locals = Locals::collect(function.node, &names);
    let mut walker = Walker {
        site,
        aliases,
        function,
        locals,
        nesting: 0,
        deepest: 0,
        statements: 0,
        flow: Vec::new(),
        uses: Vec::new(),
        seen: HashSet::new(),
    };
    let body = function.node.child_by_field_name("body");
    if let Some(body) = body {
        walker.visit(body);
    }
    let (params, returns) = signature(function.node);
    let tokens = body.map_or(0, count_tokens);
    let forwards_to = body.and_then(|b| walker.forward(b));
    let summary = FunctionSummary {
        symbol: function.id.clone(),
        max_nesting: walker.deepest,
        statements: walker.statements,
        top_level: body.map_or(0, |b| {
            u32::try_from(statements_of(b).len()).unwrap_or(u32::MAX)
        }),
        params,
        returns,
        tokens,
        flow: walker.flow,
        clone_fingerprint: None,
        forwards_to,
    };
    let from = Node::Symbol(function.id.clone());
    let edges: Vec<Edge> = walker
        .uses
        .iter()
        .map(|(kind, path)| Edge {
            kind: *kind,
            from: from.clone(),
            to: Target::Path(path.clone()),
            resolution: Resolution::Syntactic,
        })
        .collect();
    let mut seen = BTreeSet::new();
    let targets = walker
        .uses
        .iter()
        .filter(|(kind, _)| matches!(kind, EdgeKind::Calls | EdgeKind::References))
        .filter(|(_, path)| seen.insert(path.clone()))
        .map(|(_, path)| Target::Path(path.clone()))
        .collect();
    Analysis {
        summary,
        edges,
        targets,
    }
}

struct Walker<'a> {
    site: &'a Site<'a>,
    aliases: &'a Aliases,
    function: &'a Function<'a>,
    locals: Locals,
    nesting: u32,
    deepest: u32,
    statements: u32,
    flow: Vec<Flow>,
    uses: Vec<(EdgeKind, String)>,
    seen: HashSet<(EdgeKind, String)>,
}

impl<'a> Walker<'a> {
    fn source(&self) -> &'a str {
        self.site.source
    }

    fn names(&self) -> Names<'a> {
        Names {
            source: self.site.source,
            module: self.site.module,
            aliases: self.aliases,
        }
    }

    fn receiver(&self) -> Option<&'a Receiver> {
        self.function.receiver.as_ref()
    }

    fn own_type(&self) -> Option<String> {
        self.receiver()
            .map(|r| format!("{}::{}", self.site.module, r.type_name))
    }

    fn event(&mut self, kind: FlowKind) {
        self.flow.push(Flow::new(kind, self.nesting));
    }

    fn nested(&mut self, f: impl FnOnce(&mut Self)) {
        self.nesting += 1;
        self.deepest = self.deepest.max(self.nesting);
        f(self);
        self.nesting -= 1;
    }

    fn uses(&mut self, kind: EdgeKind, path: String) {
        if self.seen.insert((kind, path.clone())) {
            self.uses.push((kind, path));
        }
    }

    fn children(&mut self, node: Ast) {
        let mut cursor = node.walk();
        let children: Vec<Ast> = node.named_children(&mut cursor).collect();
        for child in children {
            self.visit(child);
        }
    }

    fn visit(&mut self, node: Ast) {
        self.count(node);
        match node.kind() {
            "if_statement" => self.if_statement(node, false),
            "for_statement" => self.loop_statement(node),
            "expression_switch_statement" | "type_switch_statement" => {
                self.switch(node, &["expression_case", "type_case"]);
            }
            "select_statement" => self.switch(node, &["communication_case"]),
            "func_literal" => self.func_literal(node),
            "binary_expression" if logical(node, self.source()).is_some() => self.logic(node),
            "break_statement" | "continue_statement" => {
                if !children_named(node, "label_name").is_empty() {
                    self.event(FlowKind::Jump);
                }
            }
            "goto_statement" => self.event(FlowKind::Jump),
            "call_expression" => self.call(node),
            "selector_expression" => self.selector(node, EdgeKind::References),
            "identifier" => self.identifier(node, EdgeKind::References),
            "type_identifier" => self.type_name(node),
            "qualified_type" => self.qualified_type(node),
            "composite_literal" => self.literal(node),
            _ => self.children(node),
        }
    }

    fn count(&mut self, node: Ast) {
        let kind = node.kind();
        let in_list = node.parent().is_some_and(|p| p.kind() == "statement_list");
        let clause = matches!(
            kind,
            "expression_case" | "default_case" | "type_case" | "communication_case"
        );
        if clause || (in_list && is_statement(kind)) {
            self.statements += 1;
        }
    }

    fn if_statement(&mut self, node: Ast, chained: bool) {
        self.event(if chained {
            FlowKind::ElseIf
        } else {
            FlowKind::If
        });
        for field in ["initializer", "condition"] {
            if let Some(child) = node.child_by_field_name(field) {
                self.visit(child);
            }
        }
        if let Some(body) = node.child_by_field_name("consequence") {
            self.nested(|w| w.visit(body));
        }
        match node.child_by_field_name("alternative") {
            Some(next) if next.kind() == "if_statement" => self.if_statement(next, true),
            Some(block) => {
                self.event(FlowKind::Else);
                self.nested(|w| w.visit(block));
            }
            None => {}
        }
    }

    fn loop_statement(&mut self, node: Ast) {
        self.event(FlowKind::Loop);
        let body = node.child_by_field_name("body");
        let mut cursor = node.walk();
        let header: Vec<Ast> = node
            .named_children(&mut cursor)
            .filter(|c| Some(*c) != body && c.kind() != "comment")
            .collect();
        for child in header {
            self.visit(child);
        }
        if let Some(body) = body {
            self.nested(|w| w.visit(body));
        }
    }

    fn switch(&mut self, node: Ast, arms: &[&str]) {
        let mut cursor = node.walk();
        let children: Vec<Ast> = node.named_children(&mut cursor).collect();
        let (clauses, header): (Vec<Ast>, Vec<Ast>) = children
            .into_iter()
            .filter(|c| c.kind() != "comment")
            .partition(|c| c.kind().ends_with("_case"));
        let counted = clauses.iter().filter(|c| arms.contains(&c.kind())).count();
        self.flow.push(Flow {
            arms: u32::try_from(counted).unwrap_or(u32::MAX),
            returning: arms != ["communication_case"]
                && !clauses.is_empty()
                && clauses.iter().all(|c| returns_only(*c)),
            ..Flow::new(FlowKind::Switch, self.nesting)
        });
        for child in header {
            self.visit(child);
        }
        self.nested(|w| {
            for clause in clauses {
                w.visit(clause);
            }
        });
    }

    fn func_literal(&mut self, node: Ast) {
        if let Some(body) = node.child_by_field_name("body") {
            self.nested(|w| w.visit(body));
        }
    }

    fn logic(&mut self, node: Ast) {
        let mut ops = Vec::new();
        let mut leaves = Vec::new();
        flatten(node, self.source(), &mut ops, &mut leaves);
        let mut runs: Vec<(&str, u32)> = Vec::new();
        for op in ops {
            match runs.last_mut() {
                Some((last, count)) if *last == op => *count += 1,
                _ => runs.push((op, 1)),
            }
        }
        for (_, operators) in runs {
            self.flow.push(Flow {
                operators,
                ..Flow::new(FlowKind::Logic, self.nesting)
            });
        }
        for leaf in leaves {
            self.visit(leaf);
        }
    }

    fn call(&mut self, node: Ast) {
        if let Some(function) = node.child_by_field_name("function") {
            if self.is_recursive(function) {
                self.event(FlowKind::Recursion);
            }
            match function.kind() {
                "identifier" => self.identifier(function, EdgeKind::Calls),
                "selector_expression" => self.selector(function, EdgeKind::Calls),
                _ => self.visit(function),
            }
        }
        for field in ["type_arguments", "arguments"] {
            if let Some(child) = node.child_by_field_name(field) {
                self.visit(child);
            }
        }
    }

    fn is_recursive(&self, function: Ast) -> bool {
        let source = self.source();
        match (function.kind(), self.receiver()) {
            ("identifier", None) => {
                text(function, source) == self.function.name
                    && self.locals.get(&self.function.name).is_none()
            }
            ("selector_expression", Some(receiver)) => {
                let operand = function.child_by_field_name("operand");
                let field = function.child_by_field_name("field");
                matches!((operand, field, &receiver.name), (Some(o), Some(f), Some(name))
                    if text(o, source) == name && text(f, source) == self.function.name)
            }
            _ => false,
        }
    }

    fn identifier(&mut self, node: Ast, kind: EdgeKind) {
        let name = text(node, self.source());
        if name == "_" || self.locals.get(name).is_some() || PREDECLARED.contains(&name) {
            return;
        }
        self.uses(kind, format!("{}::{name}", self.site.module));
    }

    fn type_name(&mut self, node: Ast) {
        let name = text(node, self.source());
        if self.locals.get(name).is_some() || PREDECLARED.contains(&name) {
            return;
        }
        self.uses(
            EdgeKind::References,
            format!("{}::{name}", self.site.module),
        );
    }

    fn qualified_type(&mut self, node: Ast) {
        if let Some(path) = type_path(node, &self.names())
            && node
                .child_by_field_name("package")
                .is_some_and(|p| self.locals.get(text(p, self.source())).is_none())
        {
            self.uses(EdgeKind::References, path);
        }
    }

    fn selector(&mut self, node: Ast, kind: EdgeKind) {
        let source = self.source();
        let (Some(operand), Some(field)) = (
            node.child_by_field_name("operand"),
            node.child_by_field_name("field"),
        ) else {
            self.children(node);
            return;
        };
        if operand.kind() != "identifier" {
            self.visit(operand);
            return;
        }
        let owner = text(operand, source);
        let member = text(field, source);
        match self.locals.get(owner) {
            Some(Some(ty)) => {
                let ty = ty.clone();
                self.member(&ty, member, kind);
            }
            Some(None) => {}
            None => match self.aliases.get(owner) {
                Some(module) => self.uses(kind, format!("{module}::{member}")),
                None => self.identifier(operand, EdgeKind::References),
            },
        }
    }

    fn member(&mut self, owner: &str, member: &str, kind: EdgeKind) {
        self.uses(kind, format!("{owner}::{member}"));
        self.private_access(owner, member);
    }

    fn private_access(&mut self, owner: &str, member: &str) {
        let foreign = self.own_type().as_deref() != Some(owner);
        if foreign && visibility(member) == lighthouse_model::Visibility::Private {
            self.uses(EdgeKind::AccessesPrivate, format!("{owner}::{member}"));
        }
    }

    fn literal(&mut self, node: Ast) {
        let ty = node.child_by_field_name("type");
        let owner = ty.and_then(|t| type_path(t, &self.names()));
        let keyed_fields =
            ty.is_none_or(|t| !matches!(t.kind(), "map_type" | "slice_type" | "array_type"));
        if let Some(t) = ty {
            self.visit(t);
        }
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let source = self.source();
        let mut cursor = body.walk();
        let elements: Vec<Ast> = body.named_children(&mut cursor).collect();
        for element in elements {
            if element.kind() != "keyed_element" {
                self.visit(element);
                continue;
            }
            let key = element.named_child(0);
            let value = element.named_child(1);
            let field = key
                .filter(|k| keyed_fields && k.named_child_count() == 1)
                .and_then(|k| k.named_child(0))
                .filter(|k| k.kind() == "identifier");
            match (field, &owner) {
                (Some(field), Some(owner)) => {
                    let name = text(field, source).to_owned();
                    self.private_access(owner, &name);
                }
                (Some(_), None) => {}
                (None, _) => {
                    if let Some(key) = key {
                        self.visit(key);
                    }
                }
            }
            if let Some(value) = value {
                self.visit(value);
            }
        }
    }

    fn forward(&self, body: Ast) -> Option<Target> {
        let call = sole_call(body)?;
        let function = call.child_by_field_name("function")?;
        let source = self.source();
        let params = parameter_names(self.function.node, source)?;
        let args = argument_names(call, source)?;
        if params != args {
            return None;
        }
        let path = match (function.kind(), self.receiver()) {
            ("selector_expression", Some(receiver)) => {
                let operand = text(function.child_by_field_name("operand")?, source);
                if Some(operand) != receiver.name.as_deref() {
                    return None;
                }
                let member = text(function.child_by_field_name("field")?, source);
                format!("{}::{}::{member}", self.site.module, receiver.type_name)
            }
            ("identifier", None) => {
                let name = text(function, source);
                if self.locals.get(name).is_some() || PREDECLARED.contains(&name) {
                    return None;
                }
                format!("{}::{name}", self.site.module)
            }
            ("selector_expression", None) => {
                let operand = text(function.child_by_field_name("operand")?, source);
                let module = self.aliases.get(operand)?;
                let member = text(function.child_by_field_name("field")?, source);
                format!("{module}::{member}")
            }
            _ => return None,
        };
        Some(Target::Path(path))
    }
}

fn is_statement(kind: &str) -> bool {
    matches!(
        kind,
        "expression_statement"
            | "send_statement"
            | "inc_statement"
            | "dec_statement"
            | "assignment_statement"
            | "short_var_declaration"
            | "labeled_statement"
            | "fallthrough_statement"
            | "break_statement"
            | "continue_statement"
            | "goto_statement"
            | "return_statement"
            | "go_statement"
            | "defer_statement"
            | "if_statement"
            | "for_statement"
            | "expression_switch_statement"
            | "type_switch_statement"
            | "select_statement"
            | "block"
            | "var_declaration"
            | "const_declaration"
            | "type_declaration"
    )
}

fn logical<'s>(node: Ast, source: &'s str) -> Option<&'s str> {
    let op = text(node.child_by_field_name("operator")?, source);
    matches!(op, "&&" | "||").then_some(op)
}

fn unparen(mut node: Ast) -> Ast {
    while node.kind() == "parenthesized_expression" {
        match node.named_child(0) {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node
}

/// Operators in source order and the operands that are not logical chains.
fn flatten<'t, 's>(
    node: Ast<'t>,
    source: &'s str,
    ops: &mut Vec<&'s str>,
    leaves: &mut Vec<Ast<'t>>,
) {
    let node = unparen(node);
    let Some(op) = logical(node, source) else {
        leaves.push(node);
        return;
    };
    if let Some(left) = node.child_by_field_name("left") {
        flatten(left, source, ops, leaves);
    }
    ops.push(op);
    if let Some(right) = node.child_by_field_name("right") {
        flatten(right, source, ops, leaves);
    }
}

fn statements_of(body: Ast) -> Vec<Ast> {
    let mut cursor = body.walk();
    let list = body
        .named_children(&mut cursor)
        .find(|c| c.kind() == "statement_list");
    let Some(list) = list else {
        return Vec::new();
    };
    let mut cursor = list.walk();
    list.named_children(&mut cursor)
        .filter(|c| c.kind() != "comment")
        .collect()
}

fn sole_call(body: Ast) -> Option<Ast> {
    let [statement] = statements_of(body)[..] else {
        return None;
    };
    let expression = match statement.kind() {
        "expression_statement" => statement.named_child(0)?,
        "return_statement" => {
            let list = statement.named_child(0)?;
            if list.named_child_count() != 1 {
                return None;
            }
            list.named_child(0)?
        }
        _ => return None,
    };
    (expression.kind() == "call_expression").then_some(expression)
}

fn returns_only(clause: Ast) -> bool {
    let mut cursor = clause.walk();
    let list = clause
        .named_children(&mut cursor)
        .find(|c| c.kind() == "statement_list");
    list.is_some_and(|list| {
        let mut cursor = list.walk();
        let items: Vec<Ast> = list
            .named_children(&mut cursor)
            .filter(|c| c.kind() != "comment")
            .collect();
        items.len() == 1 && items[0].kind() == "return_statement"
    })
}

fn count_tokens(body: Ast) -> u32 {
    let leaves = descendants(body)
        .filter(|n| n.child_count() == 0 && n.kind() != "comment")
        .count();
    u32::try_from(leaves).unwrap_or(u32::MAX)
}

fn signature(function: Ast) -> (u32, u32) {
    let params = function
        .child_by_field_name("parameters")
        .map_or(0, |list| parameter_count(list));
    let returns = function.child_by_field_name("result").map_or(0, |result| {
        if result.kind() == "parameter_list" {
            parameter_count(result)
        } else {
            1
        }
    });
    (params, returns)
}

fn parameter_count(list: Ast) -> u32 {
    let mut cursor = list.walk();
    let count: usize = list
        .named_children(&mut cursor)
        .filter(|c| {
            matches!(
                c.kind(),
                "parameter_declaration" | "variadic_parameter_declaration"
            )
        })
        .map(|c| {
            let mut cursor = c.walk();
            c.children_by_field_name("name", &mut cursor).count().max(1)
        })
        .sum();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Parameter names in order; `None` when one is unnamed or blank.
fn parameter_names<'s>(function: Ast, source: &'s str) -> Option<Vec<&'s str>> {
    let Some(list) = function.child_by_field_name("parameters") else {
        return Some(Vec::new());
    };
    let mut names = Vec::new();
    let mut cursor = list.walk();
    for param in list.named_children(&mut cursor) {
        if !matches!(
            param.kind(),
            "parameter_declaration" | "variadic_parameter_declaration"
        ) {
            continue;
        }
        let mut inner = param.walk();
        let found: Vec<&str> = param
            .children_by_field_name("name", &mut inner)
            .map(|n| text(n, source))
            .collect();
        if found.is_empty() || found.contains(&"_") {
            return None;
        }
        names.extend(found);
    }
    Some(names)
}

fn argument_names<'s>(call: Ast, source: &'s str) -> Option<Vec<&'s str>> {
    let list = call.child_by_field_name("arguments")?;
    let mut cursor = list.walk();
    list.named_children(&mut cursor)
        .filter(|c| c.kind() != "comment")
        .map(|arg| {
            let arg = if arg.kind() == "variadic_argument" {
                arg.named_child(0)?
            } else {
                arg
            };
            (arg.kind() == "identifier").then(|| text(arg, source))
        })
        .collect()
}
