//! The `fix:` block of a decision: what a catalog accepts and what it refuses.

use lighthouse_model::{Safety, Severity, ucm::Capability};
use lighthouse_spec::{
    Catalog, CommandSpec, DecisionSpec, Fix, FixKind, FixScope, OpSpec, OrderScope, Point,
};
use lighthouse_test_support::catalog::*;

fn rejected(files: Files, needle: &str) {
    let err = Catalog::from_files(files).unwrap_err();
    assert!(
        err.to_string().contains(needle),
        "{err} should mention {needle}"
    );
}

const CHECKED: &str = "  check:\n    type: builtin\n    id: p/a\n";
const EXAMPLES: &str = "  examples:
    - name: bad
      language: go
      kind: invalid
      files: [{ path: a.go, body: x }]
      fixed: [{ path: a.go, body: y }]
    - name: good
      language: go
      kind: valid
      files: [{ path: a.go, body: y }]
";

fn fixable(fix: &str) -> Files {
    decision_with(&format!("{CHECKED}{fix}{EXAMPLES}"))
}

const MOVE: &str = "  fix:
    safety: safe
    requires: [extent, complete-references]
    type: ops
    ops:
      - op: move
        node: finding.evidence.callee
        after: finding.evidence.caller
      - op: reorder
        scope: owner
        by: [design/group]
        when: finding.symbol != ''
      - op: delete
        file: finding.file
        span: finding.span
      - op: rename
        symbol: finding.symbol
        name: '{{ symbol.name }}Value'
      - op: replace
        file: finding.file
        span: finding.span
        text: '// {{ finding.decision }}'
";

#[test]
fn a_fix_of_every_operation_loads_and_is_readable() {
    let catalog = Catalog::from_files(fixable(MOVE)).unwrap();

    let fix: &Fix = catalog.decision("p/a").unwrap().fix.as_ref().unwrap();

    assert_eq!(fix.safety, Safety::Safe);
    assert_eq!(
        fix.requires,
        [Capability::Extent, Capability::CompleteReferences]
    );
    let FixKind::Ops { ops } = &fix.kind else {
        panic!("ops expected");
    };
    let names: Vec<_> = ops.iter().map(OpSpec::name).collect();
    assert_eq!(names, ["move", "reorder", "delete", "rename", "replace"]);
    assert!(matches!(
        &ops[1],
        OpSpec::Reorder { scope: OrderScope::Owner, by, .. } if by == &["design/group"]
    ));
}

#[test]
fn a_fix_names_its_type_and_the_type_decides_its_fields() {
    let untyped = "  fix:\n    safety: unsafe\n    ops: [{ op: delete, node: x }]\n";
    rejected(fixable(untyped), "type");
    let none = "  fix:\n    safety: unsafe\n    type: ops\n";
    rejected(fixable(none), "ops");
    let stray = "  fix:\n    safety: unsafe\n    type: ops\n    ops: [{ op: delete, node: x }]\n    argv: [x]\n";
    rejected(fixable(stray), "argv");
    let unknown = "  fix:\n    safety: unsafe\n    type: wasm\n";
    rejected(fixable(unknown), "wasm");
}

#[test]
fn the_rpc_type_is_kept_without_failing_the_pack() {
    let rpc = "  fix:\n    safety: unsafe\n    type: rpc\n    params: { method: fix }\n";

    let catalog = Catalog::from_files(fixable(rpc)).unwrap();

    let fix = catalog.decision("p/a").unwrap().fix.as_ref().unwrap();
    assert!(matches!(fix.kind, FixKind::Rpc { .. }));
}

#[test]
fn an_unknown_operation_or_parameter_is_refused() {
    let op = "  fix:\n    safety: unsafe\n    type: ops\n    ops: [{ op: shuffle }]\n";
    rejected(fixable(op), "shuffle");
    let param = "  fix:\n    safety: unsafe\n    type: ops\n    ops: [{ op: delete, node: x, colour: red }]\n";
    rejected(fixable(param), "colour");
}

fn ops(list: &str) -> String {
    format!("  fix:\n    safety: unsafe\n    type: ops\n    ops: {list}\n")
}

#[test]
fn operation_parameters_are_checked() {
    rejected(
        fixable(&ops("[{ op: move, node: x, before: y, after: z }]")),
        "exactly one of `before` or `after`",
    );
    rejected(
        fixable(&ops("[{ op: move, node: x }]")),
        "exactly one of `before` or `after`",
    );
    rejected(
        fixable(&ops("[{ op: delete, file: x }]")),
        "delete needs `node`, or `file` and `span`",
    );
    rejected(
        fixable(&ops("[{ op: reorder, scope: file, by: [group] }]")),
        "qualified `plugin/name`",
    );
    rejected(
        fixable(&ops("[{ op: reorder, scope: world, by: [a/b] }]")),
        "world",
    );
}

