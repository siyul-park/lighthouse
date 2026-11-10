//! The records of an answer: the decisions that bear on it, and the findings
//! that appear or disappear, in the shape of the detail asked for.

use std::collections::BTreeMap;

use crate::{Result, Session, active_decisions};

use lighthouse_model::{Diagnostic, Severity, slashed};
use lighthouse_report::{Detail, FixInfo};
use lighthouse_spec::{Catalog, CheckKind};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

/// The first characters of a fingerprint a compact record shows.
const PREFIX: usize = lighthouse_model::Fingerprint::MIN_PREFIX;

/// A decision that bears on the answer.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Premise {
    pub decision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// The first line of its requirement.
    pub requirement: String,
    /// The severity it fires at here.
    pub severity: Severity,
}

/// Findings that appear or disappear.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Delta {
    /// Grouped by decision, as `check` groups them.
    Compact(Vec<Group>),
    /// One record per finding.
    Full(Vec<Item>),
}

/// The findings of one decision at one severity.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub decision: String,
    pub severity: Severity,
    /// The fix the decision declares, for the findings in it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<FixInfo>,
    /// By file, each `[line, col, message, fingerprint prefix]`.
    pub files: BTreeMap<String, Vec<(u32, u32, String, String)>>,
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub decision: String,
    pub severity: Severity,
    pub path: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub evidence: Value,
    pub fingerprint: String,
    /// The fix the decision declares.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<FixInfo>,
}

/// The decisions behind `found`, once each, with the severity they fire at.
pub(super) fn premises<'a>(
    catalog: &Catalog,
    found: impl Iterator<Item = &'a &'a Diagnostic>,
) -> Vec<Premise> {
    let mut out: BTreeMap<&str, Premise> = BTreeMap::new();
    for d in found {
        out.entry(d.rule_id.as_str()).or_insert_with(|| {
            let decision = catalog.decision(&d.rule_id);
            Premise {
                decision: d.rule_id.clone(),
                uid: decision.and_then(|d| d.uid()).map(str::to_owned),
                requirement: decision
                    .and_then(|d| d.requirement.lines().next())
                    .unwrap_or_default()
                    .to_owned(),
                severity: d.severity,
            }
        });
    }
    out.into_values().collect()
}

/// `found` as the records of `detail`.
pub(super) fn delta(
    found: &[&Diagnostic],
    detail: Detail,
    fix: &dyn Fn(&Diagnostic) -> Option<FixInfo>,
) -> Delta {
    match detail {
        Detail::Full => Delta::Full(
            found
                .iter()
                .map(|d| Item {
                    decision: d.rule_id.clone(),
                    severity: d.severity,
                    path: slashed(&d.file),
                    line: d.span.start.line,
                    col: d.span.start.col,
                    message: d.message.clone(),
                    symbol: d.symbol.clone(),
                    evidence: d.evidence.clone(),
                    fingerprint: d.fingerprint.as_str().to_owned(),
                    fix: fix(d),
                })
                .collect(),
        ),
        Detail::Compact => {
            let mut groups: BTreeMap<(&str, Severity), Group> = BTreeMap::new();
            for d in found {
                let group = groups
                    .entry((d.rule_id.as_str(), d.severity))
                    .or_insert_with(|| Group {
                        decision: d.rule_id.clone(),
                        severity: d.severity,
                        fix: fix(d),
                        files: BTreeMap::new(),
                    });
                group.files.entry(slashed(&d.file)).or_default().push((
                    d.span.start.line,
                    d.span.start.col,
                    d.message.clone(),
                    d.fingerprint.as_str()[..PREFIX].to_owned(),
                ));
            }
            Delta::Compact(groups.into_values().collect())
        }
    }
}

/// The enabled decisions whose check is a model's.
pub(super) fn undecided(session: &Session) -> Result<Vec<String>> {
    let active = active_decisions(session)?;
    Ok(session
        .catalog
        .decisions()
        .filter(|d| active.contains(d.id()))
        .filter(|d| matches!(d.check.as_ref().map(|c| &c.kind), Some(CheckKind::Model(_))))
        .map(|d| d.id().to_owned())
        .collect())
}

/// The findings of `some` that `other` does not have, by fingerprint.
pub(super) fn only<'a>(
    some: &'a BTreeMap<String, Diagnostic>,
    other: &BTreeMap<String, Diagnostic>,
) -> Vec<&'a Diagnostic> {
    some.iter()
        .filter(|(k, _)| !other.contains_key(*k))
        .map(|(_, d)| d)
        .collect()
}
