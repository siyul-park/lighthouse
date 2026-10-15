//! The facts that let a cel check read what a builtin op reads today: `gap`
//! of a method, and the function fact of a symbol (`symbol.func`).

use std::collections::{BTreeMap, BTreeSet};

use crate::support::{Subject, World};
use lighthouse_checks::Declarative;
use lighthouse_model::SymbolKind;
use lighthouse_spec::Catalog;
use serde_json::{Value, json};

const CLOSE: &str = "design/layout";

/// the cel check of a function that the `gap` kind of `design/layout`
/// stands for.
const CONTIGUITY_AS_CEL: &str = "func.gap.between.exists(b, b.kind != 'method' || (b.owner_key != func.owner_key && (!options.visibilityGroups || (b.visibility != 'private') == (func.visibility != 'private'))))";

const EXAMPLES: &str = "  examples:
    - name: bad
      language: text
      kind: invalid
      files: [{ path: a.txt, body: x }]
      expect: [{ line: 1 }]
    - name: good
      language: text
      kind: valid
      files: [{ path: a.txt, body: x }]
";

/// The `(file, line)` of what a cel check `where` reports over `world`, for
/// the subject `select`.
pub(crate) fn flagged(
    world: &World,
    select: &str,
    condition: &str,
    options: &Value,
) -> BTreeSet<(String, u32)> {
    flagged_in(world, select, "", condition, options)
}

/// Like [`flagged`], for a decision whose scope also says `extra`, such as
/// `tests: include`.
pub(crate) fn flagged_in(
    world: &World,
    select: &str,
    extra: &str,
    condition: &str,
    options: &Value,
) -> BTreeSet<(String, u32)> {
    let subject = if select == "module" { "module" } else { "file" };
    let text = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/facts\n  labels:\n    lighthouse/pack: local\n    lighthouse/section: rules\nspec:\n  title: Facts\n  context: A check.\n  scope: {{ subject: {subject}{extra} }}\n  requirement: A check MUST hold.\n  severity: warn\n  options:\n    type: object\n    properties:\n      visibilityGroups: {{ type: boolean, default: true, description: Groups by visibility. }}\n    additionalProperties: false\n  check:\n    type: cel\n    select: {select}\n    where: {}\n    message: \"{{{{ {} }}}}\"\n{EXAMPLES}",
        serde_json::to_string(condition).unwrap(),
        match select {
            "function" => "func.name",
            "module" => "module.path",
            _ => "symbol.name",
        },
    );
    let catalog = Catalog::from_local(BTreeMap::from([("facts.yaml".to_owned(), text)])).unwrap();
    let plugin = Declarative::from_catalog("local", &catalog).unwrap();
    world
        .diagnose_with(
            &catalog,
            &Subject {
                plugin: &plugin,
                id: "local",
            },
            "local/facts",
            options.clone(),
        )
        .into_iter()
        .map(|d| (d.file.to_string_lossy().into_owned(), d.span.start.line))
        .collect()
}

fn contiguity(world: &World, options: &Value) -> BTreeSet<(String, u32)> {
    let mut gap = options.clone();
    gap["kinds"] = json!(["gap"]);
    world.check(CLOSE, gap).into_iter().collect()
}

/// A file with the shapes the contiguity examples and tests use, one module
/// per file: foreign methods, a free function, and private methods of another
/// owner between the methods of `A`.
fn crowded() -> World {
    let mut w = World::default();
    let a = w.symbol("m", "A", SymbolKind::Type, "m/a.ucm");
    let b = w.symbol("m", "B", SymbolKind::Type, "m/a.ucm");
    w.member(&a, "One", SymbolKind::Method, "m/a.ucm");
    w.member(&b, "One", SymbolKind::Method, "m/a.ucm");
    w.member(&a, "Two", SymbolKind::Method, "m/a.ucm");
    w.func("m", "Free", "m/a.ucm");
    w.member(&a, "Three", SymbolKind::Method, "m/a.ucm");
    let hidden = w.member(&b, "hidden", SymbolKind::Method, "m/a.ucm");
    w.private(&hidden);
    w.member(&a, "Four", SymbolKind::Method, "m/a.ucm");
    let own = w.member(&a, "own", SymbolKind::Method, "m/a.ucm");
    w.private(&own);
    w.member(&b, "Two", SymbolKind::Method, "m/a.ucm");
    w.member(&a, "Five", SymbolKind::Method, "m/a.ucm");
    let c = w.symbol("n", "C", SymbolKind::Type, "n/c.ucm");
    w.member(&c, "One", SymbolKind::Method, "n/c.ucm");
    w.member(&c, "Two", SymbolKind::Method, "n/c.ucm");
    w
}

