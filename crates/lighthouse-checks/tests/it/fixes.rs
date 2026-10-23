//! Fixes compiled from decisions: the edits a CEL expression returns for a
//! finding, lowered to the edit IR. A command fix is run by the fix run itself, which the engine tests.

use std::{collections::BTreeMap, path::PathBuf};

use lighthouse_checks::Checks;
use lighthouse_model::{
    Diagnostic, Fingerprint, Options, Position, Severity, Span,
    ucm::{
        Anchor, EditOp, File, FixOutcome, Fragment, Node, Owner, Project, Symbol, SymbolId,
        SymbolKind, Visibility,
    },
};
use lighthouse_plugin::{Error, FixDecision, FixRequest, Fixer, Plugin, Workspace};
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
    declared(names, true)
}

/// The same, with the extents of the declarations when `extents`.
fn declared(names: &[&str], extents: bool) -> Project {
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
                extent: extents.then_some(at),
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
            facts: Default::default(),
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
    Catalog::from_local(
        BTreeMap::from([("probe.yaml".to_owned(), text)]),
        &lighthouse_test_support::cel(),
    )
}

fn fixer(fix: &str, pack: &str) -> Box<dyn Fixer> {
    let layer = local_layer(fix).unwrap();
    let plugin = Checks::new(&lighthouse_test_support::composition(), pack, &layer).unwrap();
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

fn run(
    fixer: &dyn Fixer,
    project: &Project,
    finding: &Diagnostic,
    text: &str,
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
        false,
    )
}

#[test]
fn a_move_takes_its_node_and_anchor_from_the_finding() {
    let fix = "safety: unsafe\ntype: cel\nedits: '[move(finding.evidence.callee, after(finding.evidence.caller))]'";
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
    let fix =
        "safety: safe\ntype: cel\nedits: '[move(finding.symbol, before(finding.evidence.first))]'";
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
fn the_expression_chooses_which_edits_apply() {
    let fix = "safety: safe\ntype: cel\nedits: >-\n  finding.evidence.role == 'fixture' ? [move(finding.symbol, before(finding.evidence.at))]\n  : finding.evidence.role == 'helper' ? [move(finding.symbol, after(finding.evidence.at))]\n  : []";
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
    assert_eq!(neither, "the fix proposes no edit");
}

#[test]
fn remove_names_a_node_or_a_span() {
    let node_fix = "safety: unsafe\ntype: cel\nedits: '[remove(finding.symbol)]'";
    let range_fix = "safety: unsafe\ntype: cel\nedits: '[remove(finding.span)]'";
    let bare_fix = "safety: unsafe\ntype: cel\nedits: '[remove({\"start\": finding.span.start, \"end\": finding.span.end})]'";

    let by_node = ops(evaluate(node_fix, &finding(Some("b"), json!({}))).unwrap());
    let by_range = ops(evaluate(range_fix, &finding(None, json!({}))).unwrap());
    let by_bare = ops(evaluate(bare_fix, &finding(None, json!({}))).unwrap());

    assert_eq!(by_node, [EditOp::Delete { node: node("b") }]);
    let range = [EditOp::DeleteRange {
        file: PathBuf::from(FILE),
        span: span((2, 3), (2, 8)),
    }];
    assert_eq!(by_range, range);
    assert_eq!(by_bare, range, "a span without a file is in the finding's");
}

#[test]
fn rename_and_replace_take_their_text_from_cel() {
    let rename =
        "safety: unsafe\ntype: cel\nedits: \"[rename(finding.symbol, 'aNew' + string(1 + 1))]\"";
    let replace =
        "safety: unsafe\ntype: cel\nedits: \"[replace(finding.span, '// ' + finding.decision)]\"";

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
            text: "// local/probe".to_owned()
        }]
    );
}

#[test]
fn add_inserts_text_at_an_anchor() {
    let text = "one\ntwo\n";
    let at = |anchor: &str| {
        let fix = format!("safety: unsafe\ntype: cel\nedits: \"[add({anchor}, 'x')]\"");
        let outcome = run(
            local(&fix).as_ref(),
            &project(&["a", "b", "c"]),
            &finding(Some("b"), json!({})),
            text,
            false,
        )
        .unwrap();
        ops(outcome)
    };
    let inserted = |line: u32, col: u32| {
        let at = Position { line, col };
        [EditOp::Replace {
            file: PathBuf::from(FILE),
            span: Span { start: at, end: at },
            text: "x\n".to_owned(),
        }]
    };

    assert_eq!(at("before(finding.symbol)"), inserted(2, 1));
    assert_eq!(at("after(finding.symbol)"), inserted(3, 1));
    assert_eq!(at("start(finding.file)"), inserted(1, 1));
    assert_eq!(at("end(finding.file)"), inserted(3, 1));
}