#[test]
fn expressions_must_compile() {
    rejected(
        fixable(&ops("[{ op: delete, node: 'finding..x(' }]")),
        "delete `node`",
    );
    rejected(
        fixable(&ops("[{ op: delete, node: x, when: '1 +' }]")),
        "`when`",
    );
    rejected(
        fixable(&ops(
            "[{ op: replace, file: x, span: y, text: '{{ 1 + }}' }]",
        )),
        "replace `text`",
    );
    rejected(
        fixable(&ops("[{ op: replace, file: x, span: y, text: '{{ x' }]")),
        "never closed",
    );
}

#[test]
fn safe_is_reserved_for_error_decisions() {
    let heuristic =
        fixable("  fix:\n    safety: safe\n    type: ops\n    ops: [{ op: delete, node: x }]\n")
            .into_iter()
            .map(|(k, v)| (k, v.replace("severity: error", "severity: warn")))
            .collect();
    rejected(heuristic, "reserved for decisions that author `error`");
}

#[test]
fn a_fix_belongs_to_a_checked_decision_with_a_fixed_example() {
    let fix = "  fix:\n    safety: safe\n    type: ops\n    ops: [{ op: delete, node: x }]\n";
    rejected(
        decision_with(&format!("{fix}{EXAMPLES}")),
        "no findings to fix",
    );
    let unfixed = EXAMPLES.replace("      fixed: [{ path: a.go, body: y }]\n", "");
    rejected(
        decision_with(&format!("{CHECKED}{fix}{unfixed}")),
        "needs an invalid example with `fixed`",
    );
}

#[test]
fn fixed_examples_must_name_files_of_an_invalid_example() {
    let fix = "  fix:\n    safety: safe\n    type: ops\n    ops: [{ op: delete, node: x }]\n";
    let elsewhere = EXAMPLES.replace("fixed: [{ path: a.go,", "fixed: [{ path: b.go,");
    rejected(
        decision_with(&format!("{CHECKED}{fix}{elsewhere}")),
        "not a file of the example",
    );
    let valid = EXAMPLES.replace(
        "      files: [{ path: a.go, body: y }]\n",
        "      files: [{ path: a.go, body: y }]\n      fixed: [{ path: a.go, body: y }]\n",
    );
    rejected(
        decision_with(&format!("{CHECKED}{fix}{valid}")),
        "only an invalid example has `fixed`",
    );
    rejected(
        decision_with(&format!("{CHECKED}{EXAMPLES}")),
        "`fixed` needs a `fix`",
    );
}

#[test]
fn a_command_needs_a_program_and_a_usable_timeout() {
    let command =
        |fields: &str| format!("  fix:\n    safety: unsafe\n    type: command\n    {fields}\n");
    rejected(fixable(&command("command: []")), "needs a program");
    rejected(
        fixable(&command("command: [gofmt]\n    timeout: soon")),
        "`soon` is not a duration",
    );
    rejected(
        fixable(&command("command: [gofmt]\n    timeout: '30'")),
        "`30` is not a duration",
    );
    rejected(
        fixable(&command("command: [gofmt]\n    output: text")),
        "text",
    );
    rejected(
        fixable(&command("command: [gofmt]\n    output: lines")),
        "`output`",
    );
    rejected(fixable(&command("argv: [gofmt]")), "argv");
    let ok = command("command: [gofmt, -w, '{file}']\n    output: files\n    timeout: 2m");
    let catalog = Catalog::from_files(fixable(&ok)).unwrap();
    let fix: &Fix = catalog.decision("p/a").unwrap().fix.as_ref().unwrap();
    let FixKind::Command(command) = &fix.kind else {
        panic!("command expected");
    };
    let command: &CommandSpec = command;
    assert_eq!(command.program.command.len(), 3);
    assert_eq!(
        command.program.timeout(Point::Fix),
        std::time::Duration::from_secs(120)
    );
}

#[test]
fn a_command_fix_has_the_placeholders_of_a_fix_and_a_scope_of_its_output() {
    let command =
        |fields: &str| format!("  fix:\n    safety: unsafe\n    type: command\n    {fields}\n");
    for fine in ["file", "line", "symbol", "decision"] {
        let fix = command(&format!("command: [fixer, '{{{fine}}}']"));
        assert!(Catalog::from_files(fixable(&fix)).is_ok(), "{fine}");
    }
    for bad in ["files", "model", "rule", "path"] {
        let fix = command(&format!("command: [fixer, '{{{bad}}}']"));
        rejected(fixable(&fix), &format!("`{{{bad}}}` is not a placeholder"));
    }
    let filter = command("command: [gofmt]\n    stdin: file\n    output: stdout");
    assert!(Catalog::from_files(fixable(&filter)).is_ok());
    rejected(
        fixable(&command("command: [gofmt]\n    output: stdout")),
        "needs `{file}` or `stdin: file`",
    );
    rejected(
        fixable(&command(
            "command: [gofmt]\n    stdin: file\n    output: stdout\n    scope: files",
        )),
        "scope",
    );
    let scoped = command("command: [fixer]\n    scope: files");
    let catalog = Catalog::from_files(fixable(&scoped)).unwrap();
    let FixKind::Command(spec) = &catalog.decision("p/a").unwrap().fix.as_ref().unwrap().kind
    else {
        panic!("command expected");
    };
    assert_eq!(spec.scope, Some(FixScope::Files));
}

