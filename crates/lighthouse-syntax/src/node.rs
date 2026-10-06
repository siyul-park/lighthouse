use lighthouse_model::{Position, Span};
use tree_sitter::Node;

pub fn text<'s>(node: Node, source: &'s str) -> &'s str {
    source.get(node.byte_range()).unwrap_or_default()
}

/// 1-based span; the end column is exclusive.
pub fn span(node: Node) -> Span {
    let at = |p: tree_sitter::Point| Position {
        line: u32::try_from(p.row + 1).unwrap_or(u32::MAX),
        col: u32::try_from(p.column + 1).unwrap_or(u32::MAX),
    };
    Span {
        start: at(node.start_position()),
        end: at(node.end_position()),
    }
}

/// Named children of `kind`, in order.
pub fn children_named<'t>(node: Node<'t>, kind: &str) -> Vec<Node<'t>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|c| c.kind() == kind)
        .collect()
}

/// Pre-order descendants of `node`, excluding `node`.
pub fn descendants(node: Node) -> impl Iterator<Item = Node> {
    let mut cursor = node.walk();
    let mut done = !cursor.goto_first_child();
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        let current = cursor.node();
        if cursor.goto_first_child() {
            return Some(current);
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() || cursor.node() == node {
                done = true;
                break;
            }
        }
        Some(current)
    })
}
