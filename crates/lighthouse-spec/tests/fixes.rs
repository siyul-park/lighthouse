//! The `fix:` block of a pattern: what a catalog accepts and what it refuses.

use std::collections::BTreeMap;

use lighthouse_model::{Capability, Safety};
use lighthouse_spec::{Catalog, Fix, FixKind, OpSpec, ReorderScope, timeout_seconds};

const PATTERN: &str = "id: p/a\ntitle: A\nintent: i\nscope: file\nrequirement: A MUST b.\nenforcement: mechanical\nevidence: [x]\n";

fn base() -> BTreeMap<String, String> {
    [
        ("p/pack.yaml", "id: p\ntitle: P\nintro: x\nsections: [s]\n"),
        (
            "p/s/section.yaml",
            "id: s\ntitle: S\nintro: x\npatterns: [a]\n",
        ),
        ("p/s/a.yaml", PATTERN),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

fn pattern_with(extra: &str) -> BTreeMap<String, String> {
    let mut files = base();
    files.insert("p/s/a.yaml".to_owned(), format!("{PATTERN}{extra}"));
    files
}

fn rejected(files: BTreeMap<String, String>, needle: &str) {
    let err = Catalog::from_files(files).unwrap_err();
    assert!(
        err.to_string().contains(needle),
        "{err} should mention {needle}"
    );
}

const IMPLEMENTED: &str = "implementation:\n  builtin: p/a\n";
const EXAMPLES: &str = "examples:
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

fn fixable(fix: &str) -> std::collections::BTreeMap<String, String> {
    pattern_with(&format!("{IMPLEMENTED}{fix}{EXAMPLES}"))
}

const MOVE: &str = "fix:
  safety: safe
  requires: [extent, complete-references]
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

    let fix: &Fix = catalog.pattern("p/a").unwrap().fix.as_ref().unwrap();

    assert_eq!(fix.safety, Safety::Safe);
    assert_eq!(
        fix.requires,
        [Capability::Extent, Capability::CompleteReferences]
    );
    let FixKind::Ops(ops) = &fix.kind else {
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
fn a_fix_needs_exactly_one_kind() {
    let both =
        "fix:\n  safety: suggested\n  ops: [{ op: delete, node: x }]\n  command: { argv: [x] }\n";
    rejected(fixable(both), "exactly one of `ops`, `command` or `rpc`");
    let none = "fix:\n  safety: suggested\n";
    rejected(fixable(none), "exactly one of `ops`, `command` or `rpc`");
}

#[test]
fn the_rpc_kind_is_kept_without_failing_the_pack() {
    let rpc = "fix:\n  safety: suggested\n  rpc: { method: fix }\n";

    let catalog = Catalog::from_files(fixable(rpc)).unwrap();

    let fix = catalog.pattern("p/a").unwrap().fix.as_ref().unwrap();
    assert!(matches!(fix.kind, FixKind::Rpc(_)));
}

#[test]
fn an_unknown_operation_or_parameter_is_refused() {
    let op = "fix:\n  safety: suggested\n  ops: [{ op: shuffle }]\n";
    rejected(fixable(op), "shuffle");
    let param = "fix:\n  safety: suggested\n  ops: [{ op: delete, node: x, colour: red }]\n";
    rejected(fixable(param), "colour");
}

#[test]
fn operation_parameters_are_checked() {
    let both = "fix:\n  safety: suggested\n  ops: [{ op: move, node: x, before: y, after: z }]\n";
    rejected(fixable(both), "exactly one of `before` or `after`");
    let neither = "fix:\n  safety: suggested\n  ops: [{ op: move, node: x }]\n";
    rejected(fixable(neither), "exactly one of `before` or `after`");
    let delete = "fix:\n  safety: suggested\n  ops: [{ op: delete, file: x }]\n";
    rejected(fixable(delete), "delete needs `node`, or `file` and `span`");
    let key = "fix:\n  safety: suggested\n  ops: [{ op: reorder, scope: file, by: [group] }]\n";
    rejected(fixable(key), "qualified `plugin/name`");
    let scope = "fix:\n  safety: suggested\n  ops: [{ op: reorder, scope: world, by: [a/b] }]\n";
    rejected(fixable(scope), "world");
}

#[test]
fn expressions_must_compile() {
    let bad = "fix:\n  safety: suggested\n  ops: [{ op: delete, node: 'finding..x(' }]\n";
    rejected(fixable(bad), "delete `node`");
    let guard = "fix:\n  safety: suggested\n  ops: [{ op: delete, node: x, when: '1 +' }]\n";
    rejected(fixable(guard), "`when`");
    let text = "fix:\n  safety: suggested\n  ops: [{ op: replace, file: x, span: y, text: '{{ 1 + }}' }]\n";
    rejected(fixable(text), "replace `text`");
    let open =
        "fix:\n  safety: suggested\n  ops: [{ op: replace, file: x, span: y, text: '{{ x' }]\n";
    rejected(fixable(open), "never closed");
}

#[test]
fn safe_is_reserved_for_mechanical_patterns() {
    let heuristic = fixable("fix:\n  safety: safe\n  ops: [{ op: delete, node: x }]\n")
        .into_iter()
        .map(|(k, v)| (k, v.replace("mechanical", "heuristic")))
        .collect();
    rejected(heuristic, "reserved for mechanical patterns");
}

#[test]
fn a_fix_belongs_to_an_implemented_pattern_with_a_fixed_example() {
    let fix = "fix:\n  safety: safe\n  ops: [{ op: delete, node: x }]\n";
    rejected(
        pattern_with(&format!("{fix}{EXAMPLES}")),
        "no findings to fix",
    );
    let unfixed = EXAMPLES.replace("    fixed: [{ path: a.go, body: y }]\n", "");
    rejected(
        pattern_with(&format!("{IMPLEMENTED}{fix}{unfixed}")),
        "needs an invalid example with `fixed`",
    );
}

#[test]
fn fixed_examples_must_name_files_of_an_invalid_example() {
    let fix = "fix:\n  safety: safe\n  ops: [{ op: delete, node: x }]\n";
    let elsewhere = EXAMPLES.replace("fixed: [{ path: a.go,", "fixed: [{ path: b.go,");
    rejected(
        pattern_with(&format!("{IMPLEMENTED}{fix}{elsewhere}")),
        "not a file of the example",
    );
    let valid = EXAMPLES.replace(
        "    files: [{ path: a.go, body: y }]\n",
        "    files: [{ path: a.go, body: y }]\n    fixed: [{ path: a.go, body: y }]\n",
    );
    rejected(
        pattern_with(&format!("{IMPLEMENTED}{fix}{valid}")),
        "only an invalid example has `fixed`",
    );
    rejected(
        pattern_with(&format!("{IMPLEMENTED}{EXAMPLES}")),
        "`fixed` needs a `fix`",
    );
}

#[test]
fn a_command_needs_a_program_and_a_usable_timeout() {
    let empty = "fix:\n  safety: suggested\n  command: { argv: [] }\n";
    rejected(fixable(empty), "needs a program");
    let slow = "fix:\n  safety: suggested\n  command: { argv: [gofmt], timeout: soon }\n";
    rejected(fixable(slow), "command.timeout");
    let ok = "fix:\n  safety: suggested\n  command: { argv: [gofmt, -w, '{file}'], output: text, timeout: 2m }\n";
    let catalog = Catalog::from_files(fixable(ok)).unwrap();
    let fix: &Fix = catalog.pattern("p/a").unwrap().fix.as_ref().unwrap();
    assert!(matches!(&fix.kind, FixKind::Command(c) if c.argv.len() == 3));
}

#[test]
fn timeouts_read_seconds_and_minutes() {
    assert_eq!(timeout_seconds("30s"), Some(30));
    assert_eq!(timeout_seconds("30"), Some(30));
    assert_eq!(timeout_seconds("2m"), Some(120));
    assert_eq!(timeout_seconds("soon"), None);
    assert_eq!(timeout_seconds("5h"), None);
}

#[test]
fn a_fix_is_part_of_the_pattern_version_but_not_of_what_it_demands() {
    let unfixed = EXAMPLES.replace("    fixed: [{ path: a.go, body: y }]\n", "");
    let plain = Catalog::from_files(pattern_with(&format!("{IMPLEMENTED}{unfixed}"))).unwrap();
    let with_fix = Catalog::from_files(fixable(MOVE)).unwrap();
    let (a, b) = (
        plain.pattern("p/a").unwrap(),
        with_fix.pattern("p/a").unwrap(),
    );

    assert_ne!(a.version(), b.version());
    assert_eq!(a.semantic_version(), b.semantic_version());
}

#[test]
fn an_operations_guard_is_readable() {
    let guarded = "fix:\n  safety: suggested\n  ops:\n    - { op: delete, node: finding.symbol, when: \"finding.evidence.role == 'x'\" }\n    - { op: delete, node: finding.symbol }\n";
    let catalog = Catalog::from_files(fixable(guarded)).unwrap();
    let FixKind::Ops(ops) = &catalog.pattern("p/a").unwrap().fix.as_ref().unwrap().kind else {
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
        for param in op.params {
            assert!(page.contains(&format!("| `{}` |", param.name)));
        }
    }
    assert!(page.contains("| `design/group` | the declaration group |"));
    assert!(page.contains("`rpc` kind"));
    assert!(page.starts_with("<!-- Generated by `lighthouse docs generate`"));
}
