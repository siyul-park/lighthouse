//! The review tools: what waits for review, recording judgments, history.

use lighthouse_report::{Detail, Entry, GroupOptions, Grouped};
use lighthouse_session::{
    AgentKind, Attribution, FindingRecord, Judgment, JudgmentEvent, NewJudgment, StatusFilter,
    TaskQuery, catalog_at, head, project_root, record_judgment, review_history, review_tasks,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::tools::{Outcome, fail};

const DEFAULT_TASKS: usize = 50;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TasksArgs {
    status: Option<String>,
    rule: Option<String>,
    tier: Option<String>,
    limit: Option<usize>,
    detail: Option<Detail>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveArgs {
    fingerprint: String,
    judgment: String,
    suppress: Option<Suppress>,
    reason: Option<String>,
    seen: Option<String>,
}

/// Leaves a `fail` in place on purpose: a SARIF `external` suppression.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suppress {
    justification: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryArgs {
    fingerprint: String,
}

pub fn tasks(args: TasksArgs) -> Outcome {
    let status = match args.status.as_deref().unwrap_or("open") {
        "open" => StatusFilter::Open,
        "suppressed" => StatusFilter::Suppressed,
        "narrowing" => StatusFilter::Narrowing,
        "inactive" => StatusFilter::Inactive,
        "resolved" => StatusFilter::Resolved,
        "all" => StatusFilter::All,
        other => return Err(format!("unknown status `{other}`")),
    };
    let all_tiers = match args.tier.as_deref().unwrap_or("review") {
        "review" => false,
        "all" => true,
        other => return Err(format!("unknown tier `{other}` (review or all)")),
    };
    let root = project_root().map_err(fail)?;
    let detail = args.detail.unwrap_or_default();
    let query = TaskQuery {
        rule: args.rule,
        status,
        all_tiers,
    };
    let Some(tasks) = review_tasks(&root, &query).map_err(fail)? else {
        let empty = match detail {
            Detail::Compact => "groups",
            Detail::Full => "tasks",
        };
        return Ok(json!({ empty: [], "note": "no findings recorded yet: run `check` first" }));
    };
    let notices = tasks.notices.clone();
    let wanted: Vec<&FindingRecord> = tasks.findings.iter().collect();
    let limit = args.limit.unwrap_or(DEFAULT_TASKS);
    if detail == Detail::Compact {
        let entries = wanted.iter().map(|f| entry(f)).collect();
        let catalog = catalog_at(&root).ok();
        let options = GroupOptions {
            catalog: catalog.as_ref(),
            limit: Some(limit),
            mcp: true,
        };
        let grouped = Grouped::of(entries, &options);
        let mut fields = grouped.fields();
        if !tasks.notices.is_empty() {
            fields.insert("notices".to_owned(), json!(tasks.notices));
        }
        return Ok(Value::Object(fields));
    }
    let tasks: Vec<Value> = wanted.iter().take(limit).map(|f| task(f)).collect();
    Ok(json!({
        "tasks": tasks,
        "omitted": wanted.len().saturating_sub(limit),
        "notices": notices,
    }))
}

pub fn resolve(args: ResolveArgs, reviewer: &str) -> Outcome {
    let judgment: Judgment = args.judgment.parse().map_err(fail)?;
    let root = project_root().map_err(fail)?;
    let review = NewJudgment {
        fingerprint: args.fingerprint,
        judgment,
        reason: args.reason,
        suppress: args.suppress.map(|s| s.justification),
        attribution: Attribution {
            kind: AgentKind::SoftwareAgent,
            id: Some(reviewer.to_owned()),
        },
        commit: head(&root),
        expect_seen: args.seen,
        lighthouse_version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let recorded = record_judgment(&root, &review).map_err(fail)?;
    let event = &recorded.event;
    Ok(json!({
        "recorded": {
            "fingerprint": event.fingerprint,
            "decision": event.decision_name,
            "judgment": event.judgment.to_string(),
            "suppression": event.suppressions.first().map(|s| json!({
                "kind": s.kind, "status": s.status, "justification": s.justification,
            })),
            "wasAttributedTo": event.was_attributed_to,
        },
        "standing": recorded.standing,
        "warnings": recorded.warnings,
        "catalogError": recorded.catalog_error,
    }))
}

pub fn history(args: HistoryArgs) -> Outcome {
    let root = project_root().map_err(fail)?;
    let events = review_history(&root, &args.fingerprint).map_err(fail)?;
    let events: Vec<Value> = events.iter().map(JudgmentEvent::to_json).collect();
    Ok(json!({ "fingerprint": args.fingerprint, "events": events }))
}

/// A remembered finding as a compact entry: where the finding is in its life
/// (`state` unless open, the latest `judgment`) and its `seen` time, which
/// `review_resolve` takes, ride along as evidence.
fn entry(f: &FindingRecord) -> Entry<'_> {
    let severity = f.severity;
    let authored = f.authored_severity;
    let start = &f.locator["span"]["start"];
    let position = |key: &str| start[key].as_u64().map_or(0, |n| n as u32);
    let mut attributes = Map::new();
    attributes.insert("seen".to_owned(), json!(f.last_seen));
    let state = f.state().to_string();
    if state != "open" {
        attributes.insert("state".to_owned(), json!(state));
    }
    if let Some(judgment) = f.judgment {
        attributes.insert("judgment".to_owned(), json!(judgment));
    }
    Entry {
        rule: f.rule_id.clone(),
        severity,
        authored,
        review: f.needs_review(),
        path: f.path.clone(),
        line: position("line"),
        col: position("col"),
        message: f.message.clone(),
        symbol: f.symbol.clone(),
        evidence: f.evidence.clone(),
        attributes,
        note: None,
        fingerprint: f.fingerprint.clone(),
        facts: Some(f.facts.clone()),
        fix: None,
    }
}

fn task(f: &FindingRecord) -> Value {
    json!({
        "fingerprint": f.fingerprint,
        "rule": f.rule_id,
        "severity": f.severity,
        "authored": f.authored_severity,
        "state": f.state().to_string(),
        "location": { "path": f.path, "line": f.locator["span"]["start"]["line"] },
        "symbol": f.symbol,
        "message": f.message,
        "evidence": f.evidence,
        "lastSeen": f.last_seen,
        "judgment": f.judgment,
        "standing": f.standing,
        "justification": f.justification,
    })
}
