//! Asking what an edit would do: the project is checked as it is and as it
//! would be with the edit, and the findings are compared. Neither run is
//! remembered.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use lighthouse_engine::Overlays;
use lighthouse_model::{Diagnostic, Incomplete, Severity, slashed};
use lighthouse_report::{Detail, FixInfo, Gap};
use lighthouse_spec::{Catalog, CheckKind};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

mod verdict;

pub use verdict::Verdict;

use crate::{CheckRequest, Checked, Fixing, Result, Session, active_decisions, findings, patch};

/// The first characters of a fingerprint a compact record shows.
const PREFIX: usize = lighthouse_model::Fingerprint::MIN_PREFIX;

/// The `Answer` kind: what the decisions say of an edit.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    pub kind: Kind,
    pub verdict: Verdict,
    pub premises: Vec<Premise>,
    /// Findings the edit would add to the files it edits and to `paths`.
    pub fired: Delta,
    /// Findings there now that the edit would remove.
    pub cleared: Delta,
    /// How many findings there are both ways.
    pub unchanged: usize,
    /// The enabled decisions that only a model decides: the answer does not
    /// cover them.
    pub gaps: Vec<String>,
    /// Parts of the project not analyzed, as `check` reports them.
    pub incomplete: Vec<Gap>,
    pub basis: Basis,
}

impl Answer {
    /// The enabled decisions whose check is a model's.
    fn undecided(session: &Session) -> Result<Vec<String>> {
        let active = active_decisions(session)?;
        Ok(session
            .catalog
            .decisions()
            .filter(|d| active.contains(d.id()))
            .filter(|d| matches!(d.check.as_ref().map(|c| &c.kind), Some(CheckKind::Model(_))))
            .map(|d| d.id().to_owned())
            .collect())
    }

    /// The exit code: 0 permitted, 1 forbidden, 3 unknown.
    pub fn code(&self) -> u8 {
        match self.verdict {
            Verdict::Permitted => 0,
            Verdict::Forbidden => 1,
            Verdict::Unknown => 3,
        }
    }
}

/// What the question asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// What the decisions say of an edit that is proposed.
    WhatIf,
}

/// How the answer was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Basis {
    /// By checking the code as it is and as the edit would make it.
    Evaluated,
}

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

impl Premise {
    /// The decisions behind `found`, once each, with the severity they fire at.
    pub(crate) fn all<'a>(
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

impl Delta {
    /// `found` as the records of `detail`.
    pub(crate) fn of(
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

/// The question: the edit as a unified diff, and the report scope besides the
/// files it edits.
pub struct WhatIf<'a> {
    pub edit: &'a str,
    /// Files or directories, as `check` takes them.
    pub paths: &'a [PathBuf],
    pub detail: Detail,
}

/// What the decisions say of `question`'s edit; `check` runs a request the
/// way the caller runs one (kept warm or not).
pub fn what_if(
    session: &Session,
    question: &WhatIf<'_>,
    check: &mut dyn FnMut(CheckRequest) -> Result<Checked>,
) -> Result<Answer> {
    let overlays = patch::overlays(&session.root, question.edit)?;
    let scope: Vec<PathBuf> = overlays
        .keys()
        .cloned()
        .chain(within(&session.root, question.paths)?)
        .collect();
    let mut before = check(CheckRequest::unremembered(&session.root, Overlays::new()))?;
    let mut after = check(CheckRequest::unremembered(&session.root, overlays))?;
    for checked in [&mut before, &mut after] {
        findings::apply_judgments(&session.root, &checked.catalog, &mut checked.run)?;
    }
    let seen = |checked: &Checked| -> BTreeMap<String, Diagnostic> {
        checked
            .run
            .diagnostics
            .iter()
            .filter(|d| scope.iter().any(|s| d.file.starts_with(s)))
            .map(|d| (d.fingerprint.as_str().to_owned(), d.clone()))
            .collect()
    };
    let (was, is) = (seen(&before), seen(&after));
    let (appear, vanish) = (only(&is, &was), only(&was, &is));
    let gaps: BTreeSet<&Incomplete> = before
        .run
        .incomplete
        .iter()
        .chain(&after.run.incomplete)
        .collect();
    let verdict = Verdict::of(
        &appear,
        (&before.run.incomplete, &after.run.incomplete),
        (&session.root, &scope),
    );
    let fixes = Fixing::of(session)?.all(
        &session.catalog,
        appear.iter().chain(&vanish).map(|d| d.rule_id.as_str()),
    );
    let fix_of = |d: &Diagnostic| fixes.get(&d.rule_id).and_then(|d| d.fix.clone());
    Ok(Answer {
        kind: Kind::WhatIf,
        verdict,
        premises: Premise::all(&session.catalog, appear.iter().chain(&vanish)),
        fired: Delta::of(&appear, question.detail, &fix_of),
        cleared: Delta::of(&vanish, question.detail, &fix_of),
        unchanged: was.keys().filter(|k| is.contains_key(*k)).count(),
        gaps: Answer::undecided(session)?,
        incomplete: gaps.into_iter().map(Gap::from).collect(),
        basis: Basis::Evaluated,
    })
}

/// `paths` as paths under the project root.
fn within(root: &Path, paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let root = root.canonicalize()?;
    paths
        .iter()
        .map(|path| {
            let absolute = path
                .canonicalize()
                .map_err(|e| format!("`{}`: {e}", path.display()))?;
            absolute
                .strip_prefix(&root)
                .map(Path::to_owned)
                .map_err(|_| {
                    format!(
                        "`{}` is outside the project root {}",
                        path.display(),
                        root.display()
                    )
                    .into()
                })
        })
        .collect()
}

/// The findings of `some` that `other` does not have, by fingerprint.
fn only<'a>(
    some: &'a BTreeMap<String, Diagnostic>,
    other: &BTreeMap<String, Diagnostic>,
) -> Vec<&'a Diagnostic> {
    some.iter()
        .filter(|(k, _)| !other.contains_key(*k))
        .map(|(_, d)| d)
        .collect()
}
