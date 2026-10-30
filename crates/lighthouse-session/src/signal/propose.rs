//! From signals to proposed decisions: the dedupe check before a decision is
//! written, the settling of the signals a decision derives from, and the
//! support a proposal has.

use std::collections::BTreeMap;

use lighthouse_model::{Attribution, SignalStatus};
use lighthouse_spec::{Decision, DecisionStatus};
use lighthouse_store::Store;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::{read::DERIVED_PREFIX, spelled};
use crate::{Result, Session};

/// The similarity from which an existing decision is named a possible
/// duplicate of a new one. Set conservatively, before any data was collected:
/// it is tuned later on the prompts and proposals that shadow mode gathers.
pub const DUPLICATE_THRESHOLD: f64 = 0.7;
/// The lowest similarity at which a decision is offered to link to when a
/// message reads like a decision. Set from the scores of the bundled catalog
/// under FTS5 BM25: a decision that only shares common words ("library",
/// "code") scores up to about 0.47, one that says the same thing 0.5 and up.
pub const CANDIDATE_FLOOR: f64 = 0.5;
/// Most characters of a requirement shown for a candidate.
const SHOWN: usize = 140;

/// An existing decision a new one may repeat.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Duplicate {
    pub id: String,
    pub status: String,
    /// The first line of its requirement.
    pub requirement: String,
    /// BM25 of the decision for the new text, from 0 to 1.
    pub similarity: f64,
}

/// What settling the signals of a document did.
#[derive(Debug, Default)]
pub struct Settled {
    /// The ids of the signals now recorded or linked.
    pub signals: Vec<String>,
    /// Named ids that match no signal, such as ones the retention purged.
    pub unknown: Vec<String>,
}

/// How well a proposal is supported: the distinct signals it derives from, by
/// class and by who they are attributed to, and the Beta-smoothed share of
/// them a person made (the posterior mean under a uniform prior).
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Support {
    pub signals: usize,
    /// Signals by class, then by `person`, `agent` or `model`.
    pub by_class: BTreeMap<String, BTreeMap<String, usize>>,
    /// `(person + 1) / (signals + 2)`.
    pub person_share: f64,
}

impl Support {
    /// `3 signals (directive: person 2; correction: agent 1), person share 0.60`.
    pub fn line(&self) -> String {
        let classes: Vec<String> = self
            .by_class
            .iter()
            .map(|(class, sources)| {
                let sources: Vec<String> =
                    sources.iter().map(|(s, n)| format!("{s} {n}")).collect();
                format!("{class}: {}", sources.join(", "))
            })
            .collect();
        format!(
            "{} signal(s) ({}), person share {:.2}",
            self.signals,
            classes.join("; "),
            self.person_share
        )
    }
}

/// The decisions, accepted or proposed, that rank at least `floor` for
/// `query`, best first and at most `limit`; `skip` is left out. The ranking is
/// BM25 of SQLite FTS5 over the text of the decisions.
pub fn candidates(
    session: &Session,
    query: &str,
    skip: Option<&str>,
    floor: f64,
    limit: usize,
) -> Result<Vec<Duplicate>> {
    let pool: Vec<&Decision> = session
        .catalog
        .decisions()
        .filter(|d| {
            matches!(
                d.status,
                DecisionStatus::Accepted | DecisionStatus::Proposed
            )
        })
        .filter(|d| Some(d.id()) != skip)
        .collect();
    let documents = documents(&pool);
    let store = Store::open(&session.root)?;
    Ok(store
        .rank(&documents, query, limit.max(1) * 4)?
        .into_iter()
        .filter(|m| m.similarity >= floor)
        .filter_map(|m| {
            let decision = pool.iter().find(|d| d.id() == m.id)?;
            Some(Duplicate {
                id: m.id,
                status: decision.status.to_string(),
                requirement: first_line(&decision.requirement),
                similarity: (m.similarity * 100.0).round() / 100.0,
            })
        })
        .take(limit)
        .collect())
}

