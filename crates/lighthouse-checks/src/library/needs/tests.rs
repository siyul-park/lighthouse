//! What the fields and calls found in an expression are.

use super::*;

fn of(source: &str) -> Needs {
    Needs::of(&lighthouse_spec::Env::standard().unwrap(), [source], [])
}

#[test]
fn a_selected_field_is_read() {
    let needs = of("node.owner != null");

    assert!(needs.field("owner"));
    assert!(!needs.field("callers"));
}

#[test]
fn an_optional_selection_reads_the_field() {
    let needs = of("node.?owner.orValue(null) == null");

    assert!(needs.field("owner"));
}

#[test]
fn a_string_index_reads_the_field() {
    let needs = of("node['owner'] != null && node[?'doc'].hasValue()");

    assert!(needs.field("owner"));
    assert!(needs.field("doc"));
    assert!(!needs.field("callers"));
}

#[test]
fn a_computed_index_may_read_any_field() {
    let needs = of("node[key] == 1");

    assert!(needs.field("callers"));
    assert!(needs.field("anything"));
}

#[test]
fn a_computed_index_into_the_options_reads_no_field() {
    let needs = of("options.max[func.role] > 1");

    assert!(needs.field("role"));
    assert!(!needs.field("callers"));
}

#[test]
fn a_position_in_a_list_reads_no_field() {
    let needs = of("node.callers[0] == null");

    assert!(needs.field("callers"));
    assert!(!needs.field("anything"));
}

#[test]
fn a_comprehension_reads_inside_its_range_and_body() {
    let needs = of("node.callers.exists(c, c.members.all(m, m.doc == '' && c.owner != null))");

    for field in ["callers", "members", "doc", "owner"] {
        assert!(needs.field(field), "{field}");
    }
    assert!(!needs.field("callees"));
}

#[test]
fn the_holes_of_a_template_are_read() {
    let needs = Needs::of(
        &lighthouse_spec::Env::standard().unwrap(),
        [],
        ["{{ node.name }} is {{ node.kind }} in prose"],
    );

    assert!(needs.field("name"));
    assert!(needs.field("kind"));
    assert!(!needs.field("prose"));
}

#[test]
fn a_word_inside_a_string_or_a_longer_name_is_not_a_field() {
    let needs = of("node.test_role == 'node.owner'");

    assert!(needs.field("test_role"));
    assert!(!needs.field("owner"));
    assert!(!needs.field("role"));
}

#[test]
fn an_index_computed_from_a_size_is_a_position_not_a_field() {
    let needs = of("l[size(l) - 1].doc");
    assert!(needs.field("doc"));
    assert!(!needs.field("callers"));
}

#[test]
fn a_related_node_needs_only_what_is_read_from_something_else_than_the_subject() {
    let needs = of("func.metrics.lines > 3 && symbol.callers.exists(c, c.doc == '')");
    let related = needs.of_related();
    assert!(needs.field("metrics") && needs.field("callers"));
    assert!(related.field("doc"));
    assert!(!related.field("metrics") && !related.field("callers"));
}

#[test]
fn a_variable_over_a_range_is_a_position_in_a_list() {
    let needs = of("lists.range(size(l)).filter(i, l[i].x > l[i + 1].x)");

    assert!(needs.field("x"));
    assert!(!needs.field("callers"));
}

#[test]
fn a_variable_over_anything_else_may_name_a_field() {
    let needs = of("keys.filter(k, node[k] == 1)");

    assert!(needs.field("callers"));
}

#[test]
fn a_variable_over_a_filter_of_a_range_is_a_position_too() {
    let needs = of("lists.range(size(l)).filter(i, i in kept).map(i, l[i].x)");

    assert!(needs.field("x"));
    assert!(!needs.field("callers"));
}

#[test]
fn a_variable_over_a_map_of_a_range_may_name_a_field() {
    let needs = of("lists.range(3).map(i, keys[i]).map(k, node[k])");

    assert!(needs.field("callers"));
}
