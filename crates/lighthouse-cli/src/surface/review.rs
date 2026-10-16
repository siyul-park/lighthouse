//! `review list|show|resolve|history|prune`: the findings the store remembers
//! and the judgments on them.

use std::fmt::Write;

use lighthouse_report::{Entry, GroupOptions, Grouped};
use lighthouse_session::{
    FindingRecord, JudgmentEvent, NewJudgment, Standing, TaskQuery, catalog_at, head, long_enough,
    project_root, record_judgment, review_finding, review_history, review_prune, review_tasks,
};
use serde_json::{Map, Value, json};

use crate::surface::{
    Result,
    args::{Detail, FingerprintArgs, ReviewListArgs, ReviewResolveArgs},
    list::{omitted_line, page},
    reply::{Context, Output, Reply},
};

/// Findings of a JSON listing when `--limit` is not given.
const JSON_LIMIT: usize = 50;
/// Characters of a fingerprint that text listings show; any unique prefix of
/// at least seven names a finding.
const SHORT: usize = 12;

/// `review list`.
pub fn list(args: &ReviewListArgs, output: Output) -> Result<Reply> {
    let root = project_root()?;
    let query = TaskQuery {
        rules: args.decision.clone(),
        status: args.status.into(),
        all_tiers: args.all,
    };
    let Some(tasks) = review_tasks(&root, &query)? else {
        let empty = match args.detail {
            Detail::Compact => "groups",
            Detail::Full => "tasks",
        };
        let value = json!({ empty: [], "note": "no findings recorded yet: run `check` first" });
        let notes =
            vec!["lighthouse: no findings recorded yet (run `lighthouse check`)".to_owned()];
        let reply = match output {
            Output::Json => Reply::json(value)?,
            Output::Text => Reply::default(),
        };
        return Ok(reply.with_notes(notes));
    };
    let notes: Vec<String> = tasks
        .notices
        .iter()
        .map(|n| format!("lighthouse: {n}"))
        .collect();
    let reply = match output {
        Output::Text => {
            let shown = page(tasks.findings, args.limit);
            let mut text: String = shown.items.iter().map(|f| row(f) + "\n").collect();
            text.push_str(&omitted_line(shown.omitted));
            Reply::text(text)
        }
        Output::Json => Reply::json(json_list(&root, &tasks.findings, &tasks.notices, args))?,
    };
    Ok(reply.with_notes(notes))
}

/// `review show`.
pub fn show(args: &FingerprintArgs, output: Output) -> Result<Reply> {
    long_enough(&args.fingerprint)?;
    let finding = review_finding(&project_root()?, &args.fingerprint)?;
    Reply::data(output, &finding, || detail(&finding))
}

/// `review history`.
pub fn history(args: &FingerprintArgs, output: Output) -> Result<Reply> {
    long_enough(&args.fingerprint)?;
    let events = review_history(&project_root()?, &args.fingerprint)?;
    let notes = if events.is_empty() {
        vec![format!(
            "lighthouse: no judgments recorded for {}",
            args.fingerprint
        )]
    } else {
        Vec::new()
    };
    let value = json!({
        "fingerprint": args.fingerprint,
        "events": events.iter().map(JudgmentEvent::to_json).collect::<Vec<_>>(),
    });
    let reply = match output {
        Output::Json => Reply::json(value)?,
        Output::Text => Reply::text(events.iter().map(|e| event_row(e) + "\n").collect()),
    };
    Ok(reply.with_notes(notes))
}

/// `review prune`.
pub fn prune(older_than: Option<u32>) -> Result<Reply> {
    let removed = review_prune(&project_root()?, older_than)?;
    Ok(Reply::text(format!(
        "removed {removed} unreviewed finding(s) that are resolved or inactive\n"
    )))
}

