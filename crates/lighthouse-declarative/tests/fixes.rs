//! Fixes compiled from decisions: the generic operations evaluated over a
//! finding, and commands run on a scratch copy.

use std::{collections::BTreeMap, path::PathBuf};

use lighthouse_declarative::Declarative;
use lighthouse_model::{
    Anchor, Diagnostic, EditOp, File, Fingerprint, FixOutcome, Fragment, Node, Options, Owner,
    Position, Project, Severity, Span, Symbol, SymbolId, SymbolKind, Visibility,
};
use lighthouse_plugin::{
    Error, FixDecision, FixRequest, Fixer, KeyCtx, OrderKey, OrderKeyManifest, OrderKeys, Plugin,
    Workspace,
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

/// A decision `id` (labels for `pack`) with `fix` and `check`, as a document.
fn document(id: &str, pack: &str, section: &str, fix: &str, check: &str) -> String {
    let indented: String = fix.lines().map(|l| format!("    {l}\n")).collect();
    format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: {id}\n  labels:\n    lighthouse/pack: {pack}\n    lighthouse/section: {section}\nspec:\n  title: Probe\n  intent: A probe.\n  scope: {{ subject: file }}\n  requirement: A probe MUST hold.\n  enforcement: mechanical\n  evidence: [x]\n{check}  fix:\n{indented}{EXAMPLES}"
    )
}

const CEL: &str = "  check:\n    type: cel\n    select: file\n    where: 'true'\n    message: m\n";

/// A decision `local/probe` whose fix is `fix`.
fn local_layer(fix: &str) -> Result<Catalog, lighthouse_spec::Error> {
    let text = document("local/probe", "local", "rules", fix, CEL);
    Catalog::from_local(BTreeMap::from([("probe.yaml".to_owned(), text)]))
}

fn fixer(fix: &str, pack: &str) -> Box<dyn Fixer> {
    let layer = local_layer(fix).unwrap();
    let plugin = Declarative::from_catalog(pack, &layer).unwrap();
    plugin.fixers().remove(0)
}

fn local(fix: &str) -> Box<dyn Fixer> {
    fixer(fix, "local")
}