#[test]
fn gap_stands_for_the_layout_separator_with_visibility_groups() {
    let w = crowded();
    let options = json!({ "visibilityGroups": true });
    let expected = contiguity(&w, &options);
    assert!(!expected.is_empty(), "the world has separated methods");
    assert_eq!(
        flagged(&w, "function", CONTIGUITY_AS_CEL, &options),
        expected
    );
}

#[test]
fn gap_stands_for_the_layout_separator_without_visibility_groups() {
    let w = crowded();
    let options = json!({ "visibilityGroups": false });
    let expected = contiguity(&w, &options);
    assert_ne!(
        expected,
        contiguity(&w, &json!({ "visibilityGroups": true })),
        "the option changes what is flagged"
    );
    assert_eq!(
        flagged(&w, "function", CONTIGUITY_AS_CEL, &options),
        expected
    );
}

#[test]
fn gap_stands_for_the_layout_separator_on_adjacent_methods() {
    let mut w = World::default();
    let a = w.symbol("m", "A", SymbolKind::Type, "m/a.ucm");
    let b = w.symbol("m", "B", SymbolKind::Type, "m/a.ucm");
    w.member(&a, "One", SymbolKind::Method, "m/a.ucm");
    w.member(&a, "Two", SymbolKind::Method, "m/a.ucm");
    w.member(&b, "One", SymbolKind::Method, "m/a.ucm");
    for groups in [true, false] {
        let options = json!({ "visibilityGroups": groups });
        assert!(contiguity(&w, &options).is_empty());
        assert!(flagged(&w, "function", CONTIGUITY_AS_CEL, &options).is_empty());
    }
}

#[test]
fn gap_is_empty_for_the_first_method_of_an_owner_and_for_a_free_function() {
    let mut w = World::default();
    let a = w.symbol("m", "A", SymbolKind::Type, "m/a.ucm");
    let free = w.func("m", "Free", "m/a.ucm");
    let one = w.member(&a, "One", SymbolKind::Method, "m/a.ucm");
    let options = json!({ "visibilityGroups": true });
    let empty = "func.gap.previous == null && func.gap.between.size() == 0";
    let found = flagged(&w, "function", empty, &options);
    let lines: BTreeSet<u32> = found.iter().map(|(_, line)| *line).collect();
    assert_eq!(
        lines,
        BTreeSet::from([free.span.start.line, one.span.start.line])
    );
}

#[test]
fn gap_names_the_previous_method_and_what_lies_between() {
    let mut w = World::default();
    let a = w.symbol("m", "A", SymbolKind::Type, "m/a.ucm");
    let one = w.member(&a, "One", SymbolKind::Method, "m/a.ucm");
    w.func("m", "Free", "m/a.ucm");
    let two = w.member(&a, "Two", SymbolKind::Method, "m/a.ucm");
    let condition = format!(
        "func.gap.previous == '{}' && func.gap.between.map(b, b.name) == ['Free']",
        one.id.as_str()
    );
    let found = flagged(&w, "function", &condition, &json!({}));
    assert_eq!(
        found,
        BTreeSet::from([("m/a.ucm".to_owned(), two.span.start.line)])
    );
}

#[test]
fn symbol_func_is_the_function_fact_of_a_function_and_null_of_a_type() {
    let mut w = World::default();
    let ty = w.symbol("m", "Thing", SymbolKind::Type, "m/a.ucm");
    let big = w.func("m", "Big", "m/a.ucm");
    let small = w.func("m", "Small", "m/a.ucm");
    w.summarize(&big, 9, &[]);
    w.summarize(&small, 1, &[]);
    let options = json!({});
    let lines = |condition| -> BTreeSet<u32> {
        flagged(&w, "symbol", condition, &options)
            .into_iter()
            .map(|(_, line)| line)
            .collect()
    };
    assert_eq!(
        lines("symbol.func != null && symbol.func.statements > 5"),
        BTreeSet::from([big.span.start.line])
    );
    assert_eq!(
        lines("symbol.func == null"),
        BTreeSet::from([ty.span.start.line])
    );
}

#[test]
fn symbol_func_reads_the_same_facts_as_a_function_select() {
    let w = crowded();
    let options = json!({ "visibilityGroups": true });
    let by_symbol = "symbol.func != null && symbol.func.gap.between.exists(b, b.kind != 'method' || (b.owner_key != symbol.func.owner_key && (!options.visibilityGroups || (b.visibility != 'private') == (symbol.func.visibility != 'private'))))";
    assert_eq!(
        flagged(&w, "symbol", by_symbol, &options),
        flagged(&w, "function", CONTIGUITY_AS_CEL, &options)
    );
}
