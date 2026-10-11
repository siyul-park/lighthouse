//! The labels of a decision: what is known about subjects, from the sources
//! evaluation and training share. Only judgments under the decision's current
//! meaning version count; a label attributed to the model being trained or
//! evaluated is left out, so that a model never learns from itself.

use std::{collections::BTreeMap, fs, path::Path};

use lighthouse_model::{AgentKind, Judgment};
use lighthouse_plugin::{LabelSource, ResultKind};
use lighthouse_spec::Decision;
use lighthouse_store::Records;

use crate::Result;

/// How much a label of each source is trusted.
/// A person, a hand-reviewed baseline.
pub const WEIGHT_PERSON: f64 = 1.0;
/// An example the decision carries.
pub const WEIGHT_EXAMPLE: f64 = 1.0;
/// A coding agent.
pub const WEIGHT_AGENT: f64 = 0.7;
/// A model.
pub const WEIGHT_MODEL: f64 = 0.5;
/// A mining rule.
pub const WEIGHT_MINING: f64 = 0.3;

/// What is known about one subject.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub fingerprint: String,
    /// `Fail` or `Pass`.
    pub kind: ResultKind,
    pub weight: f64,
    pub source: LabelSource,
}

/// Where baseline rows come from: a table of reviewed findings.
pub struct Baseline<'a> {
    pub file: &'a Path,
    /// Only the rows of this repository; every row when `None`.
    pub repo: Option<&'a str>,
}

/// The labels of `decision`: its judgments under the current meaning version
/// and the rows of a baseline table, each fingerprint with its strongest
/// label (the latest among equals), without those attributed to
/// `exclude_model` (`<provider>/<model>`, any version).
pub fn collect(
    root: &Path,
    decision: &Decision,
    baseline: Option<&Baseline>,
    exclude_model: Option<&str>,
) -> Result<Vec<Label>> {
    let mut best: BTreeMap<String, Label> = BTreeMap::new();
    let mut keep = |label: Label| {
        let stronger = best
            .get(&label.fingerprint)
            .is_none_or(|have| label.weight >= have.weight);
        if stronger {
            best.insert(label.fingerprint.clone(), label);
        }
    };
    let records = Records::read(root)?;
    let meaning = decision.meaning_version();
    for event in &records.judgments {
        let ours = event.decision_uid.as_deref() == decision.uid() && decision.uid().is_some()
            || event.decision_name == decision.id();
        if !ours || event.meaning_version.as_deref() != Some(meaning.as_str()) {
            continue;
        }
        let kind = match event.judgment {
            Judgment::Fail => ResultKind::Fail,
            Judgment::Pass => ResultKind::Pass,
            Judgment::NotApplicable => continue,
        };
        let attributed = &event.was_attributed_to;
        let id = attributed.id.as_deref().unwrap_or_default();
        let (weight, source) = match attributed.kind {
            AgentKind::Person => (WEIGHT_PERSON, LabelSource::Person),
            AgentKind::SoftwareAgent if id.contains('@') => (WEIGHT_MODEL, LabelSource::Model),
            AgentKind::SoftwareAgent => (WEIGHT_AGENT, LabelSource::Person),
        };
        if exclude_model.is_some_and(|model| id.split('@').next() == Some(model)) {
            continue;
        }
        keep(Label {
            fingerprint: event.fingerprint.clone(),
            kind,
            weight,
            source,
        });
    }
    if let Some(baseline) = baseline {
        for label in baseline_rows(baseline)? {
            keep(label);
        }
    }
    Ok(best.into_values().collect())
}

/// The reviewed rows of a baseline table: `TP` is a `fail`, `FP` a `pass`,
/// anything else (`unclear`) says nothing. Rows without a fingerprint (`-`)
/// cannot be found again and are left out. A hand review is a person's.
fn baseline_rows(baseline: &Baseline) -> Result<Vec<Label>> {
    let text = fs::read_to_string(baseline.file)
        .map_err(|e| format!("{}: {e}", baseline.file.display()))?;
    let mut rows = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let cells: Vec<&str> = line.split('\t').collect();
        let [fingerprint, repo, _, verdict, ..] = cells[..] else {
            continue;
        };
        let kind = match verdict {
            "TP" => ResultKind::Fail,
            "FP" => ResultKind::Pass,
            _ => continue,
        };
        if fingerprint == "-"
            || fingerprint == "fingerprint"
            || baseline.repo.is_some_and(|wanted| wanted != repo)
        {
            continue;
        }
        rows.push(Label {
            fingerprint: fingerprint.to_owned(),
            kind,
            weight: WEIGHT_PERSON,
            source: LabelSource::Baseline,
        });
    }
    Ok(rows)
}
