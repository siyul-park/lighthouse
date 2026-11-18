//! The co-firing of two decisions over the findings of a run: of the subjects
//! one decision reports on, how many the other reports on too. Empirical and
//! never a finding: `decision check` tells it as `likelyImpliedBy`, a lead for
//! a person, not a proof of redundancy.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use lighthouse_model::Diagnostic;
use lighthouse_spec::{Catalog, CheckKind, Decision};
use serde::Serialize;

/// The least share of `N`'s subjects that `S` also reports on (percent) for
/// `S` to be a likely implier. Set at the scale of design §4 E4's example (the
/// design gave no number); with a high share a miss is the exception to be
/// looked at, not the rule.
pub const MIN_SHARE: usize = 90;

/// The fewest subjects `N` must have for its share to count: fewer is too
/// few to tell a pattern from coincidence (Agrawal et al. 1993 ask a support
/// before a confidence, and this repository's smallest rules have a score of
/// subjects).
pub const MIN_SUBJECTS: usize = 20;

/// How much of the subjects of a decision another reports on too in one
/// language, over one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cofire {
    /// The language of the files the subjects are in.
    pub language: String,
    /// The decision `N` whose subjects are counted.
    pub decision: String,
    /// The decision `S` that reports on some of them.
    pub by: String,
    /// How many subjects `N` reports on.
    pub subjects: usize,
    /// How many of them `S` reports on too.
    pub shared: usize,
}

impl Cofire {
    /// The share of `N`'s subjects `S` reports on, in percent.
    pub fn percent(&self) -> f64 {
        if self.subjects == 0 {
            return 0.0;
        }
        self.shared as f64 * 100.0 / self.subjects as f64
    }

    /// Whether `S` likely implies `N`: the share and the number of subjects
    /// both reach their thresholds.
    pub fn likely(&self) -> bool {
        self.subjects >= MIN_SUBJECTS && self.shared * 100 >= self.subjects * MIN_SHARE
    }
}

/// The co-firing of every ordered pair of decisions of the same sort, in each
/// language both apply to, that both have findings in; sorted by decision,
/// language, descending share, then the other decision. `language` says
/// which language a file is of.
pub fn cofiring(
    catalog: &Catalog,
    findings: &[Diagnostic],
    language: &dyn Fn(&Path) -> Option<String>,
) -> Vec<Cofire> {
    let mut subjects: BTreeMap<(&str, String), BTreeSet<String>> = BTreeMap::new();
    for finding in findings {
        if let Some(tongue) = language(&finding.file) {
            subjects
                .entry((finding.rule_id.as_str(), tongue))
                .or_default()
                .insert(subject(finding));
        }
    }
    let mut found = Vec::new();
    for ((id, tongue), mine) in &subjects {
        let Some(decision) = catalog.decision(id) else {
            continue;
        };
        for ((other, there), theirs) in &subjects {
            let comparable = there == tongue
                && other != id
                && catalog
                    .decision(other)
                    .is_some_and(|d| comparable(decision, d, tongue));
            if comparable {
                found.push(Cofire {
                    language: tongue.clone(),
                    decision: (*id).to_owned(),
                    by: (*other).to_owned(),
                    subjects: mine.len(),
                    shared: mine.intersection(theirs).count(),
                });
            }
        }
    }
    found.sort_by(|a, b| {
        (&a.decision, &a.language, std::cmp::Reverse(a.shared), &a.by).cmp(&(
            &b.decision,
            &b.language,
            std::cmp::Reverse(b.shared),
            &b.by,
        ))
    });
    found
}

/// The decisions that likely imply `decision` over `findings`, each in a
/// language: those that report on at least [`MIN_SHARE`] percent of its
/// subjects there, when it has at least [`MIN_SUBJECTS`] there. What
/// `decision check` tells as `likelyImpliedBy`.
pub fn likely_implied_by(
    catalog: &Catalog,
    findings: &[Diagnostic],
    language: &dyn Fn(&Path) -> Option<String>,
    decision: &str,
) -> Vec<Cofire> {
    cofiring(catalog, findings, language)
        .into_iter()
        .filter(|c| c.decision == decision && c.likely())
        .collect()
}

/// What a finding is about: the value its rule selected, as the rule
/// identifies it; for a finding of a rule that selects none (a command or a
/// model), the symbol it names, else its file.
fn subject(finding: &Diagnostic) -> String {
    finding.subject.clone().unwrap_or_else(|| {
        finding
            .symbol
            .clone()
            .unwrap_or_else(|| finding.file.to_string_lossy().into_owned())
    })
}

/// Whether two decisions are about the same sort of subject and both apply to
/// `language`.
fn comparable(a: &Decision, b: &Decision, language: &str) -> bool {
    sort(a) == sort(b) && a.applies_to(language) && b.applies_to(language)
}

/// What a decision selects: its check's `select`, else its scope's subject.
fn sort(decision: &Decision) -> String {
    match decision.check.as_ref().map(|c| &c.kind) {
        Some(CheckKind::Cel(check)) => check.selects(decision.scope.subject).map_or_else(
            || decision.scope.subject.to_string(),
            |s| s.name().to_owned(),
        ),
        _ => decision.scope.subject.to_string(),
    }
}
