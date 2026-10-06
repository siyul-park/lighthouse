use std::collections::BTreeSet;

use lighthouse_model::{Target, TestCase, TestStyle};
use lighthouse_syntax::{Node, children_named, descendants, text};

use crate::declarations::{Function, is_test_case_name};

pub(crate) fn case(function: &Function, source: &str, targets: Vec<Target>) -> Option<TestCase> {
    if !function.test || !is_test_case_name(&function.name) {
        return None;
    }
    let handles = handles(function.node, source);
    let style = if is_table(function.node, source) {
        TestStyle::Table
    } else {
        TestStyle::Scenario
    };
    Some(TestCase {
        symbol: function.id.clone(),
        nesting: run_depth(function.node, source, &handles),
        style,
        targets,
    })
}

/// Names bound to a `*testing.T` (or `B`, `F`) by any parameter of the function.
fn handles(function: Node, source: &str) -> BTreeSet<String> {
    descendants(function)
        .filter(|n| n.kind() == "parameter_declaration")
        .filter(|n| {
            n.child_by_field_name("type")
                .is_some_and(|t| is_testing_pointer(t, source))
        })
        .flat_map(|n| {
            let mut cursor = n.walk();
            n.children_by_field_name("name", &mut cursor)
                .map(|name| text(name, source).to_owned())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn is_testing_pointer(node: Node, source: &str) -> bool {
    node.kind() == "pointer_type"
        && node.named_child(0).is_some_and(|inner| {
            inner.kind() == "qualified_type"
                && inner
                    .child_by_field_name("package")
                    .is_some_and(|p| text(p, source) == "testing")
        })
}

/// Deepest chain of `t.Run(name, func...)` calls inside one another.
fn run_depth(node: Node, source: &str, handles: &BTreeSet<String>) -> u32 {
    let mut cursor = node.walk();
    let inner = node
        .named_children(&mut cursor)
        .map(|c| run_depth(c, source, handles))
        .max()
        .unwrap_or(0);
    inner + u32::from(is_run(node, source, handles))
}

fn is_run(node: Node, source: &str, handles: &BTreeSet<String>) -> bool {
    if node.kind() != "call_expression" {
        return false;
    }
    let Some(function) = node.child_by_field_name("function") else {
        return false;
    };
    let (Some(operand), Some(field)) = (
        function.child_by_field_name("operand"),
        function.child_by_field_name("field"),
    ) else {
        return false;
    };
    let Some(args) = node.child_by_field_name("arguments") else {
        return false;
    };
    let last =
        args.named_child(u32::try_from(args.named_child_count().saturating_sub(1)).unwrap_or(0));
    function.kind() == "selector_expression"
        && text(field, source) == "Run"
        && handles.contains(text(operand, source))
        && args.named_child_count() >= 2
        && last.is_some_and(|l| l.kind() == "func_literal")
}

/// A `for range` over a collection literal of anonymous structs, directly or
/// through a variable bound to one.
fn is_table(function: Node, source: &str) -> bool {
    let tables: BTreeSet<&str> = descendants(function)
        .filter(|n| matches!(n.kind(), "short_var_declaration" | "var_spec"))
        .flat_map(|n| bindings(n, source))
        .collect();
    descendants(function)
        .filter(|n| n.kind() == "range_clause")
        .filter_map(|n| n.child_by_field_name("right"))
        .any(|right| {
            is_struct_collection(right)
                || (right.kind() == "identifier" && tables.contains(text(right, source)))
        })
}

fn bindings<'s>(node: Node, source: &'s str) -> Vec<&'s str> {
    let (names, values) = if node.kind() == "var_spec" {
        let mut cursor = node.walk();
        let names: Vec<Node> = node.children_by_field_name("name", &mut cursor).collect();
        (names, node.child_by_field_name("value"))
    } else {
        let left = node.child_by_field_name("left");
        let mut names = Vec::new();
        if let Some(left) = left {
            let mut cursor = left.walk();
            names.extend(left.named_children(&mut cursor));
        }
        (names, node.child_by_field_name("right"))
    };
    let Some(values) = values else {
        return Vec::new();
    };
    let mut cursor = values.walk();
    let values: Vec<Node> = values.named_children(&mut cursor).collect();
    names
        .into_iter()
        .zip(values)
        .filter(|(_, value)| is_struct_collection(*value))
        .map(|(name, _)| text(name, source))
        .collect()
}

fn is_struct_collection(node: Node) -> bool {
    if node.kind() != "composite_literal" {
        return false;
    }
    let Some(ty) = node.child_by_field_name("type") else {
        return false;
    };
    let element = match ty.kind() {
        "slice_type" | "array_type" => ty.child_by_field_name("element"),
        "map_type" => ty.child_by_field_name("value"),
        _ => None,
    };
    element.is_some_and(|e| {
        !children_named(e, "field_declaration_list").is_empty() || e.kind() == "struct_type"
    })
}
