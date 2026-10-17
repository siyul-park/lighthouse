//! `review list|show|resolve|history|prune`: the findings the store remembers
//! and the judgments on them.

use std::fmt::Write;

use lighthouse_model::Fingerprint;
use lighthouse_report::{Entry, GroupOptions, Grouped, Groups};
use lighthouse_session::{
    catalog_at,
    review::{self, FindingRecord, JudgmentEvent, Query, Recorded, Standing},
};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, json};

use crate::surface::{
    Result,
    args::{Detail, FingerprintArgs, ReviewListArgs, ReviewResolveArgs},
    list::{omitted_line, page},
    reply::{Context, Format, Reply},
};

/// Findings of a JSON listing when `--limit` is not given.
const JSON_LIMIT: usize = 50;
/// Said when no check has recorded anything yet.
const NOTHING_RECORDED: &str = "no findings recorded yet: run `check` first";

/// The `ReviewList` kind: the findings that ask for review.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
pub(super) enum ReviewList {
    /// Grouped by decision.
    Compact {
        #[serde(flatten)]
        groups: Groups,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        notices: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<&'static str>,
    },
    /// One record per finding.
    Full {
        tasks: Vec<FindingRecord>,
        /// How many findings `limit` left out.
        omitted: usize,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        notices: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<&'static str>,
    },
}

/// The `JudgmentHistory` kind: every judgment recorded on a finding, oldest
/// first.
#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct JudgmentHistory {
    fingerprint: String,
    events: Vec<JudgmentEvent>,
}

/// `review list`.
pub fn list(ctx: &Context, args: &ReviewListArgs, format: Format) -> Result<Reply> {
    let root = ctx.root()?;
    let query = Query {
        rules: args.decision.clone(),
        status: args.status.into(),
        all_tiers: args.all,
    };
    let Some(tasks) = review::list(&root, &query)? else {
        let nothing = Some(NOTHING_RECORDED);
        let list = match args.detail {
            Detail::Compact => ReviewList::Compact {
                groups: Grouped::of(Vec::new(), &GroupOptions::default()).data(),
                notices: Vec::new(),
                note: nothing,
            },
            Detail::Full => ReviewList::Full {
                tasks: Vec::new(),
                omitted: 0,
                notices: Vec::new(),
                note: nothing,
            },
        };
        let reply = match format {
            Format::Json => Reply::json(&list)?,
            Format::Text => Reply::text(String::new()),
        };
        return Ok(
            reply.with_messages(&["no findings recorded yet (run `lighthouse check`)".into()])
        );
    };
    let reply = match format {
        Format::Text => {
            let shown = page(tasks.findings, args.limit);
            let ids: Vec<&str> = shown.items.iter().map(|f| f.fingerprint.as_str()).collect();
            let mut text: String = (shown.items.iter().zip(prefixes(&ids)))
                .map(|(f, prefix)| row(f, prefix) + "\n")
                .collect();
            text.push_str(&omitted_line(shown.omitted));
            Reply::text(text)
        }
        Format::Json => {
            let limit = args.limit.unwrap_or(JSON_LIMIT);
            let notices = tasks.notices.clone();
            Reply::json(&match args.detail {
                Detail::Compact => {
                    let catalog = catalog_at(&root).ok();
                    let options = GroupOptions {
                        catalog: catalog.as_ref(),
                        limit: Some(limit),
                        tool_form: true,
                    };
                    let entries = tasks.findings.iter().map(entry).collect();
                    ReviewList::Compact {
                        groups: Grouped::of(entries, &options).data(),
                        notices,
                        note: None,
                    }
                }
                Detail::Full => ReviewList::Full {
                    omitted: tasks.findings.len().saturating_sub(limit),
                    tasks: tasks.findings.into_iter().take(limit).collect(),
                    notices,
                    note: None,
                },
            })?
        }
    };
    Ok(reply.with_messages(&tasks.notices))
}

/// `review show`.
pub fn show(ctx: &Context, args: &FingerprintArgs, format: Format) -> Result<Reply> {
    let finding = review::show(&ctx.root()?, &args.fingerprint)?;
    Reply::data(format, &finding, || detail(&finding))
}

