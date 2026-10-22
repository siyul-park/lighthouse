//! The tools an agent reports and the capability that says a model is one.

use lighthouse_protocol::{Capabilities, EditResult, Supports, Tool, ToolEvent};

#[test]
fn parse_names_the_five_tools_and_nothing_else() {
    for (word, tool) in [
        ("read", Tool::Read),
        ("search", Tool::Search),
        ("list", Tool::List),
        ("edit", Tool::Edit),
        ("run", Tool::Run),
    ] {
        assert_eq!(Tool::parse(word), Some(tool));
    }
    assert_eq!(Tool::parse("grep"), None);
}

#[test]
fn edits_says_whether_a_model_runs_as_an_agent() {
    let agent = Capabilities {
        agent: Some(Supports { supported: true }),
        ..Capabilities::default()
    };

    assert!(agent.edits());
    assert!(!Capabilities::default().edits());
}

#[test]
fn an_edit_result_without_events_has_none() {
    let result: EditResult = serde_json::from_str(r#"{"said": "ok"}"#).unwrap();
    let event = ToolEvent {
        tool: Tool::Read,
        path: Some("a".to_owned()),
        outside: false,
        pattern: None,
        command: None,
    };

    assert!(result.events.is_empty());
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"tool":"read","path":"a"}"#
    );
}
