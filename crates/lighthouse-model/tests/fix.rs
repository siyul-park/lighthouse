use lighthouse_model::{Anchor, EditOp, FixOutcome, Node, Owner, Position, Safety, Span, SymbolId};
use serde_json::json;

fn at(line: u32, col: u32) -> Position {
    Position { line, col }
}

#[test]
fn a_claim_is_capped_by_the_weaker_of_the_two() {
    assert_eq!(Safety::Safe.capped(Safety::Safe), Safety::Safe);
    assert_eq!(Safety::Safe.capped(Safety::Suggested), Safety::Suggested);
    assert_eq!(Safety::Suggested.capped(Safety::Safe), Safety::Suggested);
}

#[test]
fn safety_is_read_and_written_as_its_lowercase_name() {
    assert_eq!("safe".parse::<Safety>().unwrap(), Safety::Safe);
    assert_eq!("suggested".parse::<Safety>().unwrap(), Safety::Suggested);
    assert_eq!(Safety::Suggested.to_string(), "suggested");
    let error = "unsafe".parse::<Safety>().unwrap_err();
    assert!(error.to_string().contains("unknown safety `unsafe`"));
    assert_eq!(serde_json::to_value(Safety::Safe).unwrap(), json!("safe"));
}

#[test]
fn a_declined_outcome_carries_its_reason() {
    let outcome = FixOutcome::declined("no extent");

    assert_eq!(
        outcome,
        FixOutcome::Declined {
            reason: "no extent".to_owned()
        }
    );
    assert_eq!(
        serde_json::to_value(&outcome).unwrap(),
        json!({ "outcome": "declined", "reason": "no extent" })
    );
}

#[test]
fn edit_operations_are_tagged_json_a_command_can_print() {
    let id = SymbolId::parse("m::f#function").unwrap();
    let ops = vec![
        EditOp::Move {
            node: Node::Symbol(id.clone()),
            anchor: Anchor::After(Node::Symbol(id.clone())),
        },
        EditOp::Reorder {
            owner: Owner::File("a.go".into()),
            order: vec![Node::Symbol(id)],
        },
        EditOp::DeleteRange {
            file: "a.go".into(),
            span: Span {
                start: at(1, 1),
                end: at(1, 5),
            },
        },
    ];

    let json = serde_json::to_value(&ops).unwrap();

    assert_eq!(json[0]["op"], "move");
    assert_eq!(json[0]["anchor"]["after"]["symbol"], "m::f#function");
    assert_eq!(json[1]["owner"]["file"], "a.go");
    assert_eq!(json[2]["op"], "delete_range");
    let back: Vec<EditOp> = serde_json::from_value(json).unwrap();
    assert_eq!(back, ops);
}
