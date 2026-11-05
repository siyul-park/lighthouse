//! The options of an engine: trust, the result cache, known gaps, and which rules the
//! configuration enables.

use super::*;

#[test]
fn engine_with_trust_tells_the_rules_the_user_trusts_the_project() {
    let dir = project(&[("x.txt", b"1")]);
    let untrusted = engine(&dir, ALL).unwrap();
    let before = untrusted
        .check(&root(&dir), &["fake/each".to_owned()])
        .unwrap();
    assert!(before.diagnostics.iter().all(|d| d.message == "file"));

    let trusted = engine(&dir, ALL).unwrap().with_trust(true);
    let after = trusted
        .check(&root(&dir), &["fake/each".to_owned()])
        .unwrap();

    assert!(!after.diagnostics.is_empty());
    assert!(
        after
            .diagnostics
            .iter()
            .all(|d| d.message == "file (trusted)")
    );
}

#[test]
fn engine_active() {
    let dir = project(&[("x.txt", b"1")]);
    let engine = engine(&dir, ALL).unwrap();
    assert!(engine.active("fake/each"));
    assert!(!engine.active("fake/other"), "a rule the config leaves out");
    assert!(!engine.active("nope/nope"), "a rule nobody registered");
}

#[test]
fn engine_with_incomplete() {
    let dir = project(&[("x.txt", b"1")]);
    let gap = Incomplete {
        path: None,
        provider: None,
        reason: "plugin failed to start".to_owned(),
    };
    let engine = engine(&dir, ALL).unwrap();
    let engine = engine.with_incomplete(vec![gap.clone()]);
    for _ in 0..2 {
        let out = engine.check(&root(&dir), &[]).unwrap();
        assert_eq!(out.incomplete, std::slice::from_ref(&gap));
    }
}
