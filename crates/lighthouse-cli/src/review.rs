//! `lighthouse review`: the findings the store remembers and the judgments on
//! them.

use std::{fmt::Write, path::Path};

use clap::{Subcommand, ValueEnum};
use lighthouse_session::{
    AgentKind, FindingRecord, Judgment, JudgmentEvent, NewJudgment, Recorded, Reviewer, Standing,
    StatusFilter, TaskQuery, head, project_root, record_judgment, review_finding, review_history,
    review_prune, review_tasks,
};
use serde_json::Value;

use crate::Result;

/// Characters of a fingerprint that listings show; any unambiguous prefix
/// names a finding.
const SHORT: usize = 12;

#[derive(Subcommand)]
pub enum ReviewCommand {
    /// List the findings that ask for review: those of decisions that
    /// authored `warn` or `info` and that no judgment stands for.
    List {
        /// Also list findings that do not ask for review (errors, and judged
        /// ones).
        #[arg(long)]
        all: bool,
        /// Only this fully qualified rule id.
        #[arg(long)]
        rule: Option<String>,
        /// `open`: still reported and not suppressed. `suppressed`: kept out
        /// of reports by a judgment. `narrowing`: judged `notApplicable`, so
        /// the decision is too broad. `inactive`: of a rule the configuration no
        /// longer enables. `resolved`: no longer reported by a complete run.
        /// `all`: every remembered finding.
        #[arg(long, value_enum, default_value = "open")]
        status: StatusArg,
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
    /// Record a judgment on a finding.
    ///
    /// `fail` means the finding is right; add --suppress with a justification
    /// to leave it in place on purpose (a SARIF `external` suppression).
    /// `pass` means the code conforms (a false positive); `notApplicable`
    /// means the decision does not apply here, a hint to narrow it. A judgment
    /// that is not `fail`, or a `fail` with a suppression, keeps the finding
    /// out of later reports while its decision and evidence stay as they
    /// are; an error is recorded but never hidden. Judgments are appended to
    /// .lighthouse/decisions.jsonl, which is meant to be committed, and never
    /// edited: a later one replaces the standing of the finding.
    ///
    /// The reviewer is --reviewer-kind (Person or SoftwareAgent; `human` and
    /// `agent` are read too), else $LIGHTHOUSE_REVIEWER_KIND, else a person;
    /// the id is --reviewer-id, else $LIGHTHOUSE_REVIEWER, else $USER.
    Resolve {
        /// A fingerprint, or the start of one.
        fingerprint: String,
        #[arg(long)]
        judgment: Judgment,
        /// With `fail`: why the finding stays, recorded as an external
        /// suppression.
        #[arg(long, value_name = "JUSTIFICATION")]
        suppress: Option<String>,
        /// Why, written for the next reader.
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        reviewer_kind: Option<AgentKind>,
        #[arg(long)]
        reviewer_id: Option<String>,
        /// The `last seen` time of the finding as you read it; refuses the
        /// judgment if the finding has been seen again since.
        #[arg(long, value_name = "LAST_SEEN")]
        seen: Option<String>,
    },
    /// Show every judgment recorded on a finding, oldest first.
    History {
        /// A fingerprint, or the start of one.
        fingerprint: String,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    /// Delete resolved and inactive findings that nobody judged. Findings
    /// with judgments stay, because the judgments are labels.
    Prune {
        /// Only those resolved or inactive for at least this many days.
        #[arg(long, value_name = "DAYS")]
        older_than: Option<u32>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum StatusArg {
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
        } => match review_tasks(&root, &query(rule, status, all))? {
            Some(tasks) => list(&tasks.findings, format),
            None => {
                eprintln!("lighthouse: no findings recorded yet (run `lighthouse check`)");
                Ok(0)
            }
        },
        ReviewCommand::Show {
            fingerprint,
            format,
        } => show(&review_finding(&root, &fingerprint)?, format),
        ReviewCommand::History {
            fingerprint,
            format,
        } => history(&review_history(&root, &fingerprint)?, &fingerprint, format),
        ReviewCommand::Prune { older_than } => prune(review_prune(&root, older_than)?),
        ReviewCommand::Resolve {
            fingerprint,
            judgment,
            suppress,
            reason,
            reviewer_kind,
            reviewer_id,
            seen,
        } => {
            let reviewer = Reviewer::from_env(reviewer_kind, reviewer_id)?;
            let review = NewJudgment {
                fingerprint,
                judgment,
                reason,
                suppress,
                attribution: reviewer.attribution(),
                commit: head(&root),
                expect_seen: seen,
                lighthouse_version: env!("CARGO_PKG_VERSION").to_owned(),
            };
            resolve(&root, &review)
        }
    }
}

/// What `list` asks the session for.
fn query(rule: Option<String>, status: StatusArg, all_tiers: bool) -> TaskQuery {
    let status = match status {
        StatusArg::Open => StatusFilter::Open,
        StatusArg::Suppressed => StatusFilter::Suppressed,
        StatusArg::Narrowing => StatusFilter::Narrowing,
        StatusArg::Inactive => StatusFilter::Inactive,
        StatusArg::Resolved => StatusFilter::Resolved,
        StatusArg::All => StatusFilter::All,
    };
    TaskQuery {
        rule,
        status,
        all_tiers,
    }
}

fn list(findings: &[FindingRecord], format: Format) -> Result<u8> {
    for finding in findings {
        match format {
            Format::Text => println!("{}", row(finding)),
            Format::Json => println!("{}", serde_json::to_string(finding)?),
        }
    }
    Ok(0)
}

fn show(finding: &FindingRecord, format: Format) -> Result<u8> {
    match format {
        Format::Text => print!("{}", detail(finding)),
        Format::Json => println!("{}", serde_json::to_string(finding)?),
    }
    Ok(0)
}

fn history(events: &[JudgmentEvent], fingerprint: &str, format: Format) -> Result<u8> {
    if events.is_empty() {
        eprintln!("lighthouse: no judgments recorded for {fingerprint}");
    }
    for event in events {
        match format {
            Format::Text => println!("{}", event_row(event)),
            Format::Json => println!("{}", event.to_json()),
        }
    }
    Ok(0)
}

fn prune(removed: usize) -> Result<u8> {
    println!("removed {removed} unreviewed finding(s) that are resolved or inactive");
    Ok(0)
}

fn resolve(root: &Path, review: &NewJudgment) -> Result<u8> {
    let Recorded {
        event,
        warnings,
        standing,
        catalog_error,
        ..
    } = record_judgment(root, review)?;
    if let Some(e) = catalog_error {
        eprintln!(
            "lighthouse: the catalog cannot be read ({e}); decision and catalog versions are not recorded"
        );
    }
    let suppressed = event
        .suppressions
        .first()
        .map(|s| format!(" and an {} suppression ({})", s.kind, s.justification))
        .unwrap_or_default();
    println!(
        "recorded {}{suppressed} for {} {}",
        event.judgment,
        short(&event.fingerprint),
        event.decision_name
    );
    for warning in warnings {
        eprintln!("lighthouse: warning: {warning}");
    }
    match standing {
        Some(Standing::Suppressed) => println!(
            "later checks keep this finding out of the report while its decision and evidence stay as they are; `lighthouse review list --status suppressed` lists it"
        ),
        Some(Standing::Unsuppressible) => println!(
            "this is an error: the judgment is recorded but the finding stays reported; fix the code or suppress it in the code"
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
    if finding.judgment.is_some() {
        field("judgment:", review_text(finding));
    }
    out
}

fn event_row(event: &JudgmentEvent) -> String {
    let suppression = event.suppressions.first().map_or("-".to_owned(), |s| {
        format!("{}:{}", s.kind, s.justification)
    });
    format!(
        "{}\t{}\t{}\t{}\t{}:{}\t{}\t{}",
        short(&event.id),
        event.generated_at_time,
        event.judgment,
        suppression,
        event.was_attributed_to.kind,
        event.was_attributed_to.id.as_deref().unwrap_or("-"),
        event.label(),
        event.reason.as_deref().unwrap_or("")
    )
}

fn line(locator: &Value) -> u64 {
    locator["span"]["start"]["line"].as_u64().unwrap_or(0)
}

fn short(fingerprint: &str) -> &str {
    fingerprint.get(..SHORT).unwrap_or(fingerprint)
}
