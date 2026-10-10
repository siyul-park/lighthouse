//! Declarative rules over synthetic projects: every selector, the message and
//! evidence expressions, and the ways a rule file can be wrong.

use std::collections::BTreeMap;

use crate::support::{Subject, World};
use lighthouse_checks::Declarative;
use lighthouse_model::{EdgeKind, SymbolKind};
use lighthouse_plugin::Plugin;
use lighthouse_spec::Catalog;
use serde_json::json;

/// A local decision `local/probe` over `scope` with the CEL check `check`: the
/// lines of its `select`, `where`, `message` and `evidence`.
fn local(scope: &str, check: &str) -> Result<Declarative, String> {
    let indented: String = check.lines().map(|l| format!("    {l}\n")).collect();
    let text = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/probe\nspec:\n  title: Probe\n  context: A probe.\n  scope: {{ subject: {scope}, tests: include }}\n  requirement: A probe MUST hold.\n  severity: error\n  check:\n    type: cel\n{indented}  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{{ path: a.txt, body: x }}]\n      expect: [{{ line: 1 }}]\n    - name: good\n      language: text\n      kind: valid\n      files: [{{ path: a.txt, body: x }}]\n"
    );
    let layer = Catalog::from_local(BTreeMap::from([("probe.yaml".to_owned(), text)]))
        .map_err(|e| e.to_string())?;
    Declarative::from_catalog("local", &layer).map_err(|e| e.to_string())
}

fn found(plugin: &Declarative, w: &World) -> Vec<(String, u32)> {
    w.check_in(
        &Subject {
            plugin,
            id: "local",
        },
        "local/probe",
        json!({}),
    )
}

#[test]
fn symbols_are_selected_with_their_facts() {
    let plugin = local(
        "symbol",
        "select: symbol\nwhere: 'symbol.kind == \"var\" && symbol.visibility == \"public\"'\nmessage: 'public var {{ symbol.name }} in {{ symbol.module }}'\nevidence:\n  name: symbol.name",
    )
    .unwrap();
    let mut w = World::default();
    w.symbol("m", "Shared", SymbolKind::Var, "m/a.ucm");
    let hidden = w.symbol("m", "hidden", SymbolKind::Var, "m/a.ucm");
    w.private(&hidden);
    w.func("m", "Run", "m/a.ucm");
    assert_eq!(w.names(&found(&plugin, &w)), ["Shared"]);
}

#[test]
fn functions_carry_their_summary_and_callers() {
    let plugin = local(
        "symbol",
        "select: function\nwhere: 'func.statements > 5 && func.callers == 0'\nmessage: 'dead and long: {{ func.name }}'",
    )
    .unwrap();
    let mut w = World::default();
    let long = w.func("m", "long", "m/a.ucm");
    w.summarize(&long, 9, &[]);
    let used = w.func("m", "used", "m/a.ucm");
    w.summarize(&used, 9, &[]);
    let caller = w.func("m", "caller", "m/a.ucm");
    w.edge(EdgeKind::Calls, &caller, &used);
    assert_eq!(w.names(&found(&plugin, &w)), ["long"]);
}

#[test]
fn edges_are_judged_once_over_the_project() {
    let plugin = local(
        "project",
        "select: edge\nwhere: 'edge.kind == \"calls\" && edge.from.module.startsWith(\"domain\") && edge.to.module.startsWith(\"infra\")'\nmessage: '{{ edge.from.name }} reaches {{ edge.to.module }}'",
    )
    .unwrap();
    let mut w = World::default();
    let domain = w.func("domain", "Run", "domain/a.ucm");
    let infra = w.func("infra", "Save", "infra/a.ucm");
    let other = w.func("app", "Main", "app/a.ucm");
    w.edge(EdgeKind::Calls, &domain, &infra);
    w.edge(EdgeKind::Calls, &other, &infra);
    w.edge(EdgeKind::References, &domain, &infra);
    assert_eq!(w.names(&found(&plugin, &w)), ["Run"]);
}

#[test]
fn modules_and_files_are_selected_too() {
    let modules = local(
        "project",
        "select: module\nwhere: 'module.symbols > 1'\nmessage: '{{ module.path }} has {{ module.symbols }}'",
    )
    .unwrap();
    let mut w = World::default();
    w.module("big", None, None);
    w.module("small", None, None);
    w.func("big", "A", "big/a.ucm");
    w.func("big", "B", "big/a.ucm");
    w.func("small", "C", "small/a.ucm");
    assert_eq!(found(&modules, &w), [("big/a.ucm".to_owned(), 1)]);

    let files = local(
        "symbol",
        "select: file\nwhere: 'file.symbols > 1 && !file.test'\nmessage: '{{ file.path }}'",
    )
    .unwrap();
    assert_eq!(found(&files, &w), [("big/a.ucm".to_owned(), 1)]);
}

