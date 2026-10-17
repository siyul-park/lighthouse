//! The labels of a decision: what is known about subjects, from the sources
//! evaluation and training share. Only judgments under the decision's current
//! meaning version count; a label attributed to the model being trained or
//! evaluated is left out, so that a model never learns from itself.
//!
//! A subject has one label: the strongest source wins (a person, then a
//! reviewed table, then a coding agent, then a model), the latest among labels
//! of one source. The examples of the decision are subjects of their own.

use std::{collections::BTreeMap, fs, path::Path};

use lighthouse_model::{AgentKind, Attribution, Judgment, Result, Role};
use lighthouse_plugin::Subject;
use lighthouse_spec::Decision;
use serde_json::{Value, json};

use crate::metrics::Outcome;

/// Who gave a label, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub judgment: Judgment,
    pub source: Source,
    /// PROV `wasAttributedTo` of the label.
    pub attribution: Value,
    /// An example subject: how the check came out on it.
    pub observed: Option<Outcome>,
    /// An example subject: what a model is asked about it.
    pub subject: Option<Subject>,
}

impl Label {
    /// A subject an example of `decision` shows: its key is
    /// `example:<decision>:<example>:<line>` for a `fail` and
    /// `example:<decision>:<example>:<fingerprint>` for a `pass`. `outcome` is
    /// how the check came out on it and `subject` what a model is asked, when
    /// a rule put it to one.
    pub fn example(
        decision: &str,
        key: String,
        judgment: Judgment,
        outcome: Outcome,
        subject: Option<Subject>,
    ) -> Self {
        Self {
            key,
            judgment,
            source: Source::Example,
            attribution: json!({"type": "Person", "id": format!("example:{decision}")}),
            observed: Some(outcome),
            subject,
        }
    }
}

/// A table of reviewed findings: `TP` is a `fail`, `FP` a `pass`, anything
/// else (`unclear`) says nothing. Rows without a fingerprint (`-`) cannot be
/// found again and are left out. A hand review is a person's.
pub struct Table<'a> {
    pub file: &'a Path,
    /// Only the rows of this repository; every row when `None`.
    pub repo: Option<&'a str>,
}

impl Table<'_> {
    fn rows(&self) -> Result<Vec<Label>> {
        let text =
            fs::read_to_string(self.file).map_err(|e| format!("{}: {e}", self.file.display()))?;
        let attribution = json!({"type": "Person", "id": self.file.display().to_string()});
        let mut rows = Vec::new();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let cells: Vec<&str> = line.split('\t').collect();
            let [fingerprint, repo, _, verdict, ..] = cells[..] else {
                continue;
            };
            let judgment = match verdict {
                "TP" => Judgment::Fail,
                "FP" => Judgment::Pass,
                _ => continue,
            };
            if fingerprint == "-"
                || fingerprint == "fingerprint"
                || self.repo.is_some_and(|wanted| wanted != repo)
            {
                continue;
            }
            rows.push(Label {
                key: fingerprint.to_owned(),
                judgment,
                source: Source::Tsv,
                attribution: attribution.clone(),
                observed: None,
                subject: None,
            });
        }
        Ok(rows)
    }
}

/// A judgment on a subject of the decision, as the store records it.
pub struct Judged {
    pub fingerprint: String,
    pub meaning_version: Option<String>,
    pub judgment: Judgment,
    pub by: Attribution,
}

/// What to collect the labels of.
pub struct Labels<'a> {
    /// The judgments given on the decision's subjects.
    pub judgments: Vec<Judged>,
    pub decision: &'a Decision,
    pub table: Option<&'a Table<'a>>,
    /// `<provider>/<model>`, any version: its own labels are left out.
    pub exclude_model: Option<&'a str>,
    /// The subjects the examples show, checked by the caller; none when the
    /// examples are not subjects.
    pub examples: Vec<Label>,
}

impl Labels<'_> {
    /// The labels of the decision: its judgments under the current meaning
    /// version, the rows of the table, and its examples.
    pub fn collect(self) -> Result<Vec<Label>> {
        let mut best: BTreeMap<String, Label> = BTreeMap::new();
        let tabled = self.table.map_or_else(|| Ok(Vec::new()), Table::rows)?;
        for label in self.judged()?.into_iter().chain(tabled) {
            let stronger = best
                .get(&label.key)
                .is_none_or(|have| label.source.rank() <= have.source.rank());
            if stronger {
                best.insert(label.key.clone(), label);
            }
        }
        let mut labels: Vec<Label> = best.into_values().collect();
        labels.extend(self.examples);
        Ok(labels)
    }

    fn judged(&self) -> Result<Vec<Label>> {
        let meaning = self.decision.meaning_version();
        let mut labels = Vec::new();
        for event in &self.judgments {
            if event.meaning_version.as_deref() != Some(meaning.as_str()) {
                continue;
            }
            let id = event.by.id.as_deref().unwrap_or_default();
            if self
                .exclude_model
                .is_some_and(|model| id.split('@').next() == Some(model))
            {
                continue;
            }
            labels.push(Label {
                key: event.fingerprint.clone(),
                judgment: event.judgment,
                source: Source::from(&event.by),
                attribution: serde_json::to_value(&event.by)?,
                observed: None,
                subject: None,
            });
        }
        Ok(labels)
    }
}
