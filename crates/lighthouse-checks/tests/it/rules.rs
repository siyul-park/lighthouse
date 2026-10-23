//! Checks rules over synthetic projects: every selector, the message and
//! evidence expressions, and the ways a rule file can be wrong.

use std::collections::BTreeMap;

use crate::support::{Subject, World};
use lighthouse_checks::Checks;
use lighthouse_model::ucm::{EdgeKind, SymbolKind};
use lighthouse_plugin::Plugin;
use lighthouse_spec::Catalog;
use serde_json::json;

/// A local decision `local/probe` over `scope` with the CEL check `check`: the
/// lines of its `select`, `where`, `message` and `evidence`.
pub(crate) fn local(scope: &str, check: &str) -> Result<Checks, String> {
    scoped(&format!("{{ subject: {scope}, tests: include }}"), check)
}

/// Like [`local`], with the whole `scope` written out.
pub(crate) fn scoped(scope: &str, check: &str) -> Result<Checks, String> {
    let indented: String = check.lines().map(|l| format!("    {l}\n")).collect();
    let text = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/probe\nspec:\n  title: Probe\n  context: A probe.\n  scope: {scope}\n  requirement: A probe MUST hold.\n  severity: error\n  check:\n    type: cel\n{indented}  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{{ path: a.txt, body: x }}]\n      expect: [{{ line: 1 }}]\n    - name: good\n      language: text\n      kind: valid\n      files: [{{ path: a.txt, body: x }}]\n"
    );
    let layer = Catalog::from_local(BTreeMap::from([("probe.yaml".to_owned(), text)]))
        .map_err(|e| e.to_string())?;
    Checks::new("local", &layer).map_err(|e| e.to_string())
}

pub(crate) fn found(plugin: &Checks, w: &World) -> Vec<(String, u32)> {
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
        "select: function\nwhere: 'func.statements > 5 && size(func.callers) == 0'\nmessage: 'dead and long: {{ func.name }}'",
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
        "select: module\nwhere: 'size(module.symbols) > 1'\nmessage: '{{ module.path }} has {{ size(module.symbols) }}'",
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
        "select: file\nwhere: 'size(file.symbols) > 1 && !file.test'\nmessage: '{{ file.path }}'",
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
        (
            "symbol",
            "select: symbol\nvariables:\n  - { name: a, expression: '1' }\n  - { name: a, expression: '2' }\nwhere: 'true'\nmessage: x",
            "`a` is defined twice",
        ),
        (
            "symbol",
            "select: symbol\nvariables:\n  - { name: node, expression: '1' }\nwhere: 'true'\nmessage: x",
            "`node` is not a usable name",
        ),
        (
            "symbol",
            "select: symbol\nvariables:\n  - { name: weight, params: [k], expression: 'k + symbol.span.start.line' }\nwhere: 'weight(1) > 0'\nmessage: x",
            "`symbol` is not a parameter",
        ),
        (
            "symbol",
            "select: symbol\nwhere: \"symbol.name == 'x' ? true : nope(1)\"\nmessage: x",
            "`nope` is not a function",
        ),
        (
            "symbol",
            "select: symbol\nwhere: \"symbol.name == 'x' ? true : symbol.name.startsWithh('a')\"\nmessage: x",
            "`startsWithh` is not a function",
        ),
        (
            "symbol",
            "select: symbol\nwhere: \"symbol.name == 'x' || regex.matchs(symbol.name, 'a')\"\nmessage: x",
            "`regex.matchs` is not a function",
        ),
        (
            "symbol",
            "select: symbol\nvariables:\n  - { name: plus, params: [n], expression: 'n + 1' }\nwhere: 'symbol.name == \"x\" ? true : plus(1, 2) > 0'\nmessage: x",
            "`plus` takes 1 argument(s), not 2",
        ),
        (
            "symbol",
            "select: symbol\nwhere: \"symbol.name == 'x' || glob.matches(symbol.name)\"\nmessage: x",
            "`glob.matches` takes 2 argument(s), not 1",
        ),
        (
            "symbol",
            "select: symbol\nwhere: \"symbol.name == 'x' || regex.replace(symbol.name, 'a') == ''\"\nmessage: x",
            "`regex.replace` takes 3 or 4 argument(s), not 2",
        ),
        (
            "symbol",
            "select: symbol\nwhere: \"symbol.name == 'x' || [1].misplaced(2).size() > 0\"\nmessage: x",
            "`misplaced` (called on a value) takes 0 argument(s), not 1",
        ),
        (
            "symbol",
            "select: symbol\nwhere: \"symbol.name == 'x' || misplaced([1]).size() > 0\"\nmessage: x",
            "`misplaced` is not a function",
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
fn a_variable_nothing_reads_is_not_evaluated() {
    let plugin = local(
        "symbol",
        "select: symbol\nvariables:\n  - { name: per_symbol, expression: 'symbol.nothing' }\n  - { name: per_file, expression: '1 / 0' }\nwhere: 'true'\nmessage: x",
    )
    .unwrap();
    let mut w = World::default();
    w.func("m", "Run", "m/a.ucm");
    assert_eq!(w.names(&found(&plugin, &w)), ["Run"]);
}

#[test]
fn a_file_variable_that_fails_when_read_is_the_error_naming_it() {
    let plugin = local(
        "symbol",
        "select: symbol\nvariables:\n  - { name: broken, expression: '1 / 0' }\nwhere: 'broken > 0'\nmessage: x",
    )
    .unwrap();
    let mut w = World::default();
    w.func("m", "Run", "m/a.ucm");
    let failed =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| found(&plugin, &w))).unwrap_err();
    let message = failed
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| failed.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default();
    assert!(message.contains("variable `broken`"), "{message}");
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
    let elsewhere = Checks::new("elsewhere", Catalog::bundled()).unwrap();
    assert!(elsewhere.is_empty());
}

