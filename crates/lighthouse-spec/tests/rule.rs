use lighthouse_model::{Diagnostic, Options};
use lighthouse_plugin::{Ctx, Error, Rule, RuleMeta, Scope};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::json;

const ID: &str = "core/max-file-lines";

#[derive(Deserialize)]
struct Max {}

fn no_findings(_: &RuleMeta, _: &Ctx, _: Max) -> Result<Vec<Diagnostic>, Error> {
    Ok(Vec::new())
}

fn options(value: serde_json::Value) -> Options {
    value.as_object().unwrap().clone()
}

#[test]
fn pattern_rule() {
    let rule = PatternRule::new(ID, &[], no_findings);
    assert_eq!(rule.meta().id, ID);
    assert_eq!(rule.meta().scope, Scope::File);
    assert!(rule.validate(&options(json!({}))).is_ok());
    assert!(rule.validate(&options(json!({ "max": 7 }))).is_ok());
    assert!(rule.validate(&options(json!({ "mx": 7 }))).is_err());
    assert!(rule.validate(&options(json!({ "max": "big" }))).is_err());
}

#[test]
fn pattern_rule_new() {
    let rule = PatternRule::new(ID, &["lines", "tokens"], no_findings);
    assert_eq!(rule.meta().analyzers, ["lines", "tokens"]);
    let missing = std::panic::catch_unwind(|| PatternRule::new("core/absent", &[], no_findings));
    assert!(missing.is_err());
}
