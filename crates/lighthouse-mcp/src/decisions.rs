//! The decision authoring tools: create, update and test.

use lighthouse_session::{Authored, Session, create_decision, test_decisions, update_decision};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::tools::{Outcome, document, fail};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateArgs {
    id: String,
    spec: Value,
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
    let spec = document(args.spec, "spec")?;
    let examples = match document(args.examples, "examples")? {
        Value::Array(items) => items,
        Value::Null => Vec::new(),
        _ => return Err("`examples` must be a list".into()),
    };
    let session = Session::load(None).map_err(fail)?;
    create_decision(session, &args.id, spec, examples)
        .map(authored)
        .map_err(fail)
}

pub fn update(args: UpdateArgs) -> Outcome {
    let patch = document(args.patch, "patch")?;
    let session = Session::load(None).map_err(fail)?;
    update_decision(session, &args.id, &patch)
        .map(authored)
        .map_err(fail)
}

pub fn test(args: TestArgs) -> Outcome {
    let session = Session::load_or_default(None).map_err(fail)?;
    let report = test_decisions(&session, &args.ids, None).map_err(fail)?;
    Ok(json!({
        "ok": report.failures.is_empty(),
        "decisions": report.decisions,
        "languages": report.languages,
        "runs": report.runs,
        "failures": report.failures,
    }))
}

fn authored(done: Authored) -> Value {
    let note = if done.plugin_listed {
        "the `local` plugin is listed; enable the decision with `extends = [\"local/recommended\"]` or a [rules] entry if it is not on yet"
    } else {
        "add \"local\" to `plugins` in lighthouse.toml, and enable the decision, for `check` to run it"
    };
    json!({
        "id": done.id,
        "path": done.path.display().to_string(),
        "created": done.created,
        "tested": { "runs": done.test.runs, "languages": done.test.languages },
        "localPluginListed": done.plugin_listed,
        "note": note,
    })
}
