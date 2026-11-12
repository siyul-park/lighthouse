//! The query a decision translates to agrees with the decision on the
//! subjects of synthetic projects: a probe decision that reports every subject
//! of the decision's own kind hands out its fact value, and the query must
//! hold of exactly the subjects the shipped decision reports.

use std::collections::BTreeMap;

use crate::support::{Subject, World};
use lighthouse_checks::Checks;
use lighthouse_model::{
    RunScope,
    ucm::{EdgeKind, SymbolKind},
};
use lighthouse_spec::{Catalog, CheckKind};
use serde_json::{Map, Value, json};

/// A project with types, functions of each visibility, methods, a constant, a
/// variable, calls, comments, a test and modules.
fn crowded() -> World {
    let mut w = World::default();
    let widget = w.symbol("m", "Widget", SymbolKind::Type, "m/a.ucm");
    let run = w.func("m", "Run", "m/a.ucm");
    let helper = w.func("m", "helper", "m/a.ucm");
    w.private(&helper);
    let unused = w.func("m", "unusedHelper", "m/a.ucm");
    w.private(&unused);
    let method = w.member(&widget, "Do", SymbolKind::Method, "m/a.ucm");
    let hidden = w.member(&widget, "inner", SymbolKind::Method, "m/a.ucm");
    w.private(&hidden);
    let limit = w.symbol("m", "limit", SymbolKind::Const, "m/a.ucm");
    w.private(&limit);
    let state = w.symbol("m", "state", SymbolKind::Var, "m/b.ucm");
    w.private(&state);
    let field = w.member(&widget, "ctx", SymbolKind::Field, "m/a.ucm");
    w.private(&field);
    let other = w.symbol("n", "Other", SymbolKind::Type, "n/c.ucm");
    let user = w.func("n", "UseWidget", "n/c.ucm");
    w.edge(EdgeKind::Calls, &run, &helper);
    w.edge(EdgeKind::Calls, &method, &helper);
    w.edge(EdgeKind::Calls, &user, &run);
    w.edge(EdgeKind::References, &run, &limit);
    w.edge(EdgeKind::References, &user, &other);
    w.comment("m/a.ucm", 2, "TODO: split this");
    w.comment("m/a.ucm", 9, "returns the widget");
    w.comment("m/b.ucm", 11, "----- banner -----");
    let test = w.func("m", "TestRun", "m/a_test.ucm");
    w.test_case(&test, &[&run]);
    w.module("m", Some("m"), None);
    w.module("n", Some("n"), None);
    w.import("n", "m");
    w
}

/// Several small projects: a long function with little around it, a project
/// of types only, one where every function is called.
fn varied() -> Vec<World> {
    let mut called = World::default();
    let a = called.func("m", "A", "m/a.ucm");
    let b = called.func("m", "b", "m/a.ucm");
    called.private(&b);
    called.edge(EdgeKind::Calls, &a, &b);
    let mut types = World::default();
    types.symbol("m", "One", SymbolKind::Type, "m/a.ucm");
    let two = types.symbol("m", "two", SymbolKind::Type, "m/a.ucm");
    types.private(&two);
    types.module("m", None, None);
    vec![crowded(), called, types]
}

fn probe(decision: &lighthouse_spec::Decision, variable: &str, in_file: bool) -> String {
    let message = if in_file {
        format!("{{{{ {{'s': {variable}, 'f': file}} }}}}")
    } else {
        format!("{{{{ {{'s': {variable}}} }}}}")
    };
    let scope = serde_json::to_string(&decision.scope).unwrap();
    // The facts of a subject are built for the fields a check reads; indexing
    // by a computed key (never evaluated) asks for all of them.
    let all = if in_file {
        format!("true || {variable}[string(1)] == null || file[string(1)] == null")
    } else {
        format!("true || {variable}[string(1)] == null")
    };
    format!(
        "apiVersion: lighthouse/v1alpha1\\nkind: Decision\\nmetadata:\\n  name: local/probe\\nspec:\\n  title: Probe\\n  context: A probe.\\n  scope: {scope}\\n  requirement: A probe MUST hold.\\n  severity: warn\\n  check:\\n    type: cel\\n    select: {select}\\n    where: {all}\\n    message: {message}\\n  examples:\\n    - name: bad\\n      language: text\\n      kind: invalid\\n      files: [{{ path: a.txt, body: x }}]\\n      expect: [{{ line: 1 }}]\\n    - name: good\\n      language: text\\n      kind: valid\\n      files: [{{ path: a.txt, body: x }}]\\n",
        select = "SELECT",
        message = serde_json::to_string(&message).unwrap(),
        all = serde_json::to_string(&all).unwrap(),
    )
}