#[test]
fn a_fix_has_a_priority_that_defaults_to_zero() {
    let plain = Catalog::from_files(fixable(MOVE)).unwrap();
    assert_eq!(
        plain
            .decision("p/a")
            .unwrap()
            .fix
            .as_ref()
            .unwrap()
            .priority,
        0
    );
    let low = MOVE.replace("safety: safe", "safety: safe\n    priority: -100");
    let catalog = Catalog::from_files(fixable(&low)).unwrap();
    assert_eq!(
        catalog
            .decision("p/a")
            .unwrap()
            .fix
            .as_ref()
            .unwrap()
            .priority,
        -100
    );
}

#[test]
fn a_fix_is_part_of_the_decision_version_but_not_of_what_it_means() {
    let unfixed = EXAMPLES.replace("      fixed: [{ path: a.go, body: y }]\n", "");
    let plain = Catalog::from_files(decision_with(&format!("{CHECKED}{unfixed}"))).unwrap();
    let with_fix = Catalog::from_files(fixable(MOVE)).unwrap();
    let (a, b) = (
        plain.decision("p/a").unwrap(),
        with_fix.decision("p/a").unwrap(),
    );

    assert_ne!(a.version(), b.version());
    assert_eq!(a.meaning_version(), b.meaning_version());
    assert_eq!(a.check_revision(), b.check_revision());
}

#[test]
fn an_operations_guard_is_readable() {
    let guarded = ops(
        "\n      - { op: delete, node: finding.symbol, when: \"finding.evidence.role == 'x'\" }\n      - { op: delete, node: finding.symbol }",
    );
    let catalog = Catalog::from_files(fixable(&guarded)).unwrap();
    let FixKind::Ops { ops } = &catalog.decision("p/a").unwrap().fix.as_ref().unwrap().kind else {
        panic!("ops expected");
    };

    assert_eq!(ops[0].when(), Some("finding.evidence.role == 'x'"));
    assert_eq!(ops[1].when(), None);
}

#[test]
fn the_effective_safety_of_a_fix_is_decided_in_one_place() {
    let load = |fix: &str| {
        Catalog::from_files(fixable(fix))
            .unwrap()
            .decision("p/a")
            .unwrap()
            .clone()
    };
    let ops_safe = load(MOVE);
    let ops_unsafe = load(&MOVE.replace("safety: safe", "safety: unsafe"));
    let model = load("  fix:\n    safety: unsafe\n    type: model\n");
    let unfixed = Catalog::from_files(decision_with(&format!(
        "{CHECKED}{}",
        EXAMPLES.replace("      fixed: [{ path: a.go, body: y }]\n", "")
    )))
    .unwrap()
    .decision("p/a")
    .unwrap()
    .clone();

    assert_eq!(ops_safe.fix_safety(None), Safety::Safe);
    assert_eq!(ops_unsafe.fix_safety(None), Safety::Unsafe);
    assert_eq!(
        model.fix_safety(None),
        Safety::Unsafe,
        "a model's is never safe"
    );
    assert_eq!(
        unfixed.fix_safety(None),
        Safety::Unsafe,
        "a model would make it"
    );
    let warned = ops_safe.map_spec(|spec| DecisionSpec {
        severity: Some(Severity::Warn),
        ..spec
    });
    assert_eq!(
        warned.fix_safety(None),
        Safety::Unsafe,
        "a safe fix of a decision that does not author `error` is unsafe"
    );
}

#[test]
fn the_safety_a_project_gives_a_rule_replaces_every_other_in_the_last_place() {
    let catalog = Catalog::from_files(fixable(MOVE)).unwrap();
    let ops = catalog.decision("p/a").unwrap().clone();
    let warned = ops.clone().map_spec(|spec| DecisionSpec {
        severity: Some(Severity::Warn),
        ..spec
    });
    let model = Catalog::from_files(fixable("  fix:\n    safety: unsafe\n    type: model\n"))
        .unwrap()
        .decision("p/a")
        .unwrap()
        .clone();

    assert_eq!(ops.fix_safety(Some(Safety::Unsafe)), Safety::Unsafe);
    assert_eq!(warned.fix_safety(Some(Safety::Safe)), Safety::Safe);
    assert_eq!(model.fix_safety(Some(Safety::Safe)), Safety::Safe);
}
