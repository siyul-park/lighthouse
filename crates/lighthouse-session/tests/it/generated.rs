//! What Lighthouse generates from the catalog: the docs and the agent skill.

use std::collections::BTreeSet;

use crate::fixture::{project, session};

#[test]
fn bundled_docs() {
    let docs = lighthouse_session::bundled_docs().unwrap();
    let dir = lighthouse_session::DOCS_DIR;
    assert!(docs.contains_key(&format!("{dir}/fix-operations.md")));
    let functions = &docs[&format!("{dir}/cel-functions.md")];
    assert!(functions.contains("upperAscii") || functions.len() > 100);
    // A page for each pack of the bundled catalog.
    assert!(
        docs.keys().any(|k| k.ends_with("core.md")),
        "{:?}",
        docs.keys()
    );
    assert!(docs.values().all(|page| !page.trim().is_empty()));
}

#[test]
fn skill() {
    let dir = project("warn");
    let session = session(dir.path());
    let active = BTreeSet::from(["local/short".to_owned()]);

    let text = lighthouse_session::skill(&session.catalog, &active);
    assert!(text.contains("## Active decisions (1)"), "{text}");
    assert!(
        text.contains("- `local/short` (warn, fixable: unsafe) Notes are short"),
        "{text}"
    );
    assert!(text.contains("### local"));
    // The loop an agent follows is stated in it once, from the shared text.
    assert!(text.contains(lighthouse_session::AGENT_LOOP.lines().next().unwrap()));
    // What is not active is not listed.
    assert!(!text.contains("core/max-lines"));
    assert!(lighthouse_session::skill(&session.catalog, &BTreeSet::new()).contains("(0)"));
}
