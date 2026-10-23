//! The `fix:` block of a decision: what a catalog accepts and what it refuses.

use lighthouse_model::{Safety, Severity, ucm::Capability};
use lighthouse_spec::{Catalog, CommandSpec, DecisionSpec, Fix, FixKind, FixScope, Point};
use lighthouse_test_support::catalog::*;

fn rejected(files: Files, needle: &str) {
    let err = Catalog::from_files(files, &lighthouse_test_support::cel()).unwrap_err();
    assert!(
        err.to_string().contains(needle),
        "{err} should mention {needle}"
    );
}

const CHECKED: &str =
    "  check:\n    type: cel\n    select: file\n    where: 'true'\n    message: m\n";
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
    requires: [complete-references]
    type: cel
    edits: >-
      [move(finding.evidence.callee, after(finding.evidence.caller)),
       reorder(file.symbols), remove(finding.span),
       rename(finding.symbol, symbol.name + 'Value'),
       replace(finding.span, '// ' + finding.decision)]
";

#[test]
fn a_cel_fix_loads_and_its_requirements_follow_from_its_constructors() {
    let catalog = Catalog::from_files(fixable(MOVE), &lighthouse_test_support::cel()).unwrap();

    let fix: &Fix = catalog.decision("p/a").unwrap().fix.as_ref().unwrap();

    assert_eq!(fix.safety, Safety::Safe);
    assert_eq!(
        fix.needs(&lighthouse_test_support::cel()),
        [Capability::CompleteReferences, Capability::Extent]
    );
    assert!(matches!(&fix.kind, FixKind::Cel { edits, .. } if edits.contains("move(")));
}

#[test]
fn a_cel_fix_may_declare_how_far_its_edits_go() {
    let fix = "  fix:\n    safety: unsafe\n    type: cel\n    edits: '[remove(finding.span)]'\n    scope: files\n";
    let catalog = Catalog::from_files(fixable(fix), &lighthouse_test_support::cel()).unwrap();

    let fix = catalog.decision("p/a").unwrap().fix.as_ref().unwrap();

    assert!(matches!(
        &fix.kind,
        FixKind::Cel {
            scope: Some(FixScope::Files),
            ..
        }
    ));
}

#[test]
fn a_removal_of_a_span_needs_no_extent() {
    let fix = "  fix:\n    safety: unsafe\n    type: cel\n    edits: '[remove(finding.span)]'\n";
    let catalog = Catalog::from_files(fixable(fix), &lighthouse_test_support::cel()).unwrap();

    let fix = catalog.decision("p/a").unwrap().fix.as_ref().unwrap();

    assert!(fix.needs(&lighthouse_test_support::cel()).is_empty());
}

#[test]
fn removing_or_replacing_a_declaration_needs_its_extent() {
    for edits in ["[remove(finding.symbol)]", "[replace(node, 'x')]"] {
        let fix = format!("  fix:\n    safety: unsafe\n    type: cel\n    edits: \"{edits}\"\n");
        let catalog = Catalog::from_files(fixable(&fix), &lighthouse_test_support::cel()).unwrap();

        let fix = catalog.decision("p/a").unwrap().fix.as_ref().unwrap();

        assert_eq!(
            fix.needs(&lighthouse_test_support::cel()),
            [Capability::Extent],
            "{edits}"
        );
    }
}

#[test]
fn a_fix_names_its_type_and_the_type_decides_its_fields() {
    let untyped = "  fix:\n    safety: unsafe\n    edits: '[]'\n";
    rejected(fixable(untyped), "type");
    let none = "  fix:\n    safety: unsafe\n    type: cel\n";
    rejected(fixable(none), "edits");
    let stray = "  fix:\n    safety: unsafe\n    type: cel\n    edits: '[]'\n    argv: [x]\n";
    rejected(fixable(stray), "argv");
    let unknown = "  fix:\n    safety: unsafe\n    type: wasm\n";
    rejected(fixable(unknown), "wasm");
    let ops = "  fix:\n    safety: unsafe\n    type: ops\n    ops: []\n";
    rejected(fixable(ops), "ops");
}

#[test]
fn the_rpc_type_is_kept_without_failing_the_pack() {
    let rpc = "  fix:\n    safety: unsafe\n    type: rpc\n    params: { method: fix }\n";

    let catalog = Catalog::from_files(fixable(rpc), &lighthouse_test_support::cel()).unwrap();

    let fix = catalog.decision("p/a").unwrap().fix.as_ref().unwrap();
    assert!(matches!(fix.kind, FixKind::Rpc { .. }));
}

#[test]
fn edits_must_compile() {
    let broken = "  fix:\n    safety: unsafe\n    type: cel\n    edits: 'remove(finding..x('\n";
    rejected(fixable(broken), "fix edits");
}

