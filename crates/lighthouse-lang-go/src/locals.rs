use std::collections::BTreeMap;

use lighthouse_syntax::{Node, descendants, text};

use crate::imports::Aliases;

/// Names declared anywhere inside one function, with the type path
/// (`module::Type`) when one can be read from the syntax. Scoping is
/// flow-insensitive: a name declared once is local in the whole function.
pub(crate) struct Locals(BTreeMap<String, Option<String>>);

pub(crate) struct Names<'a> {
    pub source: &'a str,
    pub module: &'a str,
    pub aliases: &'a Aliases,
}

impl Locals {
    pub(crate) fn collect(function: Node, names: &Names) -> Self {
        let mut locals = Self(BTreeMap::new());
        for node in descendants(function) {
            match node.kind() {
                "parameter_declaration" | "variadic_parameter_declaration" => {
                    let ty = node
                        .child_by_field_name("type")
                        .and_then(|t| type_path(t, names));
                    locals.declare_all(node, "name", names, |_| ty.clone());
                }
                "short_var_declaration" => {
                    let values = list(node.child_by_field_name("right"));
                    let ty = |i: usize, count: usize| {
                        if values.len() == count {
                            value_type(values[i], names)
                        } else {
                            None
                        }
                    };
                    locals.declare_list(node.child_by_field_name("left"), names, ty);
                }
                "var_spec" | "const_spec" => {
                    let declared = node
                        .child_by_field_name("type")
                        .and_then(|t| type_path(t, names));
                    let values = list(node.child_by_field_name("value"));
                    let mut at = 0;
                    locals.declare_all(node, "name", names, |_| {
                        let ty = declared
                            .clone()
                            .or_else(|| value_type(*values.get(at)?, names));
                        at += 1;
                        ty
                    });
                }
                "range_clause" | "receive_statement" => {
                    locals.declare_list(node.child_by_field_name("left"), names, |_, _| None);
                }
                "type_switch_statement" => {
                    locals.declare_list(node.child_by_field_name("alias"), names, |_, _| None);
                }
                "type_spec" => locals.declare_all(node, "name", names, |_| None),
                _ => {}
            }
        }
        locals
    }

    /// `Some(None)` for a local without a known type.
    pub(crate) fn get(&self, name: &str) -> Option<&Option<String>> {
        self.0.get(name)
    }

    fn declare(&mut self, name: &str, ty: Option<String>) {
        if name == "_" {
            return;
        }
        match self.0.get_mut(name) {
            Some(existing) if *existing != ty => *existing = None,
            Some(_) => {}
            None => {
                self.0.insert(name.to_owned(), ty);
            }
        }
    }

    fn declare_all(
        &mut self,
        node: Node,
        field: &str,
        names: &Names,
        mut ty: impl FnMut(usize) -> Option<String>,
    ) {
        let mut cursor = node.walk();
        let idents: Vec<Node> = node
            .children_by_field_name(field, &mut cursor)
            .filter(|n| n.kind() == "identifier")
            .collect();
        for (i, ident) in idents.into_iter().enumerate() {
            self.declare(text(ident, names.source), ty(i));
        }
    }

    fn declare_list(
        &mut self,
        left: Option<Node>,
        names: &Names,
        ty: impl Fn(usize, usize) -> Option<String>,
    ) {
        let idents: Vec<Node> = list(left);
        let count = idents.len();
        for (i, ident) in idents.into_iter().enumerate() {
            if ident.kind() == "identifier" {
                self.declare(text(ident, names.source), ty(i, count));
            }
        }
    }
}

fn list(node: Option<Node>) -> Vec<Node> {
    node.map_or_else(Vec::new, |n| {
        let mut cursor = n.walk();
        n.named_children(&mut cursor)
            .filter(|c| c.kind() != "comment")
            .collect()
    })
}

/// `module::Type` of a type expression behind pointers and type arguments;
/// types of other packages resolve through the import aliases.
pub(crate) fn type_path(node: Node, names: &Names) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(format!("{}::{}", names.module, text(node, names.source))),
        "pointer_type" | "parenthesized_type" => type_path(node.named_child(0)?, names),
        "generic_type" => type_path(node.child_by_field_name("type")?, names),
        "qualified_type" => {
            let package = text(node.child_by_field_name("package")?, names.source);
            let name = text(node.child_by_field_name("name")?, names.source);
            let module = names.aliases.get(package)?;
            Some(format!("{module}::{name}"))
        }
        _ => None,
    }
}

fn value_type(node: Node, names: &Names) -> Option<String> {
    match node.kind() {
        "composite_literal" => type_path(node.child_by_field_name("type")?, names),
        "parenthesized_expression" => value_type(node.named_child(0)?, names),
        "unary_expression" if text(node.child_by_field_name("operator")?, names.source) == "&" => {
            value_type(node.child_by_field_name("operand")?, names)
        }
        "call_expression" => {
            let function = node.child_by_field_name("function")?;
            if text(function, names.source) != "new" {
                return None;
            }
            let arg = node.child_by_field_name("arguments")?.named_child(0)?;
            if arg.kind() == "identifier" {
                return Some(format!("{}::{}", names.module, text(arg, names.source)));
            }
            type_path(arg, names)
        }
        _ => None,
    }
}
