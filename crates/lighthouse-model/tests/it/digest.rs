use lighthouse_model::evidence_digest;
use serde_json::json;

#[test]
fn the_digest_follows_what_the_evidence_says_not_how_it_is_spaced() {
    let evidence = json!({ "lines": 6, "name": "a b" });

    assert_eq!(
        evidence_digest(&evidence),
        evidence_digest(&json!({ "name": "a \n  b", "lines": 6 }))
    );
    assert_ne!(
        evidence_digest(&evidence),
        evidence_digest(&json!({ "lines": 8, "name": "a b" }))
    );
}
