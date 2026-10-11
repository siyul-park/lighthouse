//! The labels of a decision: what is known about subjects, from the sources
//! evaluation and training share. Only judgments under the decision's current
//! meaning version count; a label attributed to the model being trained or
//! evaluated is left out, so that a model never learns from itself.
//!
//! A subject has one label: the strongest source wins (a person, then a
//! reviewed table, then a coding agent, then a model), the latest among labels
//! of one source. The examples of the decision are subjects of their own.

use std::{collections::BTreeMap, fs, path::Path};

use lighthouse_model::{AgentKind, Attribution, Judgment, Role};
use lighthouse_plugin::Subject;
use lighthouse_spec::Decision;
use lighthouse_store::Records;
use serde_json::{Value, json};

use crate::{
    Result,
    metrics::{Outcome, Truth},
};

/// Who gave a label, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// A person, by a judgment.
    Person,
    /// A table of hand-reviewed findings.
    Tsv,
    /// A coding agent, by a judgment.
    Agent,
    /// A model, by a judgment.
    Model,
    /// An example the decision carries.
    Example,
}

impl Source {
    /// How much a label of this source is trusted.
    pub fn weight(self) -> f64 {
        match self {
            Self::Person | Self::Tsv | Self::Example => 1.0,
            Self::Agent => 0.7,
            Self::Model => 0.5,
        }
    }

    /// Lower is stronger; an example is never in competition.
    fn rank(self) -> u8 {
        match self {
            Self::Person | Self::Example => 0,
            Self::Tsv => 1,
            Self::Agent => 2,
            Self::Model => 3,
        }
    }

    /// The name a dataset line carries for the source.
    pub fn name(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Tsv => "tsv",
            Self::Agent => "agent",
            Self::Model => "model",
            Self::Example => "example",
        }
    }
}

/// The source a judgment's attribution stands for: a person, else a software
/// agent by the role it had, a coding agent unless it was a model.
impl From<&Attribution> for Source {
    fn from(attribution: &Attribution) -> Self {
        match (attribution.kind, attribution.role) {
            (AgentKind::Person, _) => Self::Person,
            (AgentKind::SoftwareAgent, Some(Role::Model)) => Self::Model,
            (AgentKind::SoftwareAgent, _) => Self::Agent,
        }
    }
}

/// What is known about one subject.
#[derive(Debug, Clone)]
pub struct Label {
    /// The subject's fingerprint; for an example, its own key.
    pub key: String,
    /// What the label says; `NotApplicable` is about the selector.
    pub truth: Truth,
    pub weight: f64,
    pub source: Source,
    /// PROV `wasAttributedTo` of the label.
    pub attribution: Value,
    /// An example subject: how the check came out on it.
    pub observed: Option<Outcome>,
    /// An example subject: what a model is asked about it.
    pub subject: Option<Subject>,
}

/// Where baseline rows come from: a table of reviewed findings.
pub struct Baseline<'a> {
    pub file: &'a Path,
    /// Only the rows of this repository; every row when `None`.
    pub repo: Option<&'a str>,
}

/// One subject an example shows.
pub struct Shown {
    /// `example:<decision>:<example>:<line>` for a `fail`, and
    /// `example:<decision>:<example>:<fingerprint>` for a `pass`.
    pub key: String,
    pub truth: Truth,
    /// How the check came out on it.
    pub outcome: Outcome,
    /// What a model is asked about it, when a rule put it to one.
    pub subject: Option<Subject>,
}

/// What to collect the labels of.
pub struct Collect<'a> {
    /// The project root, whose store holds the judgments.
    pub root: &'a Path,
    pub decision: &'a Decision,
    pub baseline: Option<&'a Baseline<'a>>,
    /// `<provider>/<model>`, any version: its own labels are left out.
    pub exclude_model: Option<&'a str>,
    /// The subjects the examples show, checked by the caller; none when the
    /// examples are not subjects.
    pub examples: Vec<Shown>,
}

impl Collect<'_> {
    /// The labels of the decision: its judgments under the current meaning
    /// version, the rows of the baseline table, and its examples.
    pub fn labels(mut self) -> Result<Vec<Label>> {
        let mut best: BTreeMap<String, Label> = BTreeMap::new();
        for label in self.judged()?.into_iter().chain(self.tabled()?) {
            let stronger = best
                .get(&label.key)
                .is_none_or(|have| label.source.rank() <= have.source.rank());
            if stronger {
                best.insert(label.key.clone(), label);
            }
        }
        let mut labels: Vec<Label> = best.into_values().collect();
        labels.extend(self.examples());
        Ok(labels)
    }

    fn judged(&self) -> Result<Vec<Label>> {
        let records = Records::read(self.root)?;
        let decision = self.decision;
        let meaning = decision.meaning_version();
        let mut labels = Vec::new();
        for event in &records.judgments {
            let ours = event.decision_uid.as_deref() == decision.uid() && decision.uid().is_some()
                || event.decision_name == decision.id();
            if !ours || event.meaning_version.as_deref() != Some(meaning.as_str()) {
                continue;
            }
            let truth = match event.judgment {
                Judgment::Fail => Truth::Fail,
                Judgment::Pass => Truth::Pass,
                Judgment::NotApplicable => Truth::NotApplicable,
            };
            let attributed = &event.was_attributed_to;
            let id = attributed.id.as_deref().unwrap_or_default();
            if self
                .exclude_model
                .is_some_and(|model| id.split('@').next() == Some(model))
            {
                continue;
            }
            let source = Source::from(attributed);
            labels.push(Label {
                key: event.fingerprint.clone(),
                truth,
                weight: source.weight(),
                source,
                attribution: serde_json::to_value(attributed)?,
                observed: None,
                subject: None,
            });
        }
        Ok(labels)
    }

    fn tabled(&self) -> Result<Vec<Label>> {
        self.baseline.map_or_else(|| Ok(Vec::new()), baseline_rows)
    }

    fn examples(&mut self) -> Vec<Label> {
        let attribution =
            json!({"type": "Person", "id": format!("example:{}", self.decision.id())});
        std::mem::take(&mut self.examples)
            .into_iter()
            .map(|s| Label {
                key: s.key,
                truth: s.truth,
                weight: Source::Example.weight(),
                source: Source::Example,
                attribution: attribution.clone(),
                observed: Some(s.outcome),
                subject: s.subject,
            })
            .collect()
    }
}

/// The reviewed rows of a baseline table: `TP` is a `fail`, `FP` a `pass`,
/// anything else (`unclear`) says nothing. Rows without a fingerprint (`-`)
/// cannot be found again and are left out. A hand review is a person's.
fn baseline_rows(baseline: &Baseline) -> Result<Vec<Label>> {
    let text = fs::read_to_string(baseline.file)
        .map_err(|e| format!("{}: {e}", baseline.file.display()))?;
    let attribution = json!({"type": "Person", "id": baseline.file.display().to_string()});
    let mut rows = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let cells: Vec<&str> = line.split('\t').collect();
        let [fingerprint, repo, _, verdict, ..] = cells[..] else {
            continue;
        };
        let truth = match verdict {
            "TP" => Truth::Fail,
            "FP" => Truth::Pass,
            _ => continue,
        };
        if fingerprint == "-"
            || fingerprint == "fingerprint"
            || baseline.repo.is_some_and(|wanted| wanted != repo)
        {
            continue;
        }
        rows.push(Label {
            key: fingerprint.to_owned(),
            truth,
            weight: Source::Tsv.weight(),
            source: Source::Tsv,
            attribution: attribution.clone(),
            observed: None,
            subject: None,
        });
    }
    Ok(rows)
}
