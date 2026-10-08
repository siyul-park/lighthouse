//! The review tools: what waits for a judgment, recording verdicts, history.

use lighthouse_model::{Reason, ReviewerKind, Verdict};
use lighthouse_session::{existing_store, head, project_root, record_verdict};
use lighthouse_store::{Filter, FindingRecord, NewReview, ReviewEvent, StatusFilter, Store};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::tools::{Outcome, fail};

const DEFAULT_TASKS: usize = 50;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TasksArgs {
    status: Option<String>,
    rule: Option<String>,
    tier: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveArgs {
    fingerprint: String,
    verdict: String,
    reason: Option<String>,
    note: Option<String>,
    seen: Option<String>,
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
    let Some(store) = Store::open_existing(&root).map_err(fail)? else {
        return Ok(json!({ "tasks": [], "note": "no findings recorded yet: run `check` first" }));
    };
    let found = store
        .list(&Filter {
            rule: args.rule,
            status,
        })
        .map_err(fail)?;
    let wanted: Vec<&FindingRecord> = found
        .iter()
        .filter(|f| all_tiers || f.needs_verdict())
        .collect();
    let limit = args.limit.unwrap_or(DEFAULT_TASKS);
    let tasks: Vec<Value> = wanted.iter().take(limit).map(|f| task(f)).collect();
    Ok(json!({
        "tasks": tasks,
        "omitted": wanted.len().saturating_sub(limit),
    }))
}

pub fn resolve(args: ResolveArgs, reviewer: &str) -> Outcome {
    let verdict: Verdict = args.verdict.parse().map_err(fail)?;
    let reason: Reason = match args.reason {
        Some(text) => text.parse().map_err(fail)?,
        None => Reason::Unspecified,
    };
    let root = project_root().map_err(fail)?;
    let review = NewReview {
        fingerprint: args.fingerprint,
        verdict,
        reason,
        reason_text: args.note,
        reviewer_kind: ReviewerKind::Agent,
        reviewer_id: Some(reviewer.to_owned()),
        commit: head(&root),
        expect_seen: args.seen,
        lighthouse_version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let mut store = existing_store(&root).map_err(fail)?;
    let recorded = record_verdict(&root, &mut store, &review).map_err(fail)?;
    let event = &recorded.event;
    Ok(json!({
        "recorded": {
            "fingerprint": event.fingerprint,
            "rule": event.rule_id,
            "verdict": event.verdict.to_string(),
            "reason": event.reason.to_string(),
            "reviewer": format!("{}:{}", event.reviewer_kind, reviewer),
        },
        "standing": recorded.standing,
        "warnings": recorded.warnings,
        "catalogError": recorded.catalog_error,
    }))
}

pub fn history(args: HistoryArgs) -> Outcome {
    let root = project_root().map_err(fail)?;
    let store = existing_store(&root).map_err(fail)?;
    let events = store.history(&args.fingerprint).map_err(fail)?;
    let events: Vec<Value> = events.iter().map(ReviewEvent::to_json).collect();
    Ok(json!({ "fingerprint": args.fingerprint, "events": events }))
}

fn task(f: &FindingRecord) -> Value {
    json!({
        "fingerprint": f.fingerprint,
        "rule": f.rule_id,
        "severity": f.severity,
        "tier": f.tier,
        "state": f.state().to_string(),
        "location": { "path": f.path, "line": f.locator["span"]["start"]["line"] },
        "symbol": f.symbol,
        "message": f.message,
        "evidence": f.evidence,
        "lastSeen": f.last_seen,
        "review": f.review.map(|r| json!({ "verdict": r.verdict.to_string(), "reason": r.reason.to_string() })),
    })
}