/// `review resolve`.
pub fn resolve(ctx: &Context, args: &ReviewResolveArgs, output: Output) -> Result<Reply> {
    long_enough(&args.fingerprint)?;
    let root = project_root()?;
    let review = NewJudgment {
        fingerprint: args.fingerprint.clone(),
        judgment: args.judgment.into(),
        reason: args.reason.clone(),
        suppress: args.suppress.clone(),
        attribution: ctx.attribution(&root, args.attributed_to.as_deref())?,
        commit: head(&root),
        expect_seen: args.seen.clone(),
        lighthouse_version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let recorded = record_judgment(&root, &review)?;
    let event = &recorded.event;
    let value = json!({
        "recorded": {
            "fingerprint": event.spec.fingerprint,
            "decision": event.spec.decision,
            "judgment": event.spec.judgment.to_string(),
            "suppression": event.suppressions.first().map(|s| json!({
                "kind": s.spec.kind, "status": s.spec.status, "justification": s.spec.justification,
            })),
            "wasAttributedTo": event.spec.was_attributed_to,
        },
        "standing": recorded.standing,
        "warnings": recorded.warnings,
        "catalogError": recorded.catalog_error,
    });
    let mut notes = Vec::new();
    if let Some(e) = &recorded.catalog_error {
        notes.push(format!(
            "lighthouse: the catalog cannot be read ({e}); decision and catalog versions are not recorded"
        ));
    }
    notes.extend(
        recorded
            .warnings
            .iter()
            .map(|w| format!("lighthouse: warning: {w}")),
    );
    let reply = match output {
        Output::Json => Reply::json(value)?,
        Output::Text => Reply::text(resolved_text(&recorded)),
    };
    Ok(reply.with_notes(notes))
}

/// The `ReviewList` kind.
fn json_list(
    root: &std::path::Path,
    findings: &[FindingRecord],
    notices: &[String],
    args: &ReviewListArgs,
) -> Value {
    let limit = args.limit.unwrap_or(JSON_LIMIT);
    if args.detail == Detail::Compact {
        let catalog = catalog_at(root).ok();
        let options = GroupOptions {
            catalog: catalog.as_ref(),
            limit: Some(limit),
            tool_form: true,
        };
        let mut fields = Grouped::of(findings.iter().map(entry).collect(), &options).fields();
        if !notices.is_empty() {
            fields.insert("notices".to_owned(), json!(notices));
        }
        return Value::Object(fields);
    }
    let tasks: Vec<Value> = findings.iter().take(limit).map(task).collect();
    json!({
        "tasks": tasks,
        "omitted": findings.len().saturating_sub(limit),
        "notices": notices,
    })
}

fn resolved_text(recorded: &lighthouse_session::Recorded) -> String {
    let event = &recorded.event;
    let suppressed = event
        .suppressions
        .first()
        .map(|s| {
            format!(
                " and an {} suppression ({})",
                s.spec.kind, s.spec.justification
            )
        })
        .unwrap_or_default();
    let mut out = format!(
        "recorded {}{suppressed} for {} {}\n",
        event.spec.judgment,
        short(&event.spec.fingerprint),
        event.spec.decision
    );
    match recorded.standing {
        Some(Standing::Suppressed) => out.push_str(
            "later checks keep this finding out of the report while its decision and evidence stay as they are; `lighthouse review list --status suppressed` lists it\n",
        ),
        Some(Standing::Unsuppressible) => out.push_str(
            "this is an error: the judgment is recorded but the finding stays reported; fix the code or suppress it in the code\n",
        ),
        _ => {}
    }
    out
}

/// A remembered finding as a compact entry: where the finding is in its life
/// (`state` unless open, the latest `judgment`) and its `seen` time, which
/// `review resolve` takes, ride along as evidence.
fn entry(f: &FindingRecord) -> Entry<'_> {
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
        rule: f.decision.clone(),
        severity: f.severity,
        authored: f.authored_severity,
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
        "decision": f.decision,
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

/// One line per finding: fingerprint, rule, severity, location, state, latest
/// review and message, tab-separated.
fn row(finding: &FindingRecord) -> String {
    format!(
        "{}\t{}\t{}\t{}:{}\t{}\t{}\t{}",
        short(&finding.fingerprint),
        finding.decision,
        finding.severity,
        finding.path,
        line(&finding.locator),
        finding.state(),
        review_text(finding),
        finding.message
    )
}

/// The latest judgment and, when it no longer applies, why.
fn review_text(finding: &FindingRecord) -> String {
    let Some(judgment) = finding.judgment else {
        return "-".to_owned();
    };
    let base = match &finding.justification {
        Some(justification) => format!("{judgment}:{justification}"),
        None => judgment.to_string(),
    };
    match finding.standing {
        Some(Standing::RuleChanged) => format!("{base} (expired: decision changed)"),
        Some(Standing::EvidenceChanged) => format!("{base} (expired: evidence changed)"),
        Some(Standing::Unsuppressible) => format!("{base} (error: not suppressed)"),
        _ if finding.narrowing => format!("{base} (narrow the decision)"),
        _ => base,
    }
}

fn detail(finding: &FindingRecord) -> String {
    let mut out = String::new();
    let mut field = |label: &str, value: String| {
        let _ = writeln!(out, "{label:<13}{value}");
    };
    field("fingerprint:", finding.fingerprint.clone());
    let tier = finding.authored_severity;
    field(
        "rule:",
        format!("{} ({}, {tier})", finding.decision, finding.severity),
    );
    field(
        "location:",
        format!("{}:{}", finding.path, line(&finding.locator)),
    );
    if let Some(symbol) = &finding.symbol {
        field("owner:", symbol.clone());
    }
    field("state:", finding.state().to_string());
    field("first seen:", finding.first_seen.clone());
    field("last seen:", finding.last_seen.clone());
    if let Some(resolved) = &finding.resolved_at {
        field(
            "resolved:",
            format!("{resolved} (reopened {} time(s))", finding.reopened),
        );
    }
    if let Some(inactive) = &finding.inactive_at {
        field("inactive:", format!("{inactive} (its rule is not enabled)"));
    }
    field("message:", finding.message.clone());
    if let Some(evidence) = finding.evidence.as_object().filter(|e| !e.is_empty()) {
        let pairs: Vec<String> = evidence.iter().map(|(k, v)| format!("{k}={v}")).collect();
        field("evidence:", pairs.join(" "));
    }
    if finding.judgment.is_some() {
        field("judgment:", review_text(finding));
    }
    out
}

fn event_row(event: &JudgmentEvent) -> String {
    let suppression = event.suppressions.first().map_or("-".to_owned(), |s| {
        format!("{}:{}", s.spec.kind, s.spec.justification)
    });
    format!(
        "{}\t{}\t{}\t{}\t{}:{}\t{}\t{}",
        short(&event.id),
        event.spec.generated_at_time,
        event.spec.judgment,
        suppression,
        event.spec.was_attributed_to.kind,
        event.spec.was_attributed_to.id.as_deref().unwrap_or("-"),
        event.label(),
        event.spec.reason.as_deref().unwrap_or("")
    )
}

fn line(locator: &Value) -> u64 {
    locator["span"]["start"]["line"].as_u64().unwrap_or(0)
}

fn short(fingerprint: &str) -> &str {
    fingerprint.get(..SHORT).unwrap_or(fingerprint)
}
