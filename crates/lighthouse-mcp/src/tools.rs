//! Tool dispatch and the read-only tools. Every tool takes its arguments as
//! JSON and returns JSON, or a message the agent can read and act on.

use std::path::{Path, PathBuf};

use lighthouse_engine::active_rules;
use lighthouse_report::{Briefing, Detail, agent_report};
use lighthouse_session::{CheckRequest, Session, decision_rows, explain};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{decisions, fix, review};

/// Findings returned by `check` when the caller sets no limit.
const DEFAULT_LIMIT: usize = 25;

/// What a tool needs to know about its caller.
pub struct Caller {
    /// Recorded as the reviewer id of verdicts.
    pub reviewer: String,
}

pub type Outcome = Result<Value, String>;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CheckArgs {
    #[serde(default)]
    paths: Vec<PathBuf>,
    #[serde(default)]
    changed: bool,
    diff: Option<String>,
    #[serde(default)]
    rules: Vec<String>,
    limit: Option<usize>,
    detail: Option<Detail>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplainArgs {
    id: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct DecisionListArgs {
    #[serde(default)]
    all: bool,
}

/// Runs the tool `name`. Unknown and reserved names are errors.
pub fn call(name: &str, args: Value, caller: &Caller) -> Outcome {
    match name {
        "check" => check(parse(args)?),
        "explain" => explain_tool(parse(args)?),
        "decision_list" => decision_list(parse(args)?),
        "review_tasks" => review::tasks(parse(args)?),
        "review_resolve" => review::resolve(parse(args)?, &caller.reviewer),
        "review_history" => review::history(parse(args)?),
        "decision_create" => decisions::create(parse(args)?),
        "decision_update" => decisions::update(parse(args)?),
        "decision_test" => decisions::test(parse(args)?),
        "fix" => fix::fix(parse(args)?),
        "decision_similar" | "decision_proposals" => Err(format!(
            "`{name}` is reserved for a later version and not available yet"
        )),
        old if old.starts_with("rule_") => Err(format!(
            "`{old}` was renamed: call `decision_{}`",
            &old["rule_".len()..]
        )),
        _ => Err(format!("unknown tool `{name}`")),
    }
}

/// Deserializes tool arguments, naming the offending field on failure; a
/// missing `arguments` object is an empty one.
pub fn parse<T: DeserializeOwned>(args: Value) -> Result<T, String> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|e| format!("invalid arguments: {e}"))
}

/// A string argument that may also arrive as an object: JSON or YAML text is
/// parsed (YAML is a superset of JSON).
pub fn document(value: Value, what: &str) -> Result<Value, String> {
    match value {
        Value::String(text) => serde_norway::from_str(&text)
            .map_err(|e| format!("`{what}` is not valid YAML or JSON: {e}")),
        other => Ok(other),
    }
}

pub fn fail<E: ToString>(e: E) -> String {
    e.to_string()
}

/// The paths, canonical, each required to lie under the project root: an
/// agent cannot ask for a report outside the project.
pub(crate) fn inside(root: &Path, paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    let root = root.canonicalize().map_err(fail)?;
    paths
        .into_iter()
        .map(|path| {
            let absolute = path
                .canonicalize()
                .map_err(|e| format!("`{}`: {e}", path.display()))?;
            if absolute.starts_with(&root) {
                Ok(absolute)
            } else {
                Err(format!(
                    "`{}` is outside the project root {}",
                    path.display(),
                    root.display()
                ))
            }
        })
        .collect()
}

fn check(args: CheckArgs) -> Outcome {
    if args.changed && args.diff.is_some() {
        return Err("`changed` and `diff` are alternatives".into());
    }
    let session = Session::load(None).map_err(fail)?;
    let request = CheckRequest {
        paths: inside(&session.root, args.paths)?,
        changed: args.changed,
        diff: args.diff,
        rules: args.rules,
        store: true,
    };
    let checked = lighthouse_session::check(session, &request).map_err(fail)?;
    let outcome = &checked.outcome;
    let limit = Some(args.limit.unwrap_or(DEFAULT_LIMIT));
    let detail = args.detail.unwrap_or_default();
    let fixes = checked.shown_fixes(limit, detail);
    let report = agent_report(
        &outcome.diagnostics,
        &outcome.incomplete,
        &Briefing {
            detail,
            fixes: Some(&fixes),
            mcp: true,
            ..checked.briefing(limit)
        },
    );
    let mut body = report.fields;
    match detail {
        Detail::Compact => {
            if !checked.messages.is_empty() {
                body.insert("messages".to_owned(), json!(checked.messages));
            }
        }
        Detail::Full => {
            let summary = checked.summary();
            body.insert("status".to_owned(), json!(summary.status));
            body.insert("summary".to_owned(), json!(summary));
            body.insert("messages".to_owned(), json!(checked.messages));
            body.entry("reasons").or_insert(Value::Null);
        }
    }
    Ok(Value::Object(body))
}

fn explain_tool(args: ExplainArgs) -> Outcome {
    let session = Session::load_or_default(None).map_err(fail)?;
    let catalog = session.catalog().map_err(fail)?;
    let registry = session.in_process_registry().map_err(fail)?;
    let markdown = explain(&catalog, &registry, &args.id).map_err(fail)?;
    Ok(json!({ "id": args.id, "markdown": markdown }))
}

fn decision_list(args: DecisionListArgs) -> Outcome {
    let session = Session::load_or_default(None).map_err(fail)?;
    let catalog = session.catalog().map_err(fail)?;
    let registry = session.in_process_registry().map_err(fail)?;
    let enabled = active_rules(&registry, &session.config).map_err(fail)?;
    let rows: Vec<Value> = decision_rows(&catalog, &registry, args.all)
        .into_iter()
        .map(|row| {
            let mut value = serde_json::to_value(&row).unwrap_or(Value::Null);
            value["enabled"] = json!(enabled.contains(&row.id));
            value
        })
        .collect();
    Ok(json!({ "decisions": rows }))
}
