use super::*;

fn event(tool: Tool, path: Option<&str>, pattern: Option<&str>) -> ToolEvent {
    ToolEvent {
        tool,
        outside: false,
        path: path.map(str::to_owned),
        pattern: pattern.map(str::to_owned),
        command: None,
    }
}

#[test]
fn a_read_the_brief_did_not_give_is_listed_once_and_every_call_is_counted() {
    let briefed = BTreeSet::from(["src/lib.rs".to_owned()]);
    let events = [
        event(Tool::Read, Some("src/lib.rs"), None),
        event(Tool::Read, Some("src/other.rs"), None),
        event(Tool::Read, Some("src/other.rs"), None),
        event(Tool::Search, Some("."), Some("bad_one")),
        event(Tool::List, Some("src"), None),
        event(Tool::Edit, Some("src/lib.rs"), None),
    ];

    let trace = BatchTrace::of(PathBuf::from("src/lib.rs"), &briefed, &events, None);

    assert_eq!(trace.reads, ["src/other.rs"]);
    assert_eq!(trace.searches, [events[3].clone(), events[4].clone()]);
    assert_eq!(
        trace.counts,
        ToolCounts {
            read: 3,
            search: 1,
            list: 1,
            edit: 1,
            run: 0
        }
    );
}

#[test]
fn of_lists_every_read_outside_the_project_as_one_entry() {
    let outside = ToolEvent {
        outside: true,
        ..event(Tool::Read, None, None)
    };
    let events = [
        outside.clone(),
        outside,
        event(Tool::Read, Some("a.go"), None),
    ];

    let trace = BatchTrace::of(PathBuf::from("a.go"), &BTreeSet::new(), &events, None);

    assert_eq!(trace.reads, [OUTSIDE, "a.go"]);
}

#[test]
fn of_carries_why_the_events_are_not_everything() {
    let note = Some("2 event(s) past the first 10000 were not kept".to_owned());

    let trace = BatchTrace::of(PathBuf::from("a.go"), &BTreeSet::new(), &[], note.clone());

    assert_eq!(trace.incomplete, note);
}

#[test]
fn of_lists_a_thousand_distinct_reads_without_comparing_each_to_all() {
    let events: Vec<ToolEvent> = (0..5000)
        .map(|n| event(Tool::Read, Some(&format!("f{}.go", n % 2500)), None))
        .collect();

    let trace = BatchTrace::of(PathBuf::from("a.go"), &BTreeSet::new(), &events, None);

    assert_eq!(trace.reads.len(), 2500);
    assert_eq!(trace.counts.read, 5000);
}
