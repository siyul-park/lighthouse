mod support;

use std::{fs, path::PathBuf};

use lighthouse_spec::{Catalog, decision_markdown, docs, help_path};
use support::{decision, files, pack};

const ALPHA: &str = "  title: Alpha holds
  intent: Keeps alpha true.
  scope: { subject: symbol }
  requirement: Alpha MUST hold.
  enforcement: mechanical
  evidence: [name]
  options:
    type: object
    properties:
      limit:
        type: integer
        default: 3
        description: How many.
    additionalProperties: false
  languages:
    go:
      options: { limit: 5 }
      tuning: Go spells it differently.
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
  exceptions: Generated code.
  citation: Someone 2001
";

const BETA: &str = "  title: Beta is advice
  intent: Advises beta.
  scope: { subject: project }
  requirement: Beta SHOULD be considered.
  enforcement: doc
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
        ("demo/one/alpha.yaml", decision("demo/alpha", "one", ALPHA)),
        ("demo/one/beta.yaml", decision("demo/beta", "one", BETA)),
    ]))
    .unwrap()
}

#[test]
fn pack_markdown_is_stable() {
    let docs = docs(&fixture());
    assert_eq!(docs.keys().collect::<Vec<_>>(), ["decisions/demo.md"]);
    insta::assert_snapshot!(docs["decisions/demo.md"]);
}

#[test]
fn docs_never_mention_implementation_details() {
    let text = docs(&fixture())["decisions/demo.md"].clone();
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
    let text = decision_markdown(catalog.decision("demo/beta").unwrap(), 1);
    assert!(text.starts_with("# Beta is advice\n"));
    assert!(text.contains("severity `none`"));
}

#[test]
fn committed_docs_match_the_bundled_catalog() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    for (path, text) in docs(Catalog::bundled()) {
        let committed = fs::read_to_string(root.join(&path)).unwrap_or_default();
        assert!(
            committed == text,
            "{path} is stale: run `lighthouse docs generate`"
        );
    }
}

fn fixed_fixture() -> Catalog {
    let gamma = "  title: Gamma order
  intent: Orders gamma.
  scope: { subject: file }
  requirement: Gamma MUST come first.
  enforcement: mechanical
  evidence: [name]
  check:
    type: builtin
    id: p/a
  fix:
    safety: safe
    type: ops
    ops:
      - { op: delete, node: finding.symbol }
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
        ("demo/one/gamma.yaml", decision("demo/gamma", "one", gamma)),
        ("demo/one/testdata/bad.go", "first()\nsecond()\n".to_owned()),
        ("demo/one/testdata/bad.rs", "first();\n".to_owned()),
    ]))
    .unwrap()
}

#[test]
fn a_fixable_decision_shows_a_diff_and_links_other_languages() {
    let text = docs(&fixed_fixture())["decisions/demo.md"].clone();
    assert!(
        text.contains("`demo/gamma` · file · mechanical→error · fix: safe"),
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
    let text = docs(&fixture())["decisions/demo.md"].clone();
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
