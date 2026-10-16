use lighthouse_plugin::{Abstained, Error, FAIL_AT, Facts, fails};
use serde_json::json;

#[test]
fn a_probability_fails_a_subject_only_at_or_above_the_threshold() {
    assert_eq!(fails(0.95), Some(0.95));
    assert_eq!(fails(FAIL_AT), Some(FAIL_AT));
    assert_eq!(fails(0.5), None);
    assert_eq!(fails(0.02), None, "no lower bound clears a subject");
}

#[test]
fn abstentions_say_why_in_the_words_users_read() {
    assert_eq!(Abstained::Unbound.to_string(), "no model is bound");
    assert_eq!(
        Abstained::Failed("x".to_owned()).to_string(),
        "a model failed: x"
    );
}

#[test]
fn a_model_failure_names_the_model() {
    assert_eq!(
        Error::model("local", "does not train").to_string(),
        "model `local`: does not train"
    );
}

#[test]
fn facts_are_kept_by_analyzer_and_key_and_listed_in_order() {
    let mut facts = Facts::new();
    facts.insert("b", String::new(), json!(2));
    facts.insert("a", "x.go".to_owned(), json!(1));
    facts.insert("a", String::new(), json!(0));
    assert_eq!(facts.get("a", "x.go"), Some(&json!(1)));
    assert_eq!(facts.get("a", "y.go"), None);
    let listed: Vec<_> = facts.iter().map(|(a, k, _)| (a, k)).collect();
    assert_eq!(listed, [("a", ""), ("a", "x.go"), ("b", "")]);
}
