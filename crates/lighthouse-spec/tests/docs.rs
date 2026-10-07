use std::{collections::BTreeMap, fs, path::PathBuf};

use lighthouse_spec::{Catalog, docs, pattern_markdown};

fn fixture() -> Catalog {
    let files = [
        (
            "demo/pack.yaml",
            "id: demo\ntitle: Demo Patterns\nintro: Intro of the pack.\nsections: [one]\n",
        ),
        (
            "demo/one/section.yaml",
            "id: one\ntitle: One\nintro: Intro of the section.\npatterns: [alpha, beta]\n",
        ),
        (
            "demo/one/alpha.yaml",
            "id: demo/alpha
title: Alpha holds
intent: Keeps alpha true.
scope: symbol
requirement: Alpha MUST hold.
enforcement: mechanical
evidence: [name]
options:
  limit:
    type: int
    default: 3
    description: How many.
    per_language: { go: 5 }
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
tuning:
  go: Go spells it differently.
citation: Someone 2001
",
        ),
        (
            "demo/one/beta.yaml",
            "id: demo/beta
title: Beta is advice
intent: Advises beta.
scope: project
requirement: Beta SHOULD be considered.
enforcement: doc
",
        ),
    ];
    Catalog::from_files(
        files
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect::<BTreeMap<_, _>>(),
    )
    .unwrap()
}

#[test]
fn pack_markdown_is_stable() {
    let docs = docs(&fixture());
    assert_eq!(docs.keys().collect::<Vec<_>>(), ["patterns/demo.md"]);
    insta::assert_snapshot!(docs["patterns/demo.md"]);
}

#[test]
fn docs_never_mention_implementation_details() {
    let text = docs(&fixture())["patterns/demo.md"].clone();
    for hidden in ["CP999", "evidence", "expect", "implementation"] {
        assert!(!text.contains(hidden), "{hidden}");
    }
}

#[test]
fn pattern_markdown_honours_heading_level() {
    let catalog = fixture();
    let text = pattern_markdown(catalog.pattern("demo/beta").unwrap(), 1);
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
    let files = [
        (
            "demo/pack.yaml",
            "id: demo\ntitle: Demo Patterns\nintro: Intro.\nsections: [one]\n",
        ),
        (
            "demo/one/section.yaml",
            "id: one\ntitle: One\nintro: Intro.\npatterns: [gamma]\n",
        ),
        (
            "demo/one/gamma.yaml",
            "id: demo/gamma
title: Gamma order
intent: Orders gamma.
scope: file
requirement: Gamma MUST come first.
enforcement: mechanical
evidence: [name]
implementation:
  builtin: p/a
fix:
  safety: safe
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
",
        ),
        ("demo/one/testdata/bad.go", "first()\nsecond()\n"),
        ("demo/one/testdata/bad.rs", "first();\n"),
    ];
    Catalog::from_files(
        files
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect::<BTreeMap<_, _>>(),
    )
    .unwrap()
}

#[test]
fn a_fixable_pattern_shows_a_diff_and_links_other_languages() {
    let text = docs(&fixed_fixture())["patterns/demo.md"].clone();
    assert!(
        text.contains("`demo/gamma` · file · mechanical→error · fix: safe"),
        "{text}"
    );
    assert!(text.contains("```diff\n--- a/a.go\n+++ b/a.go\n"), "{text}");
    assert!(text.contains("+second()"), "{text}");
    assert!(!text.contains("```go valid"), "{text}");
    assert!(
        text.contains("Also: rust ([invalid](../../patterns/demo/one/testdata/bad.rs)), python"),
        "{text}"
    );
}

#[test]
fn short_decisions_are_rows_and_entries_are_linked() {
    let text = docs(&fixture())["patterns/demo.md"].clone();
    assert!(
        text.contains("| `demo/beta` | Beta is advice | doc |  | Beta SHOULD be considered. |")
    );
    assert!(text.contains("| [`demo/alpha`](#alpha-holds) |"));
    assert!(!text.contains("### Beta is advice"));
}
