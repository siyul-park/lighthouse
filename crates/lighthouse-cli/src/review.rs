//! `lighthouse review`: the findings the store remembers and the verdicts on
//! them.

use std::fmt::Write;

use clap::{Subcommand, ValueEnum};
use lighthouse_model::{Reason, ReviewerKind, Verdict};
use lighthouse_store::{Filter, FindingRecord, NewReview, ReviewEvent, StatusFilter, Store};
use serde_json::{Value, json};

use crate::{Result, git, session::Session};

/// Characters of a fingerprint that listings show; any unambiguous prefix
/// names a finding.
const SHORT: usize = 12;

#[derive(Subcommand)]
pub enum ReviewCommand {
    /// List the findings the store remembers.
    List {
        /// Only this fully qualified rule id.
        #[arg(long)]
        rule: Option<String>,
        /// `open`: still reported and not suppressed. `suppressed`: kept out
        /// of reports by a rejected verdict. `all`: every remembered finding.
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
    /// reports. deferred (no reason) leaves it visible. Verdicts are appended,
    /// never edited: a later one replaces the standing of the finding.
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
        #[arg(long, default_value = "human")]
        reviewer_kind: ReviewerKind,
        #[arg(long)]
        reviewer_id: Option<String>,
    },
    /// Show every verdict recorded on a finding, oldest first.
    History {
        /// A fingerprint, or the start of one.
        fingerprint: String,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Status {
    Open,
    Suppressed,
    All,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Text,
    Json,
}

pub fn run(command: ReviewCommand) -> Result<u8> {
    let session = Session::load_or_default(None)?;
    let path = Store::path_in(&session.root);
    match command {
        ReviewCommand::List {
            rule,
            status,
            format,
        } => match Store::open_existing(&path)? {
            Some(store) => list(&store, rule, status, format),
            None => {
                eprintln!("lighthouse: no findings recorded yet (run `lighthouse check`)");
                Ok(0)
            }
        },
        ReviewCommand::Show {
            fingerprint,
            format,
        } => show(&existing(&path)?, &fingerprint, format),
        ReviewCommand::History {
            fingerprint,
            format,
        } => history(&existing(&path)?, &fingerprint, format),
        ReviewCommand::Resolve {
            fingerprint,
            verdict,
            reason,
            note,
            reviewer_kind,
            reviewer_id,
        } => {
            let mut review = NewReview {
                fingerprint,
                verdict,
                reason,
                reason_text: note,
                reviewer_kind,
                reviewer_id,
                rule_version: None,
                catalog_version: None,
                pattern_fingerprint: None,
                scope: None,
                commit: git::head(&session.root),
            };
            let mut store = existing(&path)?;
            version(&mut review, &store, &session)?;
            resolve(&mut store, &review)
        }
    }
}

/// The store, which must already exist: nothing is remembered before a check.
fn existing(path: &std::path::Path) -> Result<Store> {
    Store::open_existing(path)?
        .ok_or_else(|| "no findings recorded yet (run `lighthouse check`)".into())
}

/// Stamps the review with the versions of the pattern and catalog the rule of
/// the finding comes from, when the catalog knows the rule.
fn version(review: &mut NewReview, store: &Store, session: &Session) -> Result<()> {
    let catalog = session.catalog()?;
    let finding = store.finding(&review.fingerprint)?;
    if let Some(pattern) = catalog.pattern(&finding.rule_id) {
        review.rule_version = Some(pattern.version());
        review.scope = Some(pattern.scope.to_string());
    }
    review.catalog_version = Some(catalog.version());
    Ok(())
}

fn list(store: &Store, rule: Option<String>, status: Status, format: Format) -> Result<u8> {
    let status = match status {
        Status::Open => StatusFilter::Open,
        Status::Suppressed => StatusFilter::Suppressed,
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
    for event in store.history(fingerprint)? {
        match format {
            Format::Text => println!("{}", event_row(&event)),
            Format::Json => println!("{}", event_json(&event)?),
        }
    }
    Ok(0)
}

fn resolve(store: &mut Store, review: &NewReview) -> Result<u8> {
    let event = store.resolve(review)?;
    println!(
        "recorded {} ({}) for {} {}",
        event.verdict,
        event.reason,
        short(&event.fingerprint),
        event.rule_id
    );
    if store.suppressed()?.contains(&event.fingerprint) {
        println!(
            "later checks keep this finding out of the report; `lighthouse review list --status suppressed` lists it"
        );
    }
    Ok(0)
}

/// One line per finding: fingerprint, rule, severity, location, state, latest
/// review and message, tab-separated.
fn row(finding: &FindingRecord) -> String {
    let review = finding
        .review
        .map_or_else(|| "-".to_owned(), |r| format!("{}:{}", r.verdict, r.reason));
    format!(
        "{}\t{}\t{}\t{}:{}\t{}\t{review}\t{}",
        short(&finding.fingerprint),
        finding.rule_id,
        finding.severity,
        finding.path,
        line(&finding.locator),
        state(finding),
        finding.message
    )
}

fn detail(finding: &FindingRecord) -> String {
    let mut out = String::new();
    let mut field = |label: &str, value: String| {
        let _ = writeln!(out, "{label:<13}{value}");
    };
    field("fingerprint:", finding.fingerprint.clone());
    field(
        "rule:",
        format!("{} ({})", finding.rule_id, finding.severity),
    );
    field(
        "location:",
        format!("{}:{}", finding.path, line(&finding.locator)),
    );
    if let Some(symbol) = &finding.symbol {
        field("owner:", symbol.clone());
    }
    field("state:", state(finding).to_owned());
    field("first seen:", finding.first_seen.clone());
    field("last seen:", finding.last_seen.clone());
    if let Some(resolved) = &finding.resolved_at {
        field(
            "resolved:",
            format!("{resolved} (reopened {} time(s))", finding.reopened),
        );
    }
    field("message:", finding.message.clone());
    if let Some(evidence) = finding.evidence.as_object().filter(|e| !e.is_empty()) {
        let pairs: Vec<String> = evidence.iter().map(|(k, v)| format!("{k}={v}")).collect();
        field("evidence:", pairs.join(" "));
    }
    if let Some(review) = finding.review {
        field("review:", format!("{} ({})", review.verdict, review.reason));
    }
    out
}

fn event_row(event: &ReviewEvent) -> String {
    format!(
        "#{}\t{}\t{}:{}\t{}:{}\t{}\t{}",
        event.id,
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

/// `suppressed` once a rejection keeps the finding out of reports, else
/// `resolved` when a complete run no longer saw it, else `open`.
fn state(finding: &FindingRecord) -> &'static str {
    match (finding.suppressed, &finding.resolved_at) {
        (true, _) => "suppressed",
        (false, Some(_)) => "resolved",
        (false, None) => "open",
    }
}

fn line(locator: &Value) -> u64 {
    locator["span"]["start"]["line"].as_u64().unwrap_or(0)
}

fn short(fingerprint: &str) -> &str {
    fingerprint.get(..SHORT).unwrap_or(fingerprint)
}