/// The documents `decisions` are ranked as: `(id, text)`.
pub(crate) fn documents(decisions: &[&Decision]) -> Vec<(String, String)> {
    decisions
        .iter()
        .map(|d| (d.id().to_owned(), text_of(d)))
        .collect()
}

pub(crate) fn first_line(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or_default().trim();
    let squashed: String = line.chars().take(SHOWN).collect();
    if line.chars().count() > SHOWN {
        format!("{squashed}…")
    } else {
        squashed
    }
}

/// Settles the signals `document` derives from: `recorded` when it is a new
/// decision, `linked` when it changes an existing one.
pub(crate) fn settle_derived(
    session: &Session,
    document: &Value,
    uid: &str,
    created: bool,
    by: &Attribution,
) -> Result<Settled> {
    let ids = named(document);
    let mut settled = Settled::default();
    if ids.is_empty() {
        return Ok(settled);
    }
    let mut store = Store::open(&session.root)?;
    for id in ids {
        match store.signal(&id) {
            Ok(signal) => {
                let decision = uid.to_owned();
                let status = if created {
                    SignalStatus::Recorded { decision }
                } else {
                    SignalStatus::Linked { decision }
                };
                store.settle_signal(&signal.id, &status, &spelled(by))?;
                settled.signals.push(signal.id);
            }
            Err(lighthouse_store::Error::UnknownSignal(_)) => settled.unknown.push(id),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(settled)
}

/// Records a new proposal and what the dedupe check named, for the measures.
pub(crate) fn record_proposal(
    session: &Session,
    uid: &str,
    id: &str,
    found: &[Duplicate],
) -> Result<()> {
    let names: Vec<String> = found.iter().map(|d| d.id.clone()).collect();
    Ok(Store::open(&session.root)?.add_proposal(uid, id, &names)?)
}

/// The support of `decision`, from the signals its provenance names that the
/// store still holds; `None` when it names none.
pub fn support(session: &Session, decision: &Decision) -> Result<Option<Support>> {
    let ids: Vec<String> = decision
        .provenance
        .was_derived_from
        .iter()
        .filter_map(|entry| entry.strip_prefix(DERIVED_PREFIX))
        .map(str::to_owned)
        .collect();
    if ids.is_empty() {
        return Ok(None);
    }
    let Some(store) = Store::open_existing(&session.root)? else {
        return Ok(None);
    };
    let mut by_class: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let (mut signals, mut people) = (0, 0);
    let mut seen = Vec::new();
    for id in ids {
        let Ok(signal) = store.signal(&id) else {
            continue;
        };
        if seen.contains(&signal.id) {
            continue;
        }
        let source = spelled(&signal.by);
        let source = source.split(':').next().unwrap_or_default().to_owned();
        people += usize::from(source == "person");
        signals += 1;
        *by_class
            .entry(signal.class.to_string())
            .or_default()
            .entry(source)
            .or_default() += 1;
        seen.push(signal.id);
    }
    Ok(Some(Support {
        signals,
        by_class,
        person_share: ((people + 1) as f64 / (signals + 2) as f64 * 100.0).round() / 100.0,
    }))
}

/// What a decision says, for ranking: its name, title, requirement, why,
/// scope and labels.
fn text_of(decision: &Decision) -> String {
    let labels: Vec<&str> = decision
        .metadata()
        .labels
        .values()
        .map(String::as_str)
        .collect();
    format!(
        "{} {} {} {} {} {}",
        decision.id().replace(['/', '-', '_'], " "),
        decision.title,
        decision.requirement,
        decision.context,
        decision.scope.subject,
        labels.join(" ")
    )
}

/// The signals a decision document names in `provenance.wasDerivedFrom`.
fn named(document: &Value) -> Vec<String> {
    document
        .pointer("/spec/provenance/wasDerivedFrom")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|entry| entry.strip_prefix(DERIVED_PREFIX))
        .map(|id| id.trim().to_owned())
        .collect()
}
