//! Fixes compiled from decisions: the generic operations evaluated over a
//! finding. A command fix is run by the fix run itself, which the engine tests.

use std::{collections::BTreeMap, path::PathBuf};

use lighthouse_checks::Checks;
use lighthouse_model::{
    Diagnostic, Fingerprint, Options, Position, Severity, Span,
    ucm::{
        Anchor, EditOp, File, FixOutcome, Fragment, Node, Owner, Project, Symbol, SymbolId,
        SymbolKind, Visibility,
    },
};
use lighthouse_plugin::{
    Error, FixDecision, FixRequest, Fixer, KeyContext, OrderKey, OrderKeyManifest, OrderKeys,
    Plugin, Workspace,
};
use lighthouse_spec::Catalog;
use serde_json::{Value, json};

const FILE: &str = "a.src";

fn span(from: (u32, u32), to: (u32, u32)) -> Span {
    Span {
        start: Position {
            line: from.0,
            col: from.1,
        },
        end: Position {
            line: to.0,
            col: to.1,
        },
    }
}

fn id(name: &str) -> SymbolId {
    SymbolId::new("m", &[], name, SymbolKind::Function)
}

/// Functions `names`, one line each, in that order.
fn project(names: &[&str]) -> Project {
    let symbols = names
        .iter()
        .enumerate()
        .map(|(n, name)| {
            let at = span(
                (u32::try_from(n).unwrap() + 1, 1),
                (u32::try_from(n).unwrap() + 1, 10),
            );
            Symbol {
                id: id(name),
                kind: SymbolKind::Function,
                visibility: Visibility::Private,
                owner: None,
                file: FILE.into(),
                span: at,
                extent: Some(at),
                doc: None,
                name: (*name).to_owned(),
                role: None,
                optional: false,
                type_ref: None,
            }
        })
        .collect();
    Project::merge([Fragment {
        files: vec![File {
            path: FILE.into(),
            lang: "src".to_owned(),
            hash: String::new(),
            generated: false,
            test: false,
        }],
        symbols,
        ..Fragment::default()
    }])
}

const EXAMPLES: &str = "  examples:
    - name: bad
      language: text
      kind: invalid
      files: [{ path: a.txt, body: x }]
      expect: [{ line: 1 }]
      fixed: [{ path: a.txt, body: y }]
    - name: good
      language: text
      kind: valid
      files: [{ path: a.txt, body: x }]
";

/// A decision `id` with `fix` and `check`, as a document.
fn document(id: &str, fix: &str, check: &str) -> String {
    let indented: String = fix.lines().map(|l| format!("    {l}\n")).collect();
    format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: {id}\nspec:\n  title: Probe\n  context: A probe.\n  scope: {{ subject: file }}\n  requirement: A probe MUST hold.\n  severity: error\n{check}  fix:\n{indented}{EXAMPLES}"
    )
}

const CEL: &str = "  check:\n    type: cel\n    select: file\n    where: 'true'\n    message: m\n";

/// A decision `local/probe` whose fix is `fix`.
fn local_layer(fix: &str) -> Result<Catalog, lighthouse_spec::Error> {
    let text = document("local/probe", fix, CEL);
    Catalog::from_local(BTreeMap::from([("probe.yaml".to_owned(), text)]))
}

fn fixer(fix: &str, pack: &str) -> Box<dyn Fixer> {
    let layer = local_layer(fix).unwrap();
    let plugin = Checks::new(pack, &layer).unwrap();
    plugin.fixers().remove(0)
}

fn local(fix: &str) -> Box<dyn Fixer> {
    fixer(fix, "local")
}

fn finding(symbol: Option<&str>, evidence: Value) -> Diagnostic {
    let mut d = Diagnostic::new(
        "local/probe",
        Severity::Error,
        "m",
        FILE,
        span((2, 3), (2, 8)),
        Fingerprint::new("local/probe", FILE, ""),
    );
    d.symbol = symbol.map(|n| id(n).as_str().to_owned());
    d.evidence = evidence;
    d
}

