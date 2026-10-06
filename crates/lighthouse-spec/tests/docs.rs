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
