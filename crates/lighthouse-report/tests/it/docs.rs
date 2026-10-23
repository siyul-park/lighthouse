use std::{fs, path::PathBuf};

use lighthouse_report::docs::{
    Density, FunctionDoc, OPERATIONS, decision_markdown, fix_operations_markdown, help_path, pages,
};
use lighthouse_spec::Catalog;
use lighthouse_test_support::catalog::{decision, files, pack};

const ALPHA: &str = "  title: Alpha holds
  context: Keeps alpha true.
  scope: { subject: symbol }
  requirement: Alpha MUST hold.
  severity: error
  check:
    type: builtin
    id: demo/alpha
  options:
    properties:
      limit:
        type: integer
        default: 3
        description: How many.
    additionalProperties: false
  languages:
    go:
      options: { limit: 5 }
  examples:
    - name: broken
      language: go
      kind: invalid
      options: { limit: 1 }
      files:
        - { path: a.go, body: 'broken()' }
        - { path: b.go, body: 'also()' }
      expect: [{ line: 1 }]
    - name: fine
      language: go
      kind: valid
      files: [{ path: a.go, body: 'fine()' }]
  provenance:
    wasDerivedFrom: [Someone 2001]
";

const BETA: &str = "  title: Beta is advice
  context: Advises beta.
  scope: { subject: project }
  requirement: Beta SHOULD be considered.
";

fn fixture() -> Catalog {
    let mut demo = pack("demo", &[("one", &["alpha", "beta"])]);
    demo = demo
        .replace("title: DEMO", "title: Demo Decisions")
        .replace(
            "intro: x\n  sections",
            "intro: Intro of the pack.\n  sections",
        )
        .replace("title: ONE", "title: One")
        .replace(
            "intro: x\n      decisions",
            "intro: Intro of the section.\n      decisions",
        );
    Catalog::from_files(files(&[
        ("demo/pack.yaml", demo),
        ("demo/one/alpha.yaml", decision("demo/alpha", ALPHA)),
        ("demo/one/beta.yaml", decision("demo/beta", BETA)),
    ]))
    .unwrap()
}

#[test]
fn pack_markdown_is_stable() {
    let docs = pages(&fixture());
    assert_eq!(docs.keys().collect::<Vec<_>>(), ["decisions/demo.md"]);
    insta::assert_snapshot!(docs["decisions/demo.md"]);
}

#[test]
fn docs_never_mention_implementation_details() {
    let text = pages(&fixture())["decisions/demo.md"].clone();
    for hidden in [
        "CP999",
        "evidence",
        "expect",
        "implementation",
        "apiVersion",
    ] {
        assert!(!text.contains(hidden), "{hidden}");
    }
}

#[test]
fn decision_markdown_honours_heading_level() {
    let catalog = fixture();
    let text = decision_markdown(
        catalog.decision("demo/beta").unwrap(),
        Density::Entry {
            level: 1,
            sources: None,
        },
    );
    assert!(text.starts_with("# Beta is advice\n"));
    assert!(text.contains("· doc"));
}

#[test]
fn committed_docs_match_the_bundled_catalog() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    for (path, text) in pages(Catalog::bundled()) {
        let committed = fs::read_to_string(root.join(&path)).unwrap_or_default();
        assert!(
            committed == text,
            "{path} is stale: run `lighthouse docs generate`"
        );
    }
}

fn fixed_fixture() -> Catalog {
    let gamma = "  title: Gamma order
  context: Orders gamma.
  scope: { subject: file }
  requirement: Gamma MUST come first.
  severity: error
  check:
    type: builtin
    id: p/a
  fix:
    safety: safe
    type: cel
    edits: '[remove(finding.symbol)]'
  examples:
    - name: bad
      language: go
      kind: invalid
      files: [{ path: a.go, source: testdata/bad.go }]
      fixed: [{ path: a.go, body: 'second()\\nfirst()' }]
      expect: [{ line: 1 }]
    - name: good
      language: go
      kind: valid
      files: [{ path: a.go, body: 'good()' }]
    - name: bad rust
      language: rust
      kind: invalid
      files: [{ path: a.rs, source: testdata/bad.rs }]
      expect: [{ line: 1 }]
    - name: inline
      language: python
      kind: valid
      files: [{ path: a.py, body: 'pass' }]
";
    Catalog::from_files(files(&[
        ("demo/pack.yaml", pack("demo", &[("one", &["gamma"])])),
        ("demo/one/gamma.yaml", decision("demo/gamma", gamma)),
        ("demo/one/testdata/bad.go", "first()\nsecond()\n".to_owned()),
        ("demo/one/testdata/bad.rs", "first();\n".to_owned()),
    ]))
    .unwrap()
}

#[test]
fn a_fixable_decision_shows_a_diff_and_links_other_languages() {
    let text = pages(&fixed_fixture())["decisions/demo.md"].clone();
    assert!(
        text.contains("`demo/gamma` · file · error · builtin · fix: safe"),
        "{text}"
    );
    assert!(text.contains("```diff\n--- a/a.go\n+++ b/a.go\n"), "{text}");
    assert!(text.contains("+second()"), "{text}");
    assert!(!text.contains("```go valid"), "{text}");
    assert!(
        text.contains("Also: rust ([invalid](../../decisions/demo/one/testdata/bad.rs)), python"),
        "{text}"
    );
}

#[test]
fn short_decisions_are_rows_and_entries_are_linked() {
    let text = pages(&fixture())["decisions/demo.md"].clone();
    assert!(
        text.contains("| `demo/beta` | Beta is advice | doc |  | Beta SHOULD be considered. |")
    );
    assert!(text.contains("| [`demo/alpha`](#alpha-holds) |"));
    assert!(!text.contains("### Beta is advice"));
}

#[test]
fn a_decision_is_found_in_the_docs_by_its_page_and_heading() {
    let catalog = fixture();
    assert_eq!(
        help_path(catalog.decision("demo/alpha").unwrap()),
        "docs/decisions/demo.md#alpha-holds"
    );
    assert_eq!(
        help_path(catalog.decision("demo/beta").unwrap()),
        "docs/decisions/demo.md",
        "a decision shown as a row has no entry of its own"
    );
}

#[test]
fn the_fix_operations_page_documents_every_operation_and_the_orders() {
    let page = fix_operations_markdown();

    for op in OPERATIONS {
        assert!(page.contains(&format!("## `{}`", op.name)), "{}", op.name);
        for param in op.params {
            assert!(page.contains(&format!("| `{}` |", param.name)));
        }
    }
    assert!(page.contains("## Orders"));
    assert!(page.contains("declaration_group"));
    assert!(page.contains("returns a list of edits"));
    assert!(page.contains("`rpc` type"));
    assert!(page.contains("type: command"));
    assert!(page.starts_with("<!-- Generated by `lighthouse docs generate`"));
}

#[test]
fn cel_functions_markdown() {
    let page = lighthouse_report::docs::cel_functions_markdown(&[FunctionDoc {
        library: "p/lib".to_owned(),
        version: "1.2.3".to_owned(),
        name: "f.go".to_owned(),
        signatures: vec!["f.go(int) -> int".to_owned()],
        doc: "Goes.".to_owned(),
        example: "f.go(1) == 1".to_owned(),
        step: 3,
    }]);

    assert!(page.contains("## `p/lib` 1.2.3"));
    assert!(page.contains("### `f.go` (step 3)"));
    assert!(page.contains("- `f.go(int) -> int`"));
    assert!(page.starts_with("<!-- Generated by `lighthouse docs generate`"));
}