struct Table(OrderKeyManifest, Vec<(&'static str, u64)>);

impl OrderKey for Table {
    fn manifest(&self) -> &OrderKeyManifest {
        &self.0
    }

    fn rank(&self, _: &KeyContext, symbol: &Symbol) -> Result<Option<u64>, Error> {
        Ok(self
            .1
            .iter()
            .find(|(name, _)| *name == symbol.name)
            .map(|(_, rank)| *rank))
    }
}

struct Keys(Table);

impl OrderKeys for Keys {
    fn key(&self, id: &str) -> Option<&dyn OrderKey> {
        (id == "t/rank").then_some(&self.0 as &dyn OrderKey)
    }
}

fn keys(table: &[(&'static str, u64)]) -> Keys {
    Keys(Table(
        OrderKeyManifest {
            id: "t/rank".to_owned(),
            description: String::new(),
        },
        table.to_vec(),
    ))
}

fn run(
    fixer: &dyn Fixer,
    project: &Project,
    finding: &Diagnostic,
    text: &str,
    keys: &Keys,
    trusted: bool,
) -> Result<FixOutcome, Error> {
    let decision = FixDecision {
        id: "local/probe".to_owned(),
        requirement: String::new(),
        context: String::new(),
    };
    let root = tempfile::tempdir().unwrap();
    let ws = Workspace::new(root.path());
    let options = Options::new();
    let facts = json!({});
    fixer.fix(&FixRequest {
        finding,
        facts: &facts,
        decision: &decision,
        options: &options,
        project,
        workspace: &ws,
        text,
        keys,
        trusted,
    })
}

fn ops(outcome: FixOutcome) -> Vec<EditOp> {
    match outcome {
        FixOutcome::Proposed { ops, .. } => ops,
        FixOutcome::Declined { reason } => panic!("declined: {reason}"),
    }
}

fn reason(outcome: FixOutcome) -> String {
    match outcome {
        FixOutcome::Declined { reason } => reason,
        FixOutcome::Proposed { .. } => panic!("proposed"),
    }
}

fn node(name: &str) -> Node {
    Node::Symbol(id(name))
}

fn evaluate(fix: &str, finding: &Diagnostic) -> Result<FixOutcome, Error> {
    run(
        local(fix).as_ref(),
        &project(&["a", "b", "c"]),
        finding,
        "",
        &keys(&[]),
        false,
    )
}

#[test]
fn a_move_takes_its_node_and_anchor_from_the_finding() {
    let fix = "safety: unsafe\ntype: ops\nops:\n  - op: move\n    node: finding.evidence.callee\n    after: finding.evidence.caller";
    let evidence = json!({ "callee": id("a").as_str(), "caller": id("c").as_str() });

    let outcome = evaluate(fix, &finding(Some("a"), evidence)).unwrap();

    let FixOutcome::Proposed {
        ops,
        safety,
        description,
    } = outcome
    else {
        panic!("declined");
    };
    assert_eq!(
        ops,
        [EditOp::Move {
            node: node("a"),
            anchor: Anchor::After(node("c"))
        }]
    );
    assert_eq!(safety, lighthouse_model::Safety::Unsafe);
    assert_eq!(description, "Probe");
}

#[test]
fn a_move_before_is_a_before_anchor() {
    let fix = "safety: safe\ntype: ops\nops:\n  - op: move\n    node: finding.symbol\n    before: finding.evidence.first";
    let evidence = json!({ "first": id("b").as_str() });

    let got = ops(evaluate(fix, &finding(Some("a"), evidence)).unwrap());

    assert_eq!(
        got,
        [EditOp::Move {
            node: node("a"),
            anchor: Anchor::Before(node("b"))
        }]
    );
}

#[test]
fn guards_choose_which_operations_apply() {
    let fix = "safety: safe\ntype: ops\nops:\n  - op: move\n    when: finding.evidence.role == 'fixture'\n    node: finding.symbol\n    before: finding.evidence.at\n  - op: move\n    when: finding.evidence.role == 'helper'\n    node: finding.symbol\n    after: finding.evidence.at";
    let at = id("c").as_str().to_owned();

    let fixture = ops(evaluate(
        fix,
        &finding(Some("a"), json!({ "role": "fixture", "at": at })),
    )
    .unwrap());
    let helper = ops(evaluate(
        fix,
        &finding(Some("a"), json!({ "role": "helper", "at": at })),
    )
    .unwrap());
    let neither =
        reason(evaluate(fix, &finding(Some("a"), json!({ "role": "x", "at": at }))).unwrap());

    assert!(matches!(
        &fixture[..],
        [EditOp::Move {
            anchor: Anchor::Before(_),
            ..
        }]
    ));
    assert!(matches!(
        &helper[..],
        [EditOp::Move {
            anchor: Anchor::After(_),
            ..
        }]
    ));
    assert!(
        neither.contains("no operation of the fix applies"),
        "{neither}"
    );
}

#[test]
fn delete_names_a_node_or_a_range() {
    let node_fix = "safety: unsafe\ntype: ops\nops: [{ op: delete, node: finding.symbol }]";
    let range_fix =
        "safety: unsafe\ntype: ops\nops: [{ op: delete, file: finding.file, span: finding.span }]";

    let by_node = ops(evaluate(node_fix, &finding(Some("b"), json!({}))).unwrap());
    let by_range = ops(evaluate(range_fix, &finding(None, json!({}))).unwrap());

    assert_eq!(by_node, [EditOp::Delete { node: node("b") }]);
    assert_eq!(
        by_range,
        [EditOp::DeleteRange {
            file: PathBuf::from(FILE),
            span: span((2, 3), (2, 8))
        }]
    );
}

#[test]
fn rename_and_replace_fill_their_templates() {
    let rename = "safety: unsafe\nrequires: [complete-references]\ntype: ops\nops: [{ op: rename, symbol: finding.symbol, name: '{{ symbol.name }}New{{ 1 + 1 }}' }]";
    let replace = "safety: unsafe\ntype: ops\nops: [{ op: replace, file: finding.file, span: finding.span, text: '// {{ finding.decision }}: {{ symbol.kind }}' }]";

    let renamed = ops(evaluate(rename, &finding(Some("a"), json!({}))).unwrap());
    let replaced = ops(evaluate(replace, &finding(Some("a"), json!({}))).unwrap());

    assert_eq!(
        renamed,
        [EditOp::Rename {
            symbol: id("a"),
            name: "aNew2".to_owned()
        }]
    );
    assert_eq!(
        replaced,
        [EditOp::Replace {
            file: PathBuf::from(FILE),
            span: span((2, 3), (2, 8)),
            text: "// local/probe: function".to_owned()
        }]
    );
}

#[test]
fn an_expression_that_fails_is_an_error_naming_the_fix() {
    let fix = "safety: unsafe\ntype: ops\nops: [{ op: delete, node: finding.evidence.missing }]";

    let error = evaluate(fix, &finding(None, json!({}))).unwrap_err();

    assert!(error.to_string().contains("local/probe: fix:"), "{error}");
}

#[test]
fn a_node_that_is_not_a_symbol_id_is_an_error() {
    let fix = "safety: unsafe\ntype: ops\nops: [{ op: delete, node: \"'nope'\" }]";

    let error = evaluate(fix, &finding(None, json!({}))).unwrap_err();

    assert!(error.to_string().contains("not a symbol id"), "{error}");
}

fn reorder(table: &[(&'static str, u64)], names: &[&str]) -> FixOutcome {
    let fix = "safety: safe\ntype: ops\nops: [{ op: reorder, scope: file, by: [t/rank] }]";
    run(
        local(fix).as_ref(),
        &project(names),
        &finding(Some(names[0]), json!({})),
        "",
        &keys(table),
        false,
    )
    .unwrap()
}

#[test]
fn reorder_lists_the_ordered_declarations_by_rank_and_keeps_ties_in_source_order() {
    let table = [("a", 2), ("b", 0), ("c", 1), ("d", 0)];

    let got = ops(reorder(&table, &["a", "b", "c", "d"]));

    assert_eq!(
        got,
        [EditOp::Reorder {
            owner: Owner::File(FILE.into()),
            order: vec![node("b"), node("d"), node("c"), node("a")]
        }]
    );
}

#[test]
fn reorder_leaves_out_what_the_key_does_not_order() {
    let table = [("a", 1), ("c", 0)];

    let got = ops(reorder(&table, &["a", "b", "c"]));

    assert_eq!(
        got,
        [EditOp::Reorder {
            owner: Owner::File(FILE.into()),
            order: vec![node("c"), node("a")]
        }]
    );
}

#[test]
fn reorder_declines_when_nothing_would_move() {
    let table = [("a", 0), ("b", 1)];

    let why = reason(reorder(&table, &["a", "b"]));

    assert!(why.contains("already in the order"), "{why}");
}

#[test]
fn reorder_declines_with_fewer_than_two_ordered_declarations() {
    let why = reason(reorder(&[("a", 0)], &["a", "b"]));

    assert!(why.contains("fewer than two"), "{why}");
}

#[test]
fn reorder_by_an_unregistered_key_is_an_error() {
    let fix = "safety: safe\ntype: ops\nops: [{ op: reorder, scope: file, by: [t/missing] }]";

    let error = run(
        local(fix).as_ref(),
        &project(&["a", "b"]),
        &finding(Some("a"), json!({})),
        "",
        &keys(&[]),
        false,
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("unknown order key `t/missing`"),
        "{error}"
    );
}

#[test]
fn the_fixer_carries_the_capabilities_the_fix_requires() {
    let fix = "safety: safe\nrequires: [extent, reference-sites]\ntype: ops\nops: [{ op: delete, node: finding.symbol }]";

    let fixer = local(fix);

    assert_eq!(
        fixer.manifest().requires,
        [
            lighthouse_model::ucm::Capability::Extent,
            lighthouse_model::ucm::Capability::ReferenceSites
        ]
    );
    assert_eq!(fixer.manifest().id, "local/probe");
}

#[test]
fn only_decisions_with_a_fix_have_fixers() {
    let plugin = Checks::new("design", Catalog::bundled()).unwrap();

    let ids: Vec<String> = plugin
        .fixers()
        .iter()
        .map(|f| f.manifest().id.clone())
        .collect();

    assert!(ids.contains(&"design/declaration-groups".to_owned()));
    assert!(!ids.contains(&"design/exported-doc".to_owned()));
}

#[test]
fn the_bundled_fixers_of_a_pack_are_those_of_its_decisions_with_a_fix() {
    let ids: Vec<String> = Checks::new("testing", Catalog::bundled())
        .unwrap()
        .fixers()
        .iter()
        .map(|f| f.manifest().id.clone())
        .collect();

    assert_eq!(ids, ["testing/file-layout"]);
}

#[test]
fn a_rename_must_declare_that_it_needs_complete_references() {
    let fix = "safety: unsafe\ntype: ops\nops: [{ op: rename, symbol: finding.symbol, name: x }]";

    let error = local_layer(fix).unwrap_err();

    assert!(error.to_string().contains("complete-references"), "{error}");
}
