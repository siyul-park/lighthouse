//! A conflict an example declares, as a record and as a line.

use lighthouse_engine::Interference;

fn conflict(introduced: bool) -> Interference {
    Interference {
        decision: "local/a".to_owned(),
        example: "ok".to_owned(),
        with: "local/b".to_owned(),
        introduced,
        places: vec!["a.txt:1".to_owned(), "b.txt:2".to_owned()],
        message: "too long".to_owned(),
    }
}

#[test]
fn interference() {
    let conflict = conflict(false);

    assert_eq!(
        (conflict.decision.as_str(), conflict.with.as_str()),
        ("local/a", "local/b")
    );
    assert_eq!(conflict.places.len(), 2);
}

#[test]
fn interference_line() {
    assert_eq!(
        conflict(false).line(),
        "local/a ok: it also breaks local/b at a.txt:1, b.txt:2 (too long)"
    );
    assert!(conflict(true).line().contains("its fix introduces local/b"));
}