#[test]
fn tests_are_selected_with_their_targets() {
    let plugin = local(
        "test",
        "select: test\nwhere: 'test.target_count == 0'\nmessage: '{{ test.name }} tests nothing'",
    )
    .unwrap();
    let mut w = World::default();
    let target = w.func("m", "Get", "m/a.ucm");
    let empty = w.symbol("m", "TestNothing", SymbolKind::Test, "m/a_test.ucm");
    let full = w.symbol("m", "TestGet", SymbolKind::Test, "m/a_test.ucm");
    w.test_case(&empty, &[]);
    w.test_case(&full, &[&target]);
    assert_eq!(w.names(&found(&plugin, &w)), ["TestNothing"]);
}

#[test]
fn error() {
    let cases = [
        (
            "symbol",
            "select: symbol\nwhere: 'symbol.name =='\nmessage: x",
            "where",
        ),
        (
            "symbol",
            "select: symbol\nwhere: 'true'\nmessage: '{{ symbol.name'",
            "never closed",
        ),
        (
            "symbol",
            "select: symbol\nwhere: 'true'\nmessage: '{{ ( }}'",
            "message",
        ),
        (
            "symbol",
            "select: nonsense\nwhere: 'true'\nmessage: x",
            "nonsense",
        ),
        (
            "symbol",
            "select: edge\nwhere: 'true'\nmessage: x",
            "cannot run over",
        ),
        (
            "symbol",
            "select: symbol\nwhere: 'true'\nmessage: x\nextra: 1",
            "extra",
        ),
    ];
    for (scope, rule, reason) in cases {
        let error = local(scope, rule).err().unwrap_or_default();
        assert!(error.contains(reason), "`{reason}` not in `{error}`");
    }
}

#[test]
fn an_expression_with_an_execution_error_leaves_the_analysis_incomplete() {
    let plugin = local(
        "symbol",
        "select: symbol\nwhere: 'symbol.nothing == 1'\nmessage: x",
    )
    .unwrap();
    let mut w = World::default();
    w.func("m", "Run", "m/a.ucm");
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| found(&plugin, &w)));
    assert!(outcome.is_err(), "an execution error is not a silent pass");
}

#[test]
fn local_files_need_the_local_prefix_and_a_cel_check_and_inline_examples() {
    let files = |text: &str| BTreeMap::from([("x.yaml".to_owned(), text.to_owned())]);
    let header = |name: &str| {
        format!(
            "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: {name}\nspec:\n  title: t\n  context: i\n  scope: {{ subject: symbol }}\n  requirement: A MUST b.\n"
        )
    };
    assert!(
        Catalog::from_local(files(&header("other/x")))
            .unwrap_err()
            .to_string()
            .contains("local/<name>")
    );
    let builtin = format!(
        "{}  check:\n    type: builtin\n    id: local/x\n",
        header("local/x").replace("doc", "judgment")
    );
    assert!(
        Catalog::from_local(files(&builtin))
            .unwrap_err()
            .to_string()
            .contains("standard operation")
    );
    let sourced = format!(
        "{}  examples:\n    - name: e\n      language: text\n      kind: valid\n      files: [{{ path: a, source: a.txt }}]\n",
        header("local/x")
    );
    assert!(
        Catalog::from_local(files(&sourced))
            .unwrap_err()
            .to_string()
            .contains("does not exist")
    );
    assert!(Catalog::from_local(files(&header("local/x"))).is_ok());
}

#[test]
fn declarative_is_empty() {
    let probe = local("symbol", "select: symbol\nwhere: 'true'\nmessage: x").unwrap();
    assert!(!probe.is_empty());
    let elsewhere = Declarative::from_catalog("elsewhere", Catalog::bundled()).unwrap();
    assert!(elsewhere.is_empty());
}

#[test]
fn declarative_bundled_rules() {
    let rules = |pack: &str| {
        Declarative::from_catalog(pack, Catalog::bundled())
            .unwrap()
            .rules()
    };
    assert!(!rules("design").is_empty());
    assert!(rules("elsewhere").is_empty());
}
