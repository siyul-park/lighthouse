//! The rule authoring tools: create, update and test.

use lighthouse_session::{Authored, Session, create_rule, test_rules, update_rule};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::tools::{Outcome, document, fail};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateArgs {
    pattern: Value,
    rule: Option<Value>,
    examples: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateArgs {
    id: String,
    patch: Value,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TestArgs {
    #[serde(default)]
    ids: Vec<String>,
}

pub fn create(args: CreateArgs) -> Outcome {
    let pattern = document(args.pattern, "pattern")?;
    let rule = args.rule.map(|r| document(r, "rule")).transpose()?;
    let examples = match document(args.examples, "examples")? {
        Value::Array(items) => items,
        Value::Null => Vec::new(),
        _ => return Err("`examples` must be a list".into()),
    };
    let session = Session::load(None).map_err(fail)?;
    create_rule(session, pattern, rule, examples)
        .map(authored)
        .map_err(fail)
}

pub fn update(args: UpdateArgs) -> Outcome {
    let patch = document(args.patch, "patch")?;
    let session = Session::load(None).map_err(fail)?;
    update_rule(session, &args.id, &patch)
        .map(authored)
        .map_err(fail)
}

pub fn test(args: TestArgs) -> Outcome {
    let session = Session::load_or_default(None).map_err(fail)?;
    let report = test_rules(&session, &args.ids, None).map_err(fail)?;
    Ok(json!({
        "ok": report.failures.is_empty(),
        "patterns": report.patterns,
        "languages": report.languages,
        "runs": report.runs,
        "failures": report.failures,
    }))
}

fn authored(done: Authored) -> Value {
    let note = if done.plugin_listed {
        "the `local` plugin is listed; enable the rule with `extends = [\"local/recommended\"]` or a [rules] entry if it is not on yet"
    } else {
        "add \"local\" to `plugins` in lighthouse.toml, and enable the rule, for `check` to run it"
    };
    json!({
        "id": done.id,
        "path": done.path.display().to_string(),
        "created": done.created,
        "tested": { "runs": done.test.runs, "languages": done.test.languages },
        "local_plugin_listed": done.plugin_listed,
        "note": note,
    })
}
