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

#[test]
fn the_digest_follows_how_sure_the_model_was() {
    let decided = json!({ "confidence": 0.95, "explored": { "confidence": 0.7 } });

    assert_ne!(
        evidence_digest(&decided),
        evidence_digest(&json!({ "confidence": 0.55, "explored": { "confidence": 0.7 } }))
    );
}
