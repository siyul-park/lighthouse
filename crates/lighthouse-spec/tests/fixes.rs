//! The `fix:` block of a decision: what a catalog accepts and what it refuses.

mod support;

use lighthouse_model::{Capability, Safety};
use lighthouse_spec::{Catalog, Fix, FixKind, OpSpec, ReorderScope};
use support::*;

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
        text: '// {{ finding.rule }}'
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
        OpSpec::Reorder { scope: ReorderScope::Owner, by, .. } if by == &["design/group"]
    ));
}

#[test]
fn a_fix_names_its_type_and_the_type_decides_its_fields() {
    let untyped = "  fix:\n    safety: suggested\n    ops: [{ op: delete, node: x }]\n";
    rejected(fixable(untyped), "type");
    let none = "  fix:\n    safety: suggested\n    type: ops\n";
    rejected(fixable(none), "ops");
    let stray = "  fix:\n    safety: suggested\n    type: ops\n    ops: [{ op: delete, node: x }]\n    argv: [x]\n";
    rejected(fixable(stray), "argv");
    let unknown = "  fix:\n    safety: suggested\n    type: wasm\n";
    rejected(fixable(unknown), "wasm");
}

#[test]
fn the_rpc_type_is_kept_without_failing_the_pack() {
    let rpc = "  fix:\n    safety: suggested\n    type: rpc\n    params: { method: fix }\n";

    let catalog = Catalog::from_files(fixable(rpc)).unwrap();

    let fix = catalog.decision("p/a").unwrap().fix.as_ref().unwrap();
    assert!(matches!(fix.kind, FixKind::Rpc { .. }));
}

#[test]
fn an_unknown_operation_or_parameter_is_refused() {
    let op = "  fix:\n    safety: suggested\n    type: ops\n    ops: [{ op: shuffle }]\n";
    rejected(fixable(op), "shuffle");
    let param = "  fix:\n    safety: suggested\n    type: ops\n    ops: [{ op: delete, node: x, colour: red }]\n";
    rejected(fixable(param), "colour");
}

fn ops(list: &str) -> String {
    format!("  fix:\n    safety: suggested\n    type: ops\n    ops: {list}\n")
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
fn safe_is_reserved_for_mechanical_decisions() {
    let heuristic =
        fixable("  fix:\n    safety: safe\n    type: ops\n    ops: [{ op: delete, node: x }]\n")
            .into_iter()
            .map(|(k, v)| (k, v.replace("mechanical", "heuristic")))
            .collect();
    rejected(heuristic, "reserved for mechanical decisions");
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
        |fields: &str| format!("  fix:\n    safety: suggested\n    type: command\n    {fields}\n");
    rejected(fixable(&command("argv: []")), "needs a program");
    rejected(
        fixable(&command("argv: [gofmt]\n    timeout: soon")),
        "`timeout` is `soon`",
    );
    rejected(
        fixable(&command("argv: [gofmt]\n    timeout: '30'")),
        "expected a duration",
    );
    rejected(
        fixable(&command("argv: [gofmt]\n    output: inPlace")),
        "inPlace",
    );
    let ok = command("argv: [gofmt, -w, '{file}']\n    output: text\n    timeout: 2m");
    let catalog = Catalog::from_files(fixable(&ok)).unwrap();
    let fix: &Fix = catalog.decision("p/a").unwrap().fix.as_ref().unwrap();
    let FixKind::Command(command) = &fix.kind else {
        panic!("command expected");
    };
    assert_eq!(command.argv.len(), 3);
    assert_eq!(
        command.timeout_duration(),
        Some(std::time::Duration::from_secs(120))
    );
}

#[test]
fn a_fix_is_part_of_the_decision_version_but_not_of_what_it_demands() {
    let unfixed = EXAMPLES.replace("      fixed: [{ path: a.go, body: y }]\n", "");
    let plain = Catalog::from_files(decision_with(&format!("{CHECKED}{unfixed}"))).unwrap();
    let with_fix = Catalog::from_files(fixable(MOVE)).unwrap();
    let (a, b) = (
        plain.decision("p/a").unwrap(),
        with_fix.decision("p/a").unwrap(),
    );

    assert_ne!(a.version(), b.version());
    assert_eq!(a.semantic_version(), b.semantic_version());
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
fn the_fix_operations_page_documents_every_operation_and_key() {
    let page = lighthouse_spec::fix_operations_markdown(&[(
        "design/group".to_owned(),
        "the declaration group".to_owned(),
    )]);

    for op in lighthouse_spec::OPERATIONS {
        assert!(page.contains(&format!("## `{}`", op.name)), "{}", op.name);
        for param in op.params.iter().filter(|p| p.name != "when") {
            assert!(page.contains(&format!("| `{}` |", param.name)));
        }
    }
    assert!(page.contains("| `design/group` | the declaration group |"));
    assert!(page.contains("Every operation also takes `when`"));
    assert!(page.contains("`rpc` type"));
    assert!(page.contains("type: command"));
    assert!(page.starts_with("<!-- Generated by `lighthouse docs generate`"));
}
