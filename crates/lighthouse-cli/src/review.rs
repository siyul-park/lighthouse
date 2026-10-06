//! `lighthouse review`: the findings the store remembers and the verdicts on
//! them.

use std::{env, fmt::Write, path::Path};

use clap::{Subcommand, ValueEnum};
use lighthouse_model::{Reason, ReviewerKind, Verdict};
use lighthouse_spec::{Catalog, Pattern};
use lighthouse_store::{
    Filter, FindingRecord, NewReview, ReviewEvent, Stamp, Standing, State, StatusFilter, Store,
};
use serde_json::{Value, json};

use crate::{
    Result, git,
    session::{catalog_at, project_root},
};

/// Characters of a fingerprint that listings show; any unambiguous prefix
/// names a finding.
const SHORT: usize = 12;
/// Environment variables that say who is reviewing when no flag does. Tools
/// that run reviews for an agent (hooks, an MCP server) set both.
const KIND_VAR: &str = "LIGHTHOUSE_REVIEWER_KIND";
const ID_VAR: &str = "LIGHTHOUSE_REVIEWER";

#[derive(Subcommand)]
pub enum ReviewCommand {
    /// List the findings the store remembers.
    List {
        /// Only this fully qualified rule id.
        #[arg(long)]
        rule: Option<String>,
        /// `open`: still reported and not suppressed. `suppressed`: kept out
        /// of reports by a rejected verdict. `narrowing`: suppressed because
        /// the rule is too broad. `inactive`: of a rule the configuration no
        /// longer enables. `resolved`: no longer reported by a complete run.
        /// `all`: every remembered finding.
        #[arg(long, value_enum, default_value = "open")]
        status: Status,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    /// Show one finding: where it is, its evidence and its latest review.
    Show {
        /// A fingerprint, or the start of one.
        fingerprint: String,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    /// Record a verdict on a finding.
    ///
    /// confirmed (reasons: fixed, accepted-debt) means the finding is right.
    /// rejected (false-positive, intentional-exception, scope-too-broad,
    /// project-allowed, not-worth-fixing) keeps the finding out of later
    /// reports while its rule and evidence stay as they are; mechanical
    /// findings (severity error) are recorded but never suppressed. deferred
    /// (no reason) leaves it visible. Verdicts are appended to
    /// .lighthouse/decisions.jsonl, which is meant to be committed, and never
    /// edited: a later one replaces the standing of the finding.
    ///
    /// The reviewer is --reviewer-kind, else $LIGHTHOUSE_REVIEWER_KIND, else
    /// human; the id is --reviewer-id, else $LIGHTHOUSE_REVIEWER, else $USER.
    Resolve {
        /// A fingerprint, or the start of one.
        fingerprint: String,
        #[arg(long)]
        verdict: Verdict,
        /// Required for rejected; confirmed may carry one; deferred takes none.
        #[arg(long, default_value = "none")]
        reason: Reason,
        /// Free-text explanation, kept with the verdict.
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        reviewer_kind: Option<ReviewerKind>,
        #[arg(long)]
        reviewer_id: Option<String>,
        /// The `last seen` time of the finding as you read it; refuses the
        /// verdict if the finding has been seen again since.
        #[arg(long, value_name = "LAST_SEEN")]
        seen: Option<String>,
    },
    /// Show every verdict recorded on a finding, oldest first.
    History {
        /// A fingerprint, or the start of one.
        fingerprint: String,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    /// Delete resolved and inactive findings that nobody reviewed. Findings
    /// with verdicts stay, because the verdicts are labels.
    Prune {
        /// Only those resolved or inactive for at least this many days.
        #[arg(long, value_name = "DAYS")]
        older_than: Option<u32>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Status {
    Open,
    Suppressed,
    Narrowing,
    Inactive,
    Resolved,
    All,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Text,
    Json,
}

/// Who is reviewing: the flag, else the environment, else a human.
struct Reviewer {
    kind: ReviewerKind,
    id: Option<String>,
}

pub fn run(command: ReviewCommand) -> Result<u8> {
    let root = project_root()?;
    match command {
        ReviewCommand::List {
            rule,
            status,
            format,
        } => match Store::open_existing(&root)? {
            Some(store) => list(&store, rule, status, format),
            None => {
                eprintln!("lighthouse: no findings recorded yet (run `lighthouse check`)");
                Ok(0)
            }
        },
        ReviewCommand::Show {
            fingerprint,
            format,
        } => show(&existing(&root)?, &fingerprint, format),
        ReviewCommand::History {
            fingerprint,
            format,
        } => history(&existing(&root)?, &fingerprint, format),
        ReviewCommand::Prune { older_than } => prune(&mut existing(&root)?, older_than),
        ReviewCommand::Resolve {
            fingerprint,
            verdict,
            reason,
            note,
            reviewer_kind,
            reviewer_id,
            seen,
        } => {
            let reviewer = reviewer(reviewer_kind, reviewer_id)?;
            let review = NewReview {
                fingerprint,
                verdict,
                reason,
                reason_text: note,
                reviewer_kind: reviewer.kind,
                reviewer_id: reviewer.id,
                commit: git::head(&root),
                expect_seen: seen,
                lighthouse_version: env!("CARGO_PKG_VERSION").to_owned(),
            };
            resolve(&root, &mut existing(&root)?, &review)
        }
    }
}

/// The store, which must already exist: nothing is remembered before a check.
fn existing(root: &Path) -> Result<Store> {
    Store::open_existing(root)?
        .ok_or_else(|| "no findings recorded yet (run `lighthouse check`)".into())
}

fn reviewer(kind: Option<ReviewerKind>, id: Option<String>) -> Result<Reviewer> {
    let kind = match (kind, env::var(KIND_VAR)) {
        (Some(kind), _) => kind,
        (None, Ok(text)) => text.parse().map_err(|e| format!("{KIND_VAR}: {e}"))?,
        (None, Err(_)) => ReviewerKind::Human,
    };
    let id = id
        .or_else(|| env::var(ID_VAR).ok())
        .or_else(|| env::var("USER").ok())
        .filter(|id| !id.is_empty());
    Ok(Reviewer { kind, id })
}

fn list(store: &Store, rule: Option<String>, status: Status, format: Format) -> Result<u8> {
    let status = match status {
        Status::Open => StatusFilter::Open,
        Status::Suppressed => StatusFilter::Suppressed,
        Status::Narrowing => StatusFilter::Narrowing,
        Status::Inactive => StatusFilter::Inactive,
        Status::Resolved => StatusFilter::Resolved,
        Status::All => StatusFilter::All,
    };
    for finding in store.list(&Filter { rule, status })? {
        match format {
            Format::Text => println!("{}", row(&finding)),
            Format::Json => println!("{}", serde_json::to_string(&finding)?),
        }
    }
    Ok(0)
}

fn show(store: &Store, fingerprint: &str, format: Format) -> Result<u8> {
    let finding = store.finding(fingerprint)?;
    match format {
        Format::Text => print!("{}", detail(&finding)),
        Format::Json => println!("{}", serde_json::to_string(&finding)?),
    }
    Ok(0)
}

fn history(store: &Store, fingerprint: &str, format: Format) -> Result<u8> {
    let events = store.history(fingerprint)?;
    if events.is_empty() {
        eprintln!("lighthouse: no reviews recorded for {fingerprint}");
    }
    for event in events {
        match format {
            Format::Text => println!("{}", event_row(&event)),
            Format::Json => println!("{}", event_json(&event)?),
        }
    }
    Ok(0)
}

fn prune(store: &mut Store, older_than: Option<u32>) -> Result<u8> {
    let removed = store.prune(older_than)?;
    println!("removed {removed} unreviewed finding(s) that are resolved or inactive");
    Ok(0)
}

/// Records the verdict, stamping it with the versions of the finding's rule
/// when the catalog can be read; a broken catalog is a warning, not a failure.
fn resolve(root: &Path, store: &mut Store, review: &NewReview) -> Result<u8> {
    let catalog = catalog_at(root)
        .inspect_err(|e| {
            eprintln!(
                "lighthouse: the catalog cannot be read ({e}); rule and catalog versions are not recorded"
            );
        })
        .ok();
    let resolved = store.resolve(review, |finding| stamp(catalog.as_ref(), finding))?;
    let (event, finding) = (&resolved.event, &resolved.finding);
    println!(
        "recorded {} ({}) for {} {}",
        event.verdict,
        event.reason,
        short(&event.fingerprint),
        event.rule_id
    );
    for warning in warnings(finding) {
        eprintln!("lighthouse: warning: {warning}");
    }
    match store
        .standings()?
        .get(&event.fingerprint)
        .map(|j| j.standing)
    {
        Some(Standing::Suppressed) => println!(
            "later checks keep this finding out of the report while its rule and evidence stay as they are; `lighthouse review list --status suppressed` lists it"
        ),
        Some(Standing::Unsuppressible) => println!(
            "this is a mechanical finding: the verdict is recorded but the finding stays reported; fix the rule"
        ),
        _ => {}
    }
    Ok(0)
}

fn stamp(catalog: Option<&Catalog>, finding: &FindingRecord) -> Stamp {
    let Some(catalog) = catalog else {
        return Stamp::default();
    };
    let pattern = catalog.pattern(&finding.rule_id);
    Stamp {
        rule_version: pattern.map(Pattern::semantic_version),
        pattern_hash: pattern.map(Pattern::version),
        catalog_version: Some(catalog.version()),
        scope: pattern.map(|p| p.scope.to_string()),
    }
}

/// What the reviewer should know about the finding they just judged.
fn warnings(finding: &FindingRecord) -> Vec<String> {
    let mut warnings = Vec::new();
    match finding.state() {
        State::Resolved => warnings.push(
            "the finding was already resolved: no complete run reports it any more, so the verdict only matters if it comes back".to_owned(),
        ),
        State::Inactive => warnings.push(
            "the finding's rule is no longer enabled by the configuration".to_owned(),
        ),
        State::Open | State::Suppressed => {}
    }
    if finding.facts.get("ordinal") == Some(&Value::Bool(true)) {
        warnings.push(
            "this finding's identity rests on its position among identical findings, so the verdict may stop matching when one of them is added or removed".to_owned(),
        );
    }
    warnings
}

/// One line per finding: fingerprint, rule, severity, location, state, latest
/// review and message, tab-separated.
fn row(finding: &FindingRecord) -> String {
    format!(
        "{}\t{}\t{}\t{}:{}\t{}\t{}\t{}",
        short(&finding.fingerprint),
        finding.rule_id,
        finding.severity,
        finding.path,
        line(&finding.locator),
        finding.state(),
        review_text(finding),
        finding.message
    )
}

/// The latest verdict and, when it no longer applies, why.
fn review_text(finding: &FindingRecord) -> String {
    let Some(review) = finding.review else {
        return "-".to_owned();
    };
    let base = format!("{}:{}", review.verdict, review.reason);
    match finding.standing {
        Some(Standing::RuleChanged) => format!("{base} (expired: rule changed)"),
        Some(Standing::EvidenceChanged) => format!("{base} (expired: evidence changed)"),
        Some(Standing::Unsuppressible) => format!("{base} (mechanical: not suppressed)"),
        _ if finding.narrowing => format!("{base} (narrow the rule)"),
        _ => base,
    }
}

fn detail(finding: &FindingRecord) -> String {
    let mut out = String::new();
    let mut field = |label: &str, value: String| {
        let _ = writeln!(out, "{label:<13}{value}");
    };
    field("fingerprint:", finding.fingerprint.clone());
    let tier = finding.tier.as_deref().unwrap_or("-");
    field(
        "rule:",
        format!("{} ({}, {tier})", finding.rule_id, finding.severity),
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
    if finding.review.is_some() {
        field("review:", review_text(finding));
    }
    out
}

fn event_row(event: &ReviewEvent) -> String {
    format!(
        "{}\t{}\t{}:{}\t{}:{}\t{}\t{}",
        short(&event.id),
        event.timestamp,
        event.verdict,
        event.reason,
        event.reviewer_kind,
        event.reviewer_id.as_deref().unwrap_or("-"),
        event.label(),
        event.reason_text.as_deref().unwrap_or("")
    )
}

fn event_json(event: &ReviewEvent) -> Result<Value> {
    let mut value = serde_json::to_value(event)?;
    value["label"] = json!(event.label());
    Ok(value)
}

fn line(locator: &Value) -> u64 {
    locator["span"]["start"]["line"].as_u64().unwrap_or(0)
}

fn short(fingerprint: &str) -> &str {
    fingerprint.get(..SHORT).unwrap_or(fingerprint)
}