#[test]
fn add_ends_its_text_with_the_line_ending_of_the_file() {
    let fix = "safety: unsafe\ntype: cel\nedits: \"[add(start(finding.file), 'x'), add(end(finding.file), 'y\\\\n')]\"";

    let crlf = ops(run(
        local(fix).as_ref(),
        &project(&["a"]),
        &finding(Some("a"), json!({})),
        "one\r\ntwo\r\n",
        false,
    )
    .unwrap());

    let texts: Vec<&str> = crlf
        .iter()
        .map(|op| match op {
            EditOp::Replace { text, .. } => text.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(texts, ["x\r\n", "y\n"], "terminated only when it was not");
}

#[test]
fn replacing_a_symbol_whose_provider_reports_no_extent_declines() {
    let fix = "safety: unsafe\ntype: cel\nedits: \"[replace(finding.symbol, 'x')]\"";
    let bare = declared(&["a"], false);

    let outcome = run(
        local(fix).as_ref(),
        &bare,
        &finding(Some("a"), json!({})),
        "",
        false,
    )
    .unwrap();

    assert!(reason(outcome).contains("has no extent"));
}

#[test]
fn an_expression_that_fails_is_an_error_naming_the_fix() {
    let fix = "safety: unsafe\ntype: cel\nedits: '[remove(finding.evidence.missing)]'";

    let error = evaluate(fix, &finding(None, json!({}))).unwrap_err();

    assert!(error.to_string().contains("local/probe: fix:"), "{error}");
}

#[test]
fn a_function_the_fix_never_calls_in_a_branch_no_example_takes_is_found_at_load() {
    let fix = "safety: unsafe\ntype: cel\nedits: \"finding.file == 'x' ? [] : [remove(nope(1))]\"";
    let layer = local_layer(fix).unwrap();

    let error = Checks::new(&lighthouse_test_support::composition(), "local", &layer)
        .err()
        .unwrap()
        .to_string();

    assert!(error.contains("`nope` is not a function"), "{error}");
}

#[test]
fn edits_that_are_not_a_list_are_an_error() {
    let fix = "safety: unsafe\ntype: cel\nedits: remove(finding.span)";

    let error = evaluate(fix, &finding(None, json!({}))).unwrap_err();

    assert!(error.to_string().contains("not a list"), "{error}");
}

#[test]
fn a_node_that_is_not_a_symbol_id_declines() {
    let fix = "safety: unsafe\ntype: cel\nedits: \"[remove('nope')]\"";

    let why = reason(evaluate(fix, &finding(None, json!({}))).unwrap());

    assert!(why.contains("not a symbol id"), "{why}");
}

const BY_RANK: &str = "safety: safe\ntype: cel\nedits: \"[reorder(file.symbols.filter(s, s.name in ranks).sortBy(s, ranks[s.name]))]\"";

fn reorder(table: &str, names: &[&str]) -> FixOutcome {
    let fix = BY_RANK.replace("ranks", table);
    run(
        local(&fix).as_ref(),
        &project(names),
        &finding(Some(names[0]), json!({})),
        "",
        false,
    )
    .unwrap()
}

#[test]
fn reorder_lists_the_ordered_declarations_by_rank_and_keeps_ties_in_source_order() {
    let got = ops(reorder(
        "{'a': 2, 'b': 0, 'c': 1, 'd': 0}",
        &["a", "b", "c", "d"],
    ));

    assert_eq!(
        got,
        [EditOp::Reorder {
            owner: Owner::File(FILE.into()),
            order: vec![node("b"), node("d"), node("c"), node("a")]
        }]
    );
}

#[test]
fn reorder_leaves_out_what_the_expression_leaves_out() {
    let got = ops(reorder("{'a': 1, 'c': 0}", &["a", "b", "c"]));

    assert_eq!(
        got,
        [EditOp::Reorder {
            owner: Owner::File(FILE.into()),
            order: vec![node("c"), node("a")]
        }]
    );
}

#[test]
fn reorder_proposes_no_edit_when_nothing_would_move() {
    let why = reason(reorder("{'a': 0, 'b': 1}", &["a", "b"]));

    assert_eq!(why, "the fix proposes no edit");
}

#[test]
fn the_fixer_carries_the_capabilities_the_fix_requires() {
    let fix = "safety: safe\nrequires: [extent, reference-sites]\ntype: cel\nedits: '[remove(finding.symbol)]'";

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
fn the_constructors_a_fix_calls_say_what_it_requires() {
    use lighthouse_model::ucm::Capability::{CompleteReferences, Extent};
    let requires = |edits: &str| {
        local(&format!("safety: unsafe\ntype: cel\nedits: \"{edits}\""))
            .manifest()
            .requires
            .clone()
    };

    assert_eq!(
        requires("[rename(finding.symbol, 'x')]"),
        [CompleteReferences]
    );
    assert_eq!(
        requires("[move(finding.symbol, after(finding.symbol))]"),
        [Extent]
    );
    assert_eq!(requires("[reorder([])]"), [Extent]);
    assert!(requires("[remove(finding.span)]").is_empty());
}

#[test]
fn only_decisions_with_a_fix_have_fixers() {
    let plugin = Checks::new(
        &lighthouse_test_support::composition(),
        "design",
        lighthouse_test_support::bundled(),
    )
    .unwrap();

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
    let ids: Vec<String> = Checks::new(
        &lighthouse_test_support::composition(),
        "testing",
        lighthouse_test_support::bundled(),
    )
    .unwrap()
    .fixers()
    .iter()
    .map(|f| f.manifest().id.clone())
    .collect();

    assert_eq!(ids, ["testing/file-layout"]);
}
