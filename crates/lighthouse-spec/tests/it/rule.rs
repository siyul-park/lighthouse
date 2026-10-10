use lighthouse_model::{Diagnostic, Options};
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest, Scope};
use lighthouse_spec::{Catalog, DecisionRule};
use serde::Deserialize;
use serde_json::json;

const ID: &str = "core/max-file-lines";

#[derive(Deserialize)]
struct Max {}

fn no_findings(_: &RuleManifest, _: &Ctx, _: Max) -> Result<Vec<Diagnostic>, Error> {
    Ok(Vec::new())
}

fn options(value: serde_json::Value) -> Options {
    value.as_object().unwrap().clone()
}

#[test]
fn decision_rule() {
    let rule = DecisionRule::new(ID, &[], no_findings);
    assert_eq!(rule.manifest().id, ID);
    assert_eq!(rule.manifest().scope, Scope::File);
    assert!(rule.validate(&options(json!({}))).is_ok());
    assert!(rule.validate(&options(json!({ "max": 7 }))).is_ok());
    assert!(rule.validate(&options(json!({ "mx": 7 }))).is_err());
    assert!(rule.validate(&options(json!({ "max": "big" }))).is_err());
}

#[test]
fn decision_rule_new() {
    let rule = DecisionRule::new(ID, &["lines", "tokens"], no_findings);
    assert_eq!(rule.manifest().analyzers, ["lines", "tokens"]);
    let missing = std::panic::catch_unwind(|| DecisionRule::new("core/absent", &[], no_findings));
    assert!(missing.is_err());
}

#[test]
fn decision_rule_options() {
    let decision = Catalog::bundled().decision(ID).unwrap();

    let resolved = decision.rule_options(&options(json!({})), None).unwrap();
    assert!(resolved.contains_key("max"), "{resolved:?}");
    let set = decision
        .rule_options(&options(json!({ "max": 7 })), Some("go"))
        .unwrap();
    assert_eq!(set["max"], 7);

    let refused = decision
        .rule_options(&options(json!({ "mx": 7 })), None)
        .unwrap_err();
    assert!(matches!(refused, Error::Options { ref rule, .. } if rule == ID));
}
