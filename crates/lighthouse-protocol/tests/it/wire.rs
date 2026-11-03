//! The shapes of the wire messages that carry optional fields.

use lighthouse_protocol::{Edge, EdgeKind, Node, Resolution, When};

fn edge(when: When) -> Edge {
    Edge {
        kind: EdgeKind::References,
        from: Node::Module("m".to_owned()),
        to: "m::a#function".to_owned(),
        resolution: Resolution::Semantic,
        site: None,
        when,
    }
}

#[test]
fn when_is_left_out_of_an_edge_unless_the_use_is_evaluated_at_load() {
    let call = serde_json::to_value(edge(When::Call)).unwrap();
    let load = serde_json::to_value(edge(When::Load)).unwrap();

    assert!(call.get("when").is_none());
    assert_eq!(load["when"], "load");
    let read: Edge = serde_json::from_value(call).unwrap();
    assert_eq!(read.when, When::Call);
}

#[test]
fn is_call_is_true_for_the_default_alone() {
    assert!(When::default().is_call());
    assert!(!When::Load.is_call());
}