#[test]
fn safe_is_reserved_for_error_decisions() {
    let heuristic =
        fixable("  fix:\n    safety: safe\n    type: cel\n    edits: '[remove(finding.span)]'\n")
            .into_iter()
            .map(|(k, v)| (k, v.replace("severity: error", "severity: warn")))
            .collect();
    rejected(heuristic, "reserved for decisions that author `error`");
}

#[test]
fn a_fix_belongs_to_a_checked_decision_with_a_fixed_example() {
    let fix = "  fix:\n    safety: safe\n    type: cel\n    edits: '[remove(finding.span)]'\n";
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
    let fix = "  fix:\n    safety: safe\n    type: cel\n    edits: '[remove(finding.span)]'\n";
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
    let catalog = Catalog::from_files(fixable(&ok), &lighthouse_test_support::cel()).unwrap();
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
        assert!(
            Catalog::from_files(fixable(&fix), &lighthouse_test_support::cel()).is_ok(),
            "{fine}"
        );
    }
    for bad in ["files", "model", "rule", "path"] {
        let fix = command(&format!("command: [fixer, '{{{bad}}}']"));
        rejected(fixable(&fix), &format!("`{{{bad}}}` is not a placeholder"));
    }
    let filter = command("command: [gofmt]\n    stdin: file\n    output: stdout");
    assert!(Catalog::from_files(fixable(&filter), &lighthouse_test_support::cel()).is_ok());
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
    let catalog = Catalog::from_files(fixable(&scoped), &lighthouse_test_support::cel()).unwrap();
    let FixKind::Command(spec) = &catalog.decision("p/a").unwrap().fix.as_ref().unwrap().kind
    else {
        panic!("command expected");
    };
    assert_eq!(spec.scope, Some(FixScope::Files));
}

#[test]
fn a_fix_has_a_priority_that_defaults_to_zero() {
    let plain = Catalog::from_files(fixable(MOVE), &lighthouse_test_support::cel()).unwrap();
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
    let catalog = Catalog::from_files(fixable(&low), &lighthouse_test_support::cel()).unwrap();
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
    let plain = Catalog::from_files(
        decision_with(&format!("{CHECKED}{unfixed}")),
        &lighthouse_test_support::cel(),
    )
    .unwrap();
    let with_fix = Catalog::from_files(fixable(MOVE), &lighthouse_test_support::cel()).unwrap();
    let (a, b) = (
        plain.decision("p/a").unwrap(),
        with_fix.decision("p/a").unwrap(),
    );

    assert_ne!(a.version(), b.version());
    assert_eq!(a.meaning_version(), b.meaning_version());
    assert_eq!(a.check_revision(), b.check_revision());
}

#[test]
fn the_effective_safety_of_a_fix_is_decided_in_one_place() {
    let load = |fix: &str| {
        Catalog::from_files(fixable(fix), &lighthouse_test_support::cel())
            .unwrap()
            .decision("p/a")
            .unwrap()
            .clone()
    };
    let ops_safe = load(MOVE);
    let ops_unsafe = load(&MOVE.replace("safety: safe", "safety: unsafe"));
    let model = load("  fix:\n    safety: unsafe\n    type: model\n");
    let unfixed = Catalog::from_files(
        decision_with(&format!(
            "{CHECKED}{}",
            EXAMPLES.replace("      fixed: [{ path: a.go, body: y }]\n", "")
        )),
        &lighthouse_test_support::cel(),
    )
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
    let catalog = Catalog::from_files(fixable(MOVE), &lighthouse_test_support::cel()).unwrap();
    let ops = catalog.decision("p/a").unwrap().clone();
    let warned = ops.clone().map_spec(|spec| DecisionSpec {
        severity: Some(Severity::Warn),
        ..spec
    });
    let model = Catalog::from_files(
        fixable("  fix:\n    safety: unsafe\n    type: model\n"),
        &lighthouse_test_support::cel(),
    )
    .unwrap()
    .decision("p/a")
    .unwrap()
    .clone();

    assert_eq!(ops.fix_safety(Some(Safety::Unsafe)), Safety::Unsafe);
    assert_eq!(warned.fix_safety(Some(Safety::Safe)), Safety::Safe);
    assert_eq!(model.fix_safety(Some(Safety::Safe)), Safety::Safe);
}

#[test]
fn the_bundled_decisions_a_model_may_fix_declare_how_far_it_may_go_or_take_references() {
    let bundled = lighthouse_test_support::bundled();
    let scope_of = |id: &str| match &bundled.decision(id).unwrap().fix.as_ref().unwrap().kind {
        FixKind::Model { scope, .. } => *scope,
        other => panic!("{id}: a model fix expected, found {other:?}"),
    };
    for id in ["design/exported-doc", "design/symbols", "testing/cases"] {
        assert_eq!(scope_of(id), Some(FixScope::Symbol), "{id}");
    }
    for id in [
        "design/errors",
        "design/signature",
        "design/prefer-method",
        "design/naming",
    ] {
        assert_eq!(scope_of(id), None, "{id}: the default, `references`");
    }
}
