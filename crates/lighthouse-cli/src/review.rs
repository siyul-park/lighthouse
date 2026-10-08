//! `lighthouse review`: the findings the store remembers and the verdicts on
//! them.

use std::{fmt::Write, path::Path};

use clap::{Subcommand, ValueEnum};
use lighthouse_model::{Reason, ReviewerKind, Verdict};
use lighthouse_session::{Recorded, Reviewer, existing_store, head, project_root, record_verdict};
use lighthouse_store::{
    Filter, FindingRecord, NewReview, ReviewEvent, Standing, StatusFilter, Store,
};
use serde_json::Value;

use crate::Result;

/// Characters of a fingerprint that listings show; any unambiguous prefix
/// names a finding.
const SHORT: usize = 12;

#[derive(Subcommand)]
pub enum ReviewCommand {
    /// List the findings that ask for a verdict: those of heuristic and
    /// judgment decisions, whatever their severity.
    List {
        /// Also list findings that do not ask for a verdict (mechanical
        /// decisions).
        #[arg(long)]
        all: bool,
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
    /// findings (decided by their check, at any severity) are recorded but never suppressed. deferred
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

pub fn run(command: ReviewCommand) -> Result<u8> {
    let root = project_root()?;
    match command {
        ReviewCommand::List {
            all,
            rule,
            status,
            format,
        } => match Store::open_existing(&root)? {
            Some(store) => {
                store
                    .notices()
                    .iter()
                    .for_each(|n| eprintln!("lighthouse: {n}"));
                list(&store, rule, status, all, format)
            }
            None => {
                eprintln!("lighthouse: no findings recorded yet (run `lighthouse check`)");
                Ok(0)
            }
        },
        ReviewCommand::Show {
            fingerprint,
            format,
        } => show(&existing_store(&root)?, &fingerprint, format),
        ReviewCommand::History {
            fingerprint,
            format,
        } => history(&existing_store(&root)?, &fingerprint, format),
        ReviewCommand::Prune { older_than } => prune(&mut existing_store(&root)?, older_than),
        ReviewCommand::Resolve {
            fingerprint,
            verdict,
            reason,
            note,
            reviewer_kind,
            reviewer_id,
            seen,
        } => {
            let reviewer = Reviewer::from_env(reviewer_kind, reviewer_id)?;
            let review = NewReview {
                fingerprint,
                verdict,
                reason,
                reason_text: note,
                reviewer_kind: reviewer.kind,
                reviewer_id: reviewer.id,
                commit: head(&root),
                expect_seen: seen,
                lighthouse_version: env!("CARGO_PKG_VERSION").to_owned(),
            };
            resolve(&root, &mut existing_store(&root)?, &review)
        }
    }
}

fn list(
    store: &Store,
    rule: Option<String>,
    status: Status,
    all: bool,
    format: Format,
) -> Result<u8> {
    let status = match status {
        Status::Open => StatusFilter::Open,
        Status::Suppressed => StatusFilter::Suppressed,
        Status::Narrowing => StatusFilter::Narrowing,
        Status::Inactive => StatusFilter::Inactive,
        Status::Resolved => StatusFilter::Resolved,
        Status::All => StatusFilter::All,
    };
    let findings = store.list(&Filter { rule, status })?;
    for finding in findings.iter().filter(|f| all || f.needs_verdict()) {
        match format {
            Format::Text => println!("{}", row(finding)),
            Format::Json => println!("{}", serde_json::to_string(finding)?),
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
            Format::Json => println!("{}", event.to_json()),
        }
    }
    Ok(0)
}

fn prune(store: &mut Store, older_than: Option<u32>) -> Result<u8> {
    let removed = store.prune(older_than)?;
    println!("removed {removed} unreviewed finding(s) that are resolved or inactive");
    Ok(0)
}

fn resolve(root: &Path, store: &mut Store, review: &NewReview) -> Result<u8> {
    let Recorded {
        event,
        warnings,
        standing,
        catalog_error,
        ..
    } = record_verdict(root, store, review)?;
    if let Some(e) = catalog_error {
        eprintln!(
            "lighthouse: the catalog cannot be read ({e}); rule and catalog versions are not recorded"
        );
    }
    println!(
        "recorded {} ({}) for {} {}",
        event.verdict,
        event.reason,
        short(&event.fingerprint),
        event.rule_id
    );
    for warning in warnings {
        eprintln!("lighthouse: warning: {warning}");
    }
    match standing {
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

fn line(locator: &Value) -> u64 {
    locator["span"]["start"]["line"].as_u64().unwrap_or(0)
}

fn short(fingerprint: &str) -> &str {
    fingerprint.get(..SHORT).unwrap_or(fingerprint)
}