/// The same fix as the decision of a bundled-style pack `demo`.
fn bundled(fix: &str) -> Box<dyn Fixer> {
    let builtin = "  check:\n    type: builtin\n    id: demo/probe\n";
    let decision = document("demo/probe", "demo", "s", fix, builtin);
    let pack = "apiVersion: lighthouse/v1alpha1\nkind: Pack\nmetadata:\n  name: demo\nspec:\n  title: Demo\n  intro: x\n  sections:\n    - name: s\n      title: S\n      intro: x\n      decisions: [probe]\n";
    let files = [
        ("demo/pack.yaml", pack),
        ("demo/s/probe.yaml", decision.as_str()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let catalog = Catalog::from_files(files).unwrap();
    Declarative::from_catalog("demo", &catalog)
        .unwrap()
        .fixers()
        .remove(0)
}

fn finding(symbol: Option<&str>, evidence: Value) -> Diagnostic {
    let mut d = Diagnostic::new(
        "local/probe",
        Severity::Error,
        "m",
        FILE,
        span((2, 3), (2, 8)),
        Fingerprint::of("local/probe", FILE, ""),
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

    fn rank(&self, _: &KeyCtx, symbol: &Symbol) -> Result<Option<u64>, Error> {
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
        intent: String::new(),
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
        ws: &ws,
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
    let fix = "safety: suggested\ntype: ops\nops:\n  - op: move\n    node: finding.evidence.callee\n    after: finding.evidence.caller";
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
    assert_eq!(safety, lighthouse_model::Safety::Suggested);
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
    let node_fix = "safety: suggested\ntype: ops\nops: [{ op: delete, node: finding.symbol }]";
    let range_fix = "safety: suggested\ntype: ops\nops: [{ op: delete, file: finding.file, span: finding.span }]";

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
    let rename = "safety: suggested\nrequires: [complete-references]\ntype: ops\nops: [{ op: rename, symbol: finding.symbol, name: '{{ symbol.name }}New{{ 1 + 1 }}' }]";
    let replace = "safety: suggested\ntype: ops\nops: [{ op: replace, file: finding.file, span: finding.span, text: '// {{ finding.rule }}: {{ symbol.kind }}' }]";

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
    let fix = "safety: suggested\ntype: ops\nops: [{ op: delete, node: finding.evidence.missing }]";

    let error = evaluate(fix, &finding(None, json!({}))).unwrap_err();

    assert!(error.to_string().contains("local/probe: fix:"), "{error}");
}

#[test]
fn a_node_that_is_not_a_symbol_id_is_an_error() {
    let fix = "safety: suggested\ntype: ops\nops: [{ op: delete, node: \"'nope'\" }]";

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

fn try_command(
    extra: &str,
    argv: &str,
    output: &str,
    text: &str,
    finding: &Diagnostic,
) -> Result<FixOutcome, Error> {
    let fix = format!(
        "safety: suggested\ntype: command\nargv: {argv}\noutput: {output}\ntimeout: 5s\n{extra}"
    );
    run(
        local(&fix).as_ref(),
        &project(&["a"]),
        finding,
        text,
        &keys(&[]),
        true,
    )
}

fn command(argv: &str, output: &str, text: &str, finding: &Diagnostic) -> FixOutcome {
    try_command("", argv, output, text, finding).unwrap()
}

const SHOUT: &str = r#"["sh", "-c", "tr a-z A-Z < \"$1\"", "sh", "{file}"]"#;

#[test]
fn a_command_that_prints_the_new_text_becomes_the_smallest_line_replacement() {
    let text = "keep\nhello\nkeep too\n";

    let got = ops(command(SHOUT, "text", text, &finding(None, json!({}))));

    assert_eq!(
        got,
        [EditOp::Replace {
            file: FILE.into(),
            span: span((1, 1), (4, 1)),
            text: "KEEP\nHELLO\nKEEP TOO\n".to_owned()
        }]
    );
}

#[test]
fn a_command_that_edits_the_scratch_copy_is_diffed_against_the_original() {
    let edit = r#"["sh", "-c", "printf 'x\n' >> \"$1\"", "sh", "{file}"]"#;
    let text = "one\ntwo\n";

    let got = ops(command(edit, "in-place", text, &finding(None, json!({}))));

    assert_eq!(
        got,
        [EditOp::Replace {
            file: FILE.into(),
            span: span((3, 1), (3, 1)),
            text: "x\n".to_owned()
        }]
    );
}

#[test]
fn a_command_that_changes_nothing_or_prints_nothing_is_declined() {
    let touch = r#"["sh", "-c", "true"]"#;

    let same = reason(command(
        touch,
        "in-place",
        "same\n",
        &finding(None, json!({})),
    ));
    let empty = reason(command(touch, "text", "same\n", &finding(None, json!({}))));

    assert!(same.contains("changed nothing"), "{same}");
    assert!(empty.contains("printed no text"), "{empty}");
}

#[test]
fn stdin_is_nothing_by_default_and_the_file_when_asked_for() {
    let argv = r#"["sh", "-c", "tr a-z A-Z"]"#;
    let at = finding(None, json!({}));

    let none = reason(command(argv, "text", "hello\n", &at));
    let file = ops(try_command("stdin: file", argv, "text", "hello\nworld\n", &at).unwrap());

    assert!(none.contains("printed no text"), "{none}");
    let EditOp::Replace { text, .. } = &file[0] else {
        panic!("replace expected");
    };
    assert_eq!(text, "HELLO\nWORLD\n");
}

#[test]
fn the_context_is_in_the_environment_and_nothing_else_is() {
    let argv = r#"["sh", "-c", "env | sort"]"#;
    let at = finding(Some("a"), json!({ "word": "hi" }));
    let secret = "LIGHTHOUSE_TEST_SECRET";
    // SAFETY: this test is the only one that touches this variable.
    unsafe { std::env::set_var(secret, "leaked") };

    let got = ops(try_command("env: { KEPT: yes }", argv, "text", "x", &at).unwrap());

    let EditOp::Replace { text, .. } = &got[0] else {
        panic!("replace expected");
    };
    let names: Vec<&str> = text.lines().filter_map(|l| l.split('=').next()).collect();
    for wanted in [
        "PATH",
        "KEPT",
        "LIGHTHOUSE_DECISION",
        "LIGHTHOUSE_OPTIONS",
        "LIGHTHOUSE_API_VERSION",
    ] {
        assert!(names.contains(&wanted), "{wanted} in {names:?}");
    }
    assert!(
        !text.contains("leaked"),
        "the environment was not cleared:\n{text}"
    );
    let decision = text
        .lines()
        .find_map(|l| l.strip_prefix("LIGHTHOUSE_DECISION="))
        .unwrap();
    let decision: Value = serde_json::from_str(decision).unwrap();
    assert_eq!(decision["rule"], "local/probe");
    assert_eq!(decision["evidence"]["word"], "hi");
    assert!(text.contains("LIGHTHOUSE_API_VERSION=1"));
}

#[test]
fn placeholders_are_replaced_in_every_argument() {
    let argv = r#"["sh", "-c", "printf '%s|%s|%s' \"$1\" \"$2\" \"$3\" > \"$4\"", "sh", "{rule}", "{line}", "{symbol}", "{file}"]"#;

    let got = ops(command(
        argv,
        "in-place",
        "",
        &finding(Some("a"), json!({})),
    ));

    let EditOp::Replace { text, .. } = &got[0] else {
        panic!("replace expected");
    };
    assert_eq!(text, &format!("local/probe|2|{}", id("a").as_str()));
}

#[test]
fn exit_one_declines_with_the_reason_from_stderr() {
    let why = reason(command(
        r#"["sh", "-c", "echo 'not for me' >&2; exit 1"]"#,
        "text",
        "",
        &finding(None, json!({})),
    ));

    assert!(why.contains("declined: not for me"), "{why}");
}

#[test]
fn exit_two_a_signal_and_a_timeout_are_errors_that_apply_nothing() {
    let at = finding(None, json!({}));
    let two = try_command(
        "",
        r#"["sh", "-c", "echo broken >&2; exit 2"]"#,
        "text",
        "",
        &at,
    )
    .unwrap_err()
    .to_string();
    let killed = try_command("", r#"["sh", "-c", "kill -9 $$"]"#, "text", "", &at)
        .unwrap_err()
        .to_string();
    let slow = "safety: suggested\ntype: command\nargv: [\"sleep\", \"5\"]\ntimeout: 1s";
    let started = std::time::Instant::now();
    let timed_out = run(
        local(slow).as_ref(),
        &project(&["a"]),
        &at,
        "",
        &keys(&[]),
        true,
    )
    .unwrap_err()
    .to_string();

    assert!(two.contains("exited 2") && two.contains("broken"), "{two}");
    assert!(killed.contains("signal"), "{killed}");
    assert!(timed_out.contains("timed out"), "{timed_out}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn a_program_that_cannot_start_is_an_error() {
    let error = try_command(
        "",
        r#"["lighthouse-no-such-program"]"#,
        "text",
        "",
        &finding(None, json!({})),
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("cannot start"), "{error}");
}

#[test]
fn a_command_runs_only_in_a_project_the_user_trusts_whoever_wrote_it() {
    let fix = format!("safety: suggested\ntype: command\nargv: {SHOUT}\noutput: text");
    let project = project(&["a"]);
    let at = finding(None, json!({}));

    for fixer in [bundled(&fix), local(&fix)] {
        let denied = run(fixer.as_ref(), &project, &at, "hello\n", &keys(&[]), false).unwrap();
        let allowed = run(fixer.as_ref(), &project, &at, "hello\n", &keys(&[]), true).unwrap();

        let why = reason(denied);
        assert!(why.contains("not trusted"), "{why}");
        assert!(why.contains("lighthouse trust"), "{why}");
        assert!(why.contains("tr a-z A-Z"), "names the command: {why}");
        assert!(matches!(allowed, FixOutcome::Proposed { .. }));
    }
}

#[test]
fn a_placeholder_value_that_starts_with_a_dash_is_refused() {
    let argv = r#"["sh", "-c", "true", "sh", "{symbol}"]"#;
    let at = finding(Some("a"), json!({}));
    let mut dashed = at.clone();
    dashed.symbol = Some("-rf".to_owned());

    let fine = command(argv, "in-place", "x\n", &at);
    let why = reason(command(argv, "in-place", "x\n", &dashed));

    assert!(reason(fine).contains("changed nothing"));
    assert!(why.contains("starts with `-`"), "{why}");
}

#[test]
fn a_command_prints_at_most_a_megabyte_that_is_kept() {
    let flood = r#"["sh", "-c", "yes | head -c 5000000"]"#;

    let got = ops(command(flood, "text", "x", &finding(None, json!({}))));

    let EditOp::Replace { text, .. } = &got[0] else {
        panic!("replace expected");
    };
    assert!(text.len() <= 1 << 20, "{}", text.len());
}

#[test]
fn the_fixer_carries_the_capabilities_the_fix_requires() {
    let fix = "safety: safe\nrequires: [extent, reference-sites]\ntype: ops\nops: [{ op: delete, node: finding.symbol }]";

    let fixer = local(fix);

    assert_eq!(
        fixer.manifest().requires,
        [
            lighthouse_model::Capability::Extent,
            lighthouse_model::Capability::ReferenceSites
        ]
    );
    assert_eq!(fixer.manifest().id, "local/probe");
}

#[test]
fn only_decisions_with_a_fix_have_fixers() {
    let plugin = Declarative::from_catalog("design", Catalog::bundled()).unwrap();

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
    let ids: Vec<String> = Declarative::bundled_fixers("testing")
        .iter()
        .map(|f| f.manifest().id.clone())
        .collect();

    assert_eq!(ids, ["testing/test-file-layout"]);
}

#[test]
fn a_rename_must_declare_that_it_needs_complete_references() {
    let fix =
        "safety: suggested\ntype: ops\nops: [{ op: rename, symbol: finding.symbol, name: x }]";

    let error = local_layer(fix).unwrap_err();

    assert!(error.to_string().contains("complete-references"), "{error}");
}

#[test]
fn a_command_argument_may_not_embed_a_placeholder_or_use_root() {
    let build =
        |argv: &str| local_layer(&format!("safety: suggested\ntype: command\nargv: {argv}"));

    for bad in [r#"["tool", "--out={file}"]"#, r#"["tool", "{root}"]"#] {
        let error = build(bad).unwrap_err();
        assert!(error.to_string().contains("placeholder"), "{bad}: {error}");
    }
    assert!(build(r#"["tool", "{file}", "--flag"]"#).is_ok());
    assert!(build(r#"["sh", "-c", "echo '{}' > \"$1\"", "sh", "{file}"]"#).is_ok());
}