#[test]
fn declarative_bundled_rules() {
    let rules = |pack: &str| Checks::new(pack, Catalog::bundled()).unwrap().rules();
    assert!(!rules("design").is_empty());
    assert!(rules("elsewhere").is_empty());
}

#[test]
fn a_module_of_test_code_is_a_subject_only_as_the_scope_says() {
    let check = "select: module\nwhere: 'true'\nmessage: '{{ module.path }}'";
    let mut w = World::default();
    w.module("prod", None, None);
    w.module("prod_tests", None, Some("prod"));
    w.func("prod", "A", "prod/a.ucm");
    w.func("prod_tests", "B", "prod_tests/a.ucm");
    let at = |scope: &str| -> Vec<String> {
        let plugin = scoped(scope, check).unwrap();
        found(&plugin, &w)
            .into_iter()
            .map(|(file, _)| file)
            .collect()
    };

    assert_eq!(at("{ subject: module }"), ["prod/a.ucm"]);
    assert_eq!(at("{ subject: module, tests: only }"), ["prod_tests/a.ucm"]);
    assert_eq!(at("{ subject: module, tests: include }").len(), 2);
}

#[test]
fn a_related_node_is_the_same_kind_of_value() {
    let plugin = local(
        "symbol",
        "select: function\nwhere: 'func.callers.exists(c, c.doc == \"\" && c.span.start.line > 0 && c.kind == \"function\")'\nmessage: '{{ func.name }} has an undocumented caller'",
    )
    .unwrap();
    let mut w = World::default();
    let used = w.func("m", "used", "m/a.ucm");
    let caller = w.func("m", "caller", "m/a.ucm");
    w.func("m", "alone", "m/a.ucm");
    w.edge(EdgeKind::Calls, &caller, &used);

    assert_eq!(w.names(&found(&plugin, &w)), ["used"]);
}

#[test]
#[should_panic(expected = "callers")]
fn a_relation_of_a_related_node_is_an_error_not_a_missing_value() {
    let plugin = local(
        "symbol",
        "select: function\nwhere: 'func.callers.exists(c, size(c.callers) == 0)'\nmessage: '{{ func.name }}'",
    )
    .unwrap();
    let mut w = World::default();
    let used = w.func("m", "used", "m/a.ucm");
    let caller = w.func("m", "caller", "m/a.ucm");
    w.edge(EdgeKind::Calls, &caller, &used);

    found(&plugin, &w);
}