/// `review history`.
pub fn history(ctx: &Context, args: &FingerprintArgs, format: Format) -> Result<Reply> {
    let events = review::history(&ctx.root()?, &args.fingerprint)?;
    let notes = if events.is_empty() {
        vec![format!("no judgments recorded for {}", args.fingerprint)]
    } else {
        Vec::new()
    };
    let history = JudgmentHistory {
        fingerprint: args.fingerprint.clone(),
        events,
    };
    let reply = Reply::data(format, &history, || {
        let ids: Vec<&str> = history.events.iter().map(|e| e.id.as_str()).collect();
        let rows = history.events.iter().zip(prefixes(&ids));
        rows.map(|(e, prefix)| event_row(e, prefix) + "\n")
            .collect()
    })?;
    Ok(reply.with_messages(&notes))
}

/// `review prune`.
pub fn prune(ctx: &Context, older_than: Option<u32>) -> Result<Reply> {
    let removed = review::prune(&ctx.root()?, older_than)?;
    Ok(Reply::text(format!(
        "removed {removed} unreviewed finding(s) that are resolved or inactive\n"
    )))
}

/// `review resolve`.
pub fn resolve(ctx: &Context, args: &ReviewResolveArgs, format: Format) -> Result<Reply> {
    let recorded = review::resolve(&ctx.root()?, &args.judgment(ctx)?)?;
    let mut notes = Vec::new();
    if let Some(e) = &recorded.catalog_error {
        notes.push(format!(
            "the catalog cannot be read ({e}); decision and catalog versions are not recorded"
        ));
    }
    notes.extend(recorded.warnings.iter().map(|w| format!("warning: {w}")));
    let reply = Reply::data(format, &recorded, || resolved_text(&recorded))?;
    Ok(reply.with_messages(&notes))
}

fn resolved_text(recorded: &Recorded) -> String {
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
        &event.spec.fingerprint[..Fingerprint::MIN_PREFIX],
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
    let mut attributes = Map::new();
    attributes.insert("seen".to_owned(), json!(f.last_seen));
    let state = f.state.to_string();
    if state != "open" {
        attributes.insert("state".to_owned(), json!(state));
    }
    if let Some(judgment) = f.judgment {
        attributes.insert("judgment".to_owned(), json!(judgment));
    }
    Entry {
        decision: f.decision.clone(),
        severity: f.severity,
        authored: f.authored_severity,
        review: f.needs_review(),
        path: f.path.clone(),
        line: f.line(),
        col: f.col(),
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

/// One line per finding: fingerprint, decision, severity, location, state,
/// latest review and message, tab-separated.
fn row(finding: &FindingRecord, fingerprint: &str) -> String {
    format!(
        "{}\t{}\t{}\t{}:{}\t{}\t{}\t{}",
        fingerprint,
        finding.decision,
        finding.severity,
        finding.path,
        finding.line(),
        finding.state,
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
        "decision:",
        format!("{} ({}, {tier})", finding.decision, finding.severity),
    );
    field("location:", format!("{}:{}", finding.path, finding.line()));
    if let Some(symbol) = &finding.symbol {
        field("owner:", symbol.clone());
    }
    field("state:", finding.state.to_string());
    field("first seen:", finding.first_seen.clone());
    field("last seen:", finding.last_seen.clone());
    if let Some(resolved) = &finding.resolved_at {
        field(
            "resolved:",
            format!("{resolved} (reopened {} time(s))", finding.reopened),
        );
    }
    if let Some(inactive) = &finding.inactive_at {
        field(
            "inactive:",
            format!("{inactive} (its decision is not enabled)"),
        );
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

fn event_row(event: &JudgmentEvent, id: &str) -> String {
    let suppression = event.suppressions.first().map_or("-".to_owned(), |s| {
        format!("{}:{}", s.spec.kind, s.spec.justification)
    });
    format!(
        "{}\t{}\t{}\t{}\t{}:{}\t{}\t{}",
        id,
        event.spec.generated_at_time,
        event.spec.judgment,
        suppression,
        event.spec.was_attributed_to.kind,
        event.spec.was_attributed_to.id.as_deref().unwrap_or("-"),
        event.label(),
        event.spec.reason.as_deref().unwrap_or("")
    )
}

/// Each id cut to the width that tells it apart from the others listed.
fn prefixes<'a>(ids: &[&'a str]) -> Vec<&'a str> {
    let widths = Fingerprint::prefix_widths(ids);
    (ids.iter().zip(widths))
        .map(|(id, width)| id.get(..width).unwrap_or(id))
        .collect()
}
