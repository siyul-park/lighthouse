use std::{collections::BTreeMap, fs, path::PathBuf};

use lighthouse_spec::Catalog;

const DOCS: [&str; 2] = ["coding-patterns", "testing"];

fn vendored() -> BTreeMap<String, String> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sources");
    DOCS.iter()
        .map(|doc| {
            let text = fs::read_to_string(dir.join(format!("{doc}.md"))).unwrap();
            ((*doc).to_owned(), text)
        })
        .collect()
}

fn docs(markdown: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("d".to_owned(), markdown.to_owned())])
}

/// What an empty catalog reports as missing for `markdown`: (ref, text) of
/// every normative line, in report order.
fn extracted(markdown: &str) -> Vec<(String, String)> {
    let Err(err) = Catalog::default().verify_sources(&docs(markdown)) else {
        return Vec::new();
    };
    let err = err.to_string();
    let missing = err
        .split("missing: [")
        .nth(1)
        .and_then(|rest| rest.split("]; stale").next())
        .unwrap();
    missing
        .split("; ")
        .map(|entry| {
            let (reference, text) = entry.split_once(": ").unwrap();
            (reference.to_owned(), text.to_owned())
        })
        .collect()
}

fn texts(markdown: &str) -> Vec<String> {
    let mut texts: Vec<_> = extracted(markdown)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    texts.sort();
    texts
}

fn sorted<const N: usize>(texts: [&str; N]) -> Vec<String> {
    let mut texts: Vec<_> = texts.into_iter().map(str::to_owned).collect();
    texts.sort();
    texts
}

fn reference(markdown: &str, text: &str) -> String {
    extracted(markdown)
        .into_iter()
        .find(|(_, t)| t == text)
        .unwrap()
        .0
}

#[test]
fn catalog_verify_sources() {
    Catalog::bundled().verify_sources(&vendored()).unwrap();

    let mut changed = vendored();
    let text = changed.get_mut("testing").unwrap();
    *text = text.replace("A test MUST show public usage", "A test MUST show usage");
    let err = Catalog::bundled()
        .verify_sources(&changed)
        .unwrap_err()
        .to_string();
    assert!(err.contains("A test MUST show usage"), "{err}");
    assert!(err.contains("stale"), "{err}");

    let markdown = "## H\n\n- one\n";
    let sources = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: SourceMap\nmetadata:\n  name: sources\nspec:\n  sources:\n    - ref: {}\n      text: one\n      omitted: project-specific\n",
        reference(markdown, "one")
    );
    let files = BTreeMap::from([("sources.yaml".to_owned(), sources)]);
    let catalog = Catalog::from_files(files).unwrap();
    catalog.verify_sources(&docs(markdown)).unwrap();
    catalog
        .verify_sources(&docs("## H\n\n- extra MUST\n- one\n"))
        .unwrap_err();
}

#[test]
fn sources_take_list_items_numbered_items_and_table_body_rows() {
    let markdown =
        "## Part\n\n- first\n* second\n\n1. numbered\n\n| H | V |\n| --- | --- |\n| a | b |\n";
    assert_eq!(
        texts(markdown),
        sorted(["first", "second", "numbered", "a | b"])
    );
    assert_eq!(
        texts("Plain prose.\n\nThis MUST be kept.\n\nShould not count.\n"),
        sorted(["This MUST be kept."])
    );
}

#[test]
fn sources_join_indented_continuation_lines_into_the_bullet() {
    assert_eq!(
        texts("## Part\n\n- first line\n  wraps here\n    and here\n- next\n"),
        sorted(["first line wraps here and here", "next"])
    );
    assert_eq!(
        texts("## H\n\n- first part\n  second part\n"),
        sorted(["first part second part"])
    );
    assert_eq!(texts("- outer\n  - inner\n"), sorted(["outer", "inner"]));
}

#[test]
fn sources_skip_backtick_and_tilde_fences() {
    assert_eq!(
        texts("```\n- a\n```\n~~~\n- b\n- c MUST\n~~~\n- d\n"),
        sorted(["d"])
    );
}

#[test]
fn source_refs_follow_content_not_position() {
    let first = "## H\n- one\n- two\n";
    let second = "## H\n- two\n- one\n";
    assert_eq!(reference(first, "one"), reference(second, "one"));
    assert_ne!(reference(first, "one"), reference(first, "two"));
    assert!(reference(first, "one").starts_with("d#h-"));

    let refs = extracted("## H\n- same\n- same\n");
    assert_ne!(refs[0].0, refs[1].0);
}