#[test]
fn a_method_without_an_owner_is_not_hidden_by_it() {
    // What `design/signature` asks: a method is reachable unless its owner is
    // private. With no owner there is nothing to hide it, as it was when the
    // owner of a node without one had an empty visibility.
    let plugin = local(
        "symbol",
        "select: function\nwhere: 'func.kind == \"method\" && (func.owner == null || func.owner.visibility != \"private\")'\nmessage: '{{ func.name }} is reachable'",
    )
    .unwrap();
    let mut w = World::default();
    let orphan = w.symbol("m", "orphan", SymbolKind::Method, "m/a.ucm");
    w.summarize(&orphan, 1, &[]);
    let hidden_owner = w.symbol("m", "Hidden", SymbolKind::Type, "m/a.ucm");
    w.private(&hidden_owner);
    let hidden = w.member(&hidden_owner, "hidden", SymbolKind::Method, "m/a.ucm");
    w.summarize(&hidden, 1, &[]);
    let open_owner = w.symbol("m", "Open", SymbolKind::Type, "m/a.ucm");
    let open = w.member(&open_owner, "open", SymbolKind::Method, "m/a.ucm");
    w.summarize(&open, 1, &[]);

    assert_eq!(w.names(&found(&plugin, &w)), ["orphan", "open"]);
}

/// The decision `local/probe` of `local`, as the catalog holds it.
fn probe(scope: &str, check: &str) -> lighthouse_spec::Decision {
    let indented: String = check.lines().map(|l| format!("    {l}\n")).collect();
    let text = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/probe\nspec:\n  title: Probe\n  context: A probe.\n  scope: {{ subject: {scope}, tests: include }}\n  requirement: A probe MUST hold.\n  severity: error\n  check:\n    type: cel\n{indented}  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{{ path: a.txt, body: x }}]\n      expect: [{{ line: 1 }}]\n    - name: good\n      language: text\n      kind: valid\n      files: [{{ path: a.txt, body: x }}]\n"
    );
    let layer = Catalog::from_local(BTreeMap::from([("probe.yaml".to_owned(), text)])).unwrap();
    layer.decision("local/probe").unwrap().clone()
}

#[test]
fn the_check_revision_names_the_libraries_its_expressions_call() {
    let globs = probe(
        "symbol",
        "select: symbol\nwhere: \"glob.matches(symbol.module, 'a/**')\"\nmessage: x",
    );
    let (used, revision) = lighthouse_checks::probe::revision(&globs, |_| None).unwrap();
    assert_eq!(
        used.keys().collect::<Vec<_>>(),
        ["cel/glob"],
        "only the library it calls"
    );

    let (_, bumped) = lighthouse_checks::probe::revision(&globs, |id| {
        (id == "cel/glob").then(|| "9.9.9".to_owned())
    })
    .unwrap();
    assert_ne!(
        revision, bumped,
        "a used library's version is in the revision"
    );

    let (_, other) = lighthouse_checks::probe::revision(&globs, |id| {
        (id == "cel/regex").then(|| "9.9.9".to_owned())
    })
    .unwrap();
    assert_eq!(revision, other, "an unused library's version is not");
}

#[test]
fn a_decision_that_calls_no_library_has_a_revision_no_library_changes() {
    let plain = probe(
        "symbol",
        "select: symbol\nwhere: \"symbol.name.startsWith('a')\"\nmessage: x",
    );
    let (used, revision) = lighthouse_checks::probe::revision(&plain, |_| None).unwrap();
    assert!(used.is_empty());
    let (_, bumped) =
        lighthouse_checks::probe::revision(&plain, |_| Some("9.9.9".to_owned())).unwrap();
    assert_eq!(revision, bumped);
}

#[test]
fn the_function_catalog_is_what_the_plugin_registers_and_the_runtime_has() {
    let catalog = lighthouse_checks::catalog().unwrap();
    let registry = lighthouse_checks::registry();
    let registered: Vec<String> = registry
        .cel_functions()
        .iter()
        .map(|c| {
            format!(
                "{} {} {:?}",
                c.library, c.function.name, c.function.receiver
            )
        })
        .collect();
    let composed: Vec<String> = catalog
        .iter()
        .map(|c| {
            format!(
                "{} {} {:?}",
                c.library, c.function.name, c.function.receiver
            )
        })
        .collect();
    assert_eq!(registered, composed);
    for entry in catalog {
        let call = match &entry.function.receiver {
            Some(_) => format!("[1].{}", entry.function.name),
            None => entry.function.name.clone(),
        };
        let args = entry.function.overloads[0].params.len();
        if entry.function.name == "cel.bind" {
            continue;
        }
        let args = vec!["null"; args].join(", ");
        let source = format!("{call}({args})");
        let outcome = lighthouse_spec::compile(&source)
            .unwrap()
            .execute(&lighthouse_spec::context().unwrap());
        assert!(
            !matches!(outcome, Err(cel::ExecutionError::UndeclaredReference(_))),
            "{source}: {outcome:?}"
        );
    }
}
