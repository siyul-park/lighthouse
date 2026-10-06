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

#[test]
fn sources_yaml_covers_every_normative_line_of_the_vendored_docs() {
    Catalog::bundled().verify_sources(&vendored()).unwrap();
}

#[test]
fn drift_in_a_document_is_reported() {
    let mut changed = vendored();
    let text = changed.get_mut("testing").unwrap();
    *text = text.replace("A test MUST show public usage", "A test MUST show usage");
    let err = Catalog::bundled()
        .verify_sources(&changed)
        .unwrap_err()
        .to_string();
    assert!(err.contains("A test MUST show usage"), "{err}");
    assert!(err.contains("stale"), "{err}");
}

#[test]
fn wrapped_bullets_are_reported_as_one_line() {
    let err = Catalog::default()
        .verify_sources(&docs("## H\n\n- first part\n  second part\n"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("first part second part"), "{err}");
}

#[test]
fn sources_verify_by_ref_and_flag_new_bullets() {
    let err = Catalog::default()
        .verify_sources(&docs("## H\n\n- one\n"))
        .unwrap_err()
        .to_string();
    let reference = err
        .split("missing: [")
        .nth(1)
        .unwrap()
        .split(':')
        .next()
        .unwrap();
    let sources = format!("- ref: {reference}\n  text: one\n  omitted: project-specific\n");
    let files = BTreeMap::from([("sources.yaml".to_owned(), sources)]);
    let catalog = Catalog::from_files(files).unwrap();
    catalog.verify_sources(&docs("## H\n\n- one\n")).unwrap();
    catalog
        .verify_sources(&docs("## H\n\n- extra MUST\n- one\n"))
        .unwrap_err();
}