type Key = (String, u32);

fn count(found: impl IntoIterator<Item = Key>) -> BTreeMap<Key, usize> {
    let mut by_place = BTreeMap::new();
    for key in found {
        *by_place.entry(key).or_insert(0) += 1;
    }
    by_place
}

#[test]
fn the_query_holds_of_the_subjects_the_decision_reports() {
    let comp = lighthouse_test_support::composition();
    let bundled = lighthouse_test_support::bundled();
    let worlds = varied();
    let (mut compared, mut skipped, mut violating) = (Vec::new(), Vec::new(), 0usize);
    for decision in bundled.decisions() {
        let Some(CheckKind::Cel(check)) = decision.check.as_ref().map(|c| &c.kind) else {
            continue;
        };
        let Some(select) = check.selects(decision.scope.subject) else {
            continue;
        };
        let id = decision.id().to_owned();
        let Ok(ucq) = lighthouse_logic::translate(comp.cel(), check, select.variable()) else {
            continue;
        };
        if check.at.is_some() || !decision.applies_to("go") {
            skipped.push(id);
            continue;
        }
        let options = decision.resolve_options(&Map::new(), Some("go")).unwrap();
        let options = Value::Object(options);
        let text = probe(
            decision,
            select.variable(),
            select.scope() == RunScope::File,
        )
        .replace("SELECT", select.name());
        let probe_catalog = Catalog::from_local(
            BTreeMap::from([("probe.yaml".to_owned(), text.replace("\\n", "\n"))]),
            comp.cel(),
        )
        .unwrap_or_else(|e| panic!("{id}: the probe: {e}"));
        let probe_plugin = Checks::new(&comp, "local", &probe_catalog).unwrap();
        let pack = decision.pack().to_owned();
        let shipped_plugin = Checks::new(&comp, &pack, bundled).unwrap();
        let mut saw = 0;
        for world in &worlds {
            let Ok(subjects) = world.try_diagnose_with(
                &probe_catalog,
                &Subject {
                    plugin: &probe_plugin,
                    id: "local",
                },
                "local/probe",
                json!({}),
            ) else {
                continue;
            };
            let Ok(reported) = world.try_diagnose_with(
                bundled,
                &Subject {
                    plugin: &shipped_plugin,
                    id: &pack,
                },
                &id,
                json!({}),
            ) else {
                continue;
            };
            let place = |d: &lighthouse_model::Diagnostic| {
                (d.file.to_string_lossy().into_owned(), d.span.start.line)
            };
            let expected = count(reported.iter().map(place));
            let found = count(subjects.iter().filter_map(|d| {
                let both: Value = serde_json::from_str(&d.message).ok()?;
                let file = both.get("f").unwrap_or(&Value::Null);
                ucq.holds(&both["s"], file, &options).then(|| place(d))
            }));
            saw += subjects.len();
            violating += found.values().sum::<usize>();
            assert_eq!(
                found, expected,
                "{id}: the query and the decision differ on the subjects of a project\n{ucq}"
            );
        }
        if saw > 0 {
            compared.push(id);
        } else {
            skipped.push(id);
        }
    }
    eprintln!("compared {}: {compared:?}", compared.len());
    eprintln!("skipped {}: {skipped:?}", skipped.len());
    assert!(
        compared.len() >= 5 && violating > 0,
        "{compared:?} {violating}"
    );
}
