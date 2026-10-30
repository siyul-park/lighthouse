//! What one run of a check found, by subject.

use std::{collections::BTreeMap, sync::Arc};

use lighthouse_engine::Run;
use lighthouse_model::Judgment;
use lighthouse_plugin::Subject;

use super::{Layer, Score, Usage};
use crate::{labels::Sample, metrics::Outcome};

/// What one run of a check found, by subject.
#[derive(Default, Clone)]
pub struct Found {
    pub(super) by_fingerprint: BTreeMap<String, Outcome>,
    /// The subject a fingerprint is about: its file and symbol.
    pub(super) place: BTreeMap<String, String>,
    pub(super) by_place: BTreeMap<String, Outcome>,
    pub(super) usage: Usage,
    /// The probability of failing models gave, by place, with the model.
    pub(super) probabilities: BTreeMap<String, (f64, String)>,
    /// The notices of the run.
    pub(super) notices: Vec<String>,
    /// With subjects collected, what models are asked about each, by
    /// fingerprint.
    pub(super) subjects: Arc<BTreeMap<String, Subject>>,
    /// The layer of an attached model laid over the run, when one was.
    pub(super) layer: Option<Layer>,
}

/// A candidate check's run beside the current one.
pub struct Candidate<'a> {
    /// The id of the candidate decision.
    pub id: String,
    pub found: &'a Found,
    /// The labels as the candidate shows them: an example carries how the
    /// candidate checked it.
    pub labels: &'a [Sample],
}

impl Found {
    /// What a run found: the diagnostics and reviews by fingerprint and by
    /// place, and what models decided.
    pub fn of(run: &Run) -> Self {
        let mut found = Self::default();
        let each = run
            .diagnostics
            .iter()
            .map(|d| (d, Outcome::Fail))
            .chain(run.reviews.iter().map(|d| (d, Outcome::Review)));
        for (d, result) in each {
            let place = format!(
                "{}|{}",
                d.file.display(),
                d.symbol.as_deref().unwrap_or(d.fingerprint.as_str())
            );
            let fp = d.fingerprint.as_str().to_owned();
            if result == Outcome::Review {
                found.usage.reviews += 1;
            } else if d.model_version.is_some() {
                found.usage.model_decided += 1;
            }
            found.by_fingerprint.insert(fp.clone(), result);
            found.by_place.insert(place.clone(), result);
            found.place.insert(fp, place);
        }
        found.probabilities = run
            .probabilities
            .iter()
            .filter_map(|(fp, answer)| Some((found.place.get(fp)?.clone(), answer.clone())))
            .collect();
        found.notices = run.notices.iter().cloned().collect();
        found.subjects = Arc::new(run.questions.clone());
        found.usage.calls = run.model_calls;
        found.usage.cost = run.model_cost;
        found
    }

    /// With subjects collected, what models are asked about each, by
    /// fingerprint.
    pub fn subjects(&self) -> &BTreeMap<String, Subject> {
        &self.subjects
    }

    /// The layer laid over the run, when there is one.
    pub fn layer(&self) -> Option<&Layer> {
        self.layer.as_ref()
    }

    /// How the run came out on a label's subject: the outcome an example
    /// carries, else the one the run gave its fingerprint.
    pub fn outcome_of(&self, label: &Sample) -> Outcome {
        label.observed.unwrap_or_else(|| {
            self.by_fingerprint
                .get(&label.key)
                .copied()
                .unwrap_or(Outcome::NotApplicable)
        })
    }

    /// The probability of failing a model gave the subject of a label this
    /// run was shown, found by the place `current` knows it at. A
    /// `notApplicable` label is about the selector and is not scored.
    pub(super) fn scored(
        &self,
        current: &Found,
        label: &Sample,
        repo: Option<&str>,
    ) -> Option<Score> {
        let place = current.place.get(&label.key)?;
        let (probability, model) = self.probabilities.get(place)?;
        Some(Score {
            fingerprint: label.key.clone(),
            repo: repo.map(str::to_owned),
            violates: match label.judgment {
                Judgment::Fail => true,
                Judgment::Pass => false,
                Judgment::NotApplicable => return None,
            },
            probability: *probability,
            model: model.clone(),
        })
    }

    /// The rows of an audit table: `n` of the selected subjects the check did
    /// not flag, a deterministic sample, as `fingerprint`, `repo`, place and
    /// two empty cells for a person to label.
    pub(super) fn audit_rows(&self, n: usize, repo: Option<&str>) -> Vec<String> {
        let mut rows: Vec<(&String, &Subject)> = self
            .subjects
            .iter()
            .filter(|(fingerprint, _)| !self.by_fingerprint.contains_key(*fingerprint))
            .collect();
        rows.sort_by_cached_key(|(fingerprint, _)| {
            lighthouse_protocol::hash::short(fingerprint, 8)
        });
        rows.into_iter()
            .take(n)
            .map(|(fingerprint, subject)| {
                format!(
                    "{fingerprint}\t{}\t{} {}\t\t",
                    repo.unwrap_or("-"),
                    subject.state.file,
                    subject.id
                )
            })
            .collect()
    }

    /// A layer's answer about a subject the run left for review.
    pub(super) fn answered(&mut self, fingerprint: &str, noul: f64, by: &str, fails: bool) {
        let Some(place) = self.place.get(fingerprint).cloned() else {
            return;
        };
        if fails {
            self.by_fingerprint
                .insert(fingerprint.to_owned(), Outcome::Fail);
            self.by_place.insert(place.clone(), Outcome::Fail);
            self.usage.model_decided += 1;
            self.usage.reviews = self.usage.reviews.saturating_sub(1);
            self.probabilities.insert(place, (noul, by.to_owned()));
        } else {
            self.probabilities
                .entry(place)
                .or_insert((noul, by.to_owned()));
        }
    }
}
