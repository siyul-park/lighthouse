//! Declarative rules over synthetic projects: every selector, the message and
//! evidence expressions, and the ways a rule file can be wrong.

#[path = "../../lighthouse-design/tests/support/mod.rs"]
mod support;

use std::collections::BTreeMap;

use lighthouse_declarative::Declarative;
use lighthouse_model::{EdgeKind, SymbolKind};
use lighthouse_spec::Catalog;
use serde_json::json;
use support::{Subject, World};

/// A local rule `local/probe` over `scope` with the rule file `rule`.
fn local(scope: &str, rule: &str) -> Result<Declarative, String> {
    let indented: String = rule.lines().map(|l| format!("  {l}\n")).collect();
    let text = format!(
        "id: local/probe\ntitle: Probe\nintent: A probe.\nscope: {scope}\nrequirement: A probe MUST hold.\nenforcement: mechanical\nevidence: [x]\nexamples:\n  - name: bad\n    language: text\n    kind: invalid\n    files: [{{ path: a.txt, body: x }}]\n    expect: [{{ line: 1 }}]\n  - name: good\n    language: text\n    kind: valid\n    files: [{{ path: a.txt, body: x }}]\nrule:\n{indented}"
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
            "select",
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
fn an_expression_that_fails_to_evaluate_fails_the_run_loudly() {
    let plugin = local(
        "symbol",
        "select: symbol\nwhere: 'symbol.nothing == 1'\nmessage: x",
    )
    .unwrap();
    let mut w = World::default();
    w.func("m", "Run", "m/a.ucm");
    let outcome = std::panic::catch_unwind(|| found(&plugin, &w));
    assert!(outcome.is_err(), "an evaluation error is not a silent pass");
}

#[test]
fn local_files_need_the_local_prefix_a_rule_section_and_inline_examples() {
    let files = |text: &str| BTreeMap::from([("x.yaml".to_owned(), text.to_owned())]);
    let bad_id = "id: other/x\ntitle: t\nintent: i\nscope: symbol\nrequirement: A MUST b.\nenforcement: doc\nrule: {}\n";
    assert!(
        Catalog::from_local(files(bad_id))
            .unwrap_err()
            .to_string()
            .contains("local/<name>")
    );
    let no_rule = "id: local/x\ntitle: t\nintent: i\nscope: symbol\nrequirement: A MUST b.\nenforcement: doc\n";
    assert!(
        Catalog::from_local(files(no_rule))
            .unwrap_err()
            .to_string()
            .contains("rule:")
    );
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
    assert!(!Declarative::bundled_rules("design").is_empty());
    assert!(Declarative::bundled_rules("elsewhere").is_empty());
}

#[test]
fn load_local() {
    let root = tempfile::tempdir().unwrap();
    assert!(
        lighthouse_declarative::load_local(root.path())
            .unwrap()
            .is_none()
    );

    let dir = root.path().join(".lighthouse/rules");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("notes.txt"), "not a rule").unwrap();
    std::fs::write(dir.join("bad.yaml"), "id: other/x\n").unwrap();
    assert!(lighthouse_declarative::load_local(root.path()).is_err());

    std::fs::remove_file(dir.join("bad.yaml")).unwrap();
    assert!(
        lighthouse_declarative::load_local(root.path())
            .unwrap()
            .is_some()
    );
}
