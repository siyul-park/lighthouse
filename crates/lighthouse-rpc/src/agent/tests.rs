use serde_json::json;

use super::*;

fn read_in(dir: &Path, sent: Value, cut: bool) -> Edited {
    let environment = Environment::new(&BTreeMap::new(), Vec::new(), Home::User).unwrap();
    let sent: Sent = serde_json::from_value(sent).unwrap();
    read_sent(sent, cut, dir, &environment)
}

fn read(sent: Value, cut: bool) -> Edited {
    read_in(Path::new("/work/scratch"), sent, cut)
}

#[test]
fn read_sent_skips_an_event_of_an_unknown_tool_and_counts_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.go"), "x").unwrap();
    let file = dir.path().join("a.go");
    let edited = read_in(
        dir.path(),
        json!({"said": "ok", "events": [
            {"tool": "teleport", "path": "a"},
            {"tool": "read", "path": file},
            {"tool": "read", "path": 7},
            "nonsense",
        ]}),
        false,
    );

    assert_eq!(edited.events.len(), 1);
    assert_eq!(edited.events[0].path.as_deref(), Some("a.go"));
    let note = edited.incomplete.unwrap();
    assert!(note.starts_with("3 event(s) were skipped"), "{note}");
}

#[test]
fn read_sent_notes_events_that_are_not_a_list() {
    let edited = read(json!({"events": {"tool": "read"}}), false);

    assert!(edited.events.is_empty());
    assert!(edited.incomplete.unwrap().contains("not a list"));
}

#[test]
fn read_sent_keeps_the_end_of_what_was_said_to_the_cap() {
    let said = format!("{}{}", "e".repeat(5000), "fixed");
    let edited = read(json!({"said": said}), false);

    assert_eq!(edited.said.len(), NOTES_BYTES);
    assert!(edited.said.ends_with("fixed"));
    assert_eq!(edited.incomplete, None);
}

#[test]
fn read_sent_says_when_the_output_was_cut() {
    let edited = read(json!({}), true);

    assert!(edited.incomplete.unwrap().contains("cap"));
}

#[test]
fn read_sent_keeps_no_more_events_than_the_cap() {
    let events = vec![json!({"tool": "run", "command": "x"}); MAX_EVENTS + 3];

    let edited = read(json!({"events": events}), false);

    assert_eq!(edited.events.len(), MAX_EVENTS);
    assert!(edited.incomplete.unwrap().starts_with("3 event(s)"));
}
