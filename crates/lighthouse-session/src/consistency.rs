//! Whether a decision holds together with the decisions in force: the
//! contradictions among the order decisions (proven on every shape of up to
//! two declarations), the conflicts its examples declare, and the decisions
//! whose findings its own cover (co-firing, empirical). Nothing is written and
//! nothing is remembered: a document is laid over the project's decisions in
//! memory. Whether its examples pass is `decision test`'s question: a
//! decision whose examples fail is checked all the same.

use std::path::Path;

use lighthouse_engine::{Bounded, Contradicted, NogoodKind, Overlays};
use lighthouse_logic::Tier;
use lighthouse_model::Severity;
use lighthouse_report::Detail;
use lighthouse_spec::{Catalog, Decision, DecisionStatus};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::{
    CheckRequest, Result, Session, active_decisions,
    authoring::{complete, layer, local_name},
    check::check_owning,
    cofiring::{Cofire, likely_implied_by, overlap, sort},
    decisions::test_catalog,
};

/// The exit code of a proven contradiction.
pub const CONTRADICTED: u8 = 1;

/// The exit code of an analysis that did not finish.
pub const UNFINISHED: u8 = 3;

/// What `decision check` checks.
#[derive(Debug, Clone)]
pub enum Target {
    /// A `Decision` document the project does not have (or has in another
    /// version): it is laid over the project's decisions, and not written.
    Document(Value),
    /// A decision of the catalog, as the project has it.
    Id(String),
}

/// The `DecisionCheck` kind: what the decisions in force say of one decision.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DecisionCheck {
    pub decision: String,
    /// The worst of what was found, in this order: `contradicts`,
    /// `incomplete`, `implied`, `interferes`, `independent`. `implied` is
    /// never the verdict yet: proven implication is not computed.
    pub verdict: Verdict,
    /// How far the decision can be reasoned about without running it: `F`
    /// formal, `O` ordering, `T` the rest.
    pub tier: String,
    /// Why it is in that tier.
    pub tier_reason: String,
    /// The sets of decisions that cannot hold together with this one, each
    /// with the smallest code that shows it.
    pub contradicts: Vec<Contradiction>,
    /// The decisions that provably imply this one or are equivalent to it.
    /// Always empty: proven implication is not computed yet.
    pub implied: Vec<Implied>,
    /// The decisions that report on most of the subjects this one reports on,
    /// over the findings of the project's run with this decision enabled. A
    /// lead for a person, never a finding.
    pub likely_implied_by: Vec<LikelyImplied>,
    /// The conflicts the examples of the decision declare and still have with
    /// other decisions.
    pub interferes: Vec<Interference>,
    /// The order decisions it was held to, in each language searched in full,
    /// that it does not contradict.
    pub independent_of: Vec<String>,
    /// The enabled decisions on the same subject and a shared language that
    /// no formal analysis decides (tier `T`): they may bear on this one.
    pub gaps: Vec<String>,
    /// What the searches done in full covered: no other contradiction exists
    /// within it. Never "proven consistent". Told only when none was found.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bounded: Vec<Scope>,
    /// Why an analysis involving this decision could not be done in full.
    /// A line about the run of the project behind `likelyImpliedBy` is told
    /// here too, and does not change the verdict or the exit code.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub incomplete: Vec<String>,
    /// The weakest basis among the entries; `incomplete` when an analysis did
    /// not finish.
    pub basis: Basis,
}

/// What the check found, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    /// A set of decisions that includes this one cannot hold together.
    Contradicts,
    /// An analysis involving this decision did not finish.
    Incomplete,
    /// Other decisions provably cover it.
    Implied,
    /// Its examples declare conflicts with other decisions.
    Interferes,
    /// Nothing was found.
    Independent,
}

/// How an entry was reached, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Basis {
    /// Shown by a witness.
    Proven,
    /// No other exists within a stated bound.
    Bounded,
    /// Seen on the code of the project, not shown.
    Empirical,
    /// An analysis did not finish.
    Incomplete,
}

/// A set of decisions that cannot hold together.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Contradiction {
    /// The language of the code the contradiction is on.
    pub language: String,
    /// The other decisions; this one alone when it flags every order of
    /// some declarations.
    pub with: Vec<String>,
    pub kind: Kind,
    pub basis: Basis,
    /// The shape of code, the order each decision forces and, at `--detail
    /// full`, every arrangement and who flags it.
    pub witness: Value,
    /// The decision that supersedes another of the set: the order of the
    /// decisions settles the contradiction, and it does not fail the check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
}

/// What kind of contradiction it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// No order of some declarations satisfies the decisions.
    Order,
    /// Together the decisions flag every subject.
    Covering,
}

/// Decisions that provably cover this one.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Implied {
    pub by: Vec<String>,
    pub equivalence: Equivalence,
    pub basis: Basis,
    pub label: String,
}

/// How a set of decisions covers one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Equivalence {
    Equivalent,
    Implied,
}

/// A decision that reports on most of the subjects of this one.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LikelyImplied {
    pub by: String,
    pub language: String,
    /// The share of this decision's subjects the other reports on too, in
    /// percent.
    pub cofire: f64,
    /// How many subjects this decision reports on in the language.
    pub subjects: usize,
    pub basis: Basis,
}

/// A conflict with another decision that an example of the decision declares
/// and the code still has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Interference {
    /// The language of the example.
    pub language: String,
    /// The other decision.
    pub with: String,
    /// The example that shows it.
    pub example: String,
    /// Whether the fix of the decision introduces it, rather than the code
    /// having it anyway.
    pub introduced: bool,
    /// Where the other decision reports, as `file:line`.
    pub places: Vec<String>,
    /// What the other decision says there.
    pub message: String,
}

/// What a search done in full covered, for one language.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub language: String,
    /// The most declarations of the shapes searched.
    pub declarations: usize,
    pub alphabet: String,
    pub renderer: String,
    /// The decisions with the options and digests the search saw.
    pub options: Vec<Assumed>,
}

/// A decision as a search saw it: what it asks, in a digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Assumed {
    pub decision: String,
    pub digest: String,
}

impl From<(&(String, Bounded), &Catalog)> for Scope {
    fn from(((language, bounded), catalog): (&(String, Bounded), &Catalog)) -> Self {
        let named = |uid: &str| {
            catalog
                .decisions()
                .find(|d| d.key() == uid)
                .map_or_else(|| uid.to_owned(), |d| d.id().to_owned())
        };
        Self {
            language: language.clone(),
            declarations: bounded.declarations,
            alphabet: bounded.alphabet.clone(),
            renderer: bounded.renderer.clone(),
            options: bounded
                .options
                .iter()
                .map(|assumption| Assumed {
                    decision: named(&assumption.uid),
                    digest: assumption.digest.clone(),
                })
                .collect(),
        }
    }
}

impl DecisionCheck {
    /// The exit code: 1 a contradiction, 3 an analysis that did not finish,
    /// else 0.
    pub fn code(&self) -> u8 {
        match self.verdict {
            Verdict::Contradicts => CONTRADICTED,
            Verdict::Incomplete => UNFINISHED,
            _ => 0,
        }
    }

    /// The worst of what was found: a contradiction no decision settles,
    /// then an analysis that did not finish, then proven implication, then
    /// interference, then nothing.
    pub fn worst_verdict(&self) -> Verdict {
        if self.contradicts.iter().any(is_open) {
            Verdict::Contradicts
        } else if !self.incomplete.is_empty() {
            Verdict::Incomplete
        } else if !self.implied.is_empty() {
            Verdict::Implied
        } else if !self.interferes.is_empty() {
            Verdict::Interferes
        } else {
            Verdict::Independent
        }
    }

    /// The weakest basis among the entries. `bounded` only where a search
    /// finished and found nothing; with no entry at all, `empirical`: the
    /// co-firing over the run of the project is all that was done.
    pub fn weakest_basis(&self) -> Basis {
        let entries = self
            .contradicts
            .iter()
            .map(|c| c.basis)
            .chain(self.implied.iter().map(|i| i.basis))
            .chain(self.likely_implied_by.iter().map(|l| l.basis))
            .chain((!self.bounded.is_empty()).then_some(Basis::Bounded))
            .chain((!self.incomplete.is_empty()).then_some(Basis::Incomplete));
        entries.max().unwrap_or(Basis::Empirical)
    }
}

/// What the decisions in force say of `target`, over the project of
/// `session`.
pub fn check_decision(session: &Session, target: Target, detail: Detail) -> Result<DecisionCheck> {
    let (candidate, id) = overlay(session, target)?;
    let decision = candidate
        .catalog
        .decision(&id)
        .ok_or_else(|| format!("unknown decision `{id}`"))?;
    let tier = lighthouse_logic::tier(candidate.comp.cel(), decision);
    let mut check = DecisionCheck {
        decision: id.clone(),
        verdict: Verdict::Independent,
        tier: tier.tier.as_str().to_owned(),
        tier_reason: tier.tier_reason,
        contradicts: Vec::new(),
        implied: Vec::new(),
        likely_implied_by: Vec::new(),
        interferes: Vec::new(),
        independent_of: Vec::new(),
        gaps: Vec::new(),
        bounded: Vec::new(),
        incomplete: Vec::new(),
        basis: Basis::Empirical,
    };
    if !decision.automated() {
        return Ok(check);
    }
    let ids = std::slice::from_ref(&id);
    let (test, analysis) = test_catalog(&candidate, &candidate.catalog, ids, None, None)?;
    check.contradicts = analysis
        .contradicted
        .iter()
        .map(|c| contradiction(c, &id, detail))
        .collect();
    check.interferes = analysis
        .declared
        .into_iter()
        .flat_map(|(language, found)| found.into_iter().map(move |i| interference(&language, i)))
        .collect();
    check.incomplete = test.incomplete.into_iter().chain(test.skipped).collect();
    // What a search covered is told only when it found nothing and finished.
    if check.incomplete.is_empty() && !check.contradicts.iter().any(is_open) {
        let covered = analysis.bounded.iter();
        check.bounded = covered
            .map(|b| Scope::from((b, &*candidate.catalog)))
            .collect();
    }
    let searched: Vec<&str> = check.bounded.iter().map(|s| s.language.as_str()).collect();
    check.independent_of = held_to(&candidate, decision, &searched);
    check
        .independent_of
        .retain(|id| !check.contradicts.iter().any(|c| c.with.contains(id)));
    check.gaps = gaps(&candidate, decision)?;
    let (likely, unfinished) = likely_implied(&candidate, &id)?;
    check.likely_implied_by = likely;
    check.verdict = check.worst_verdict();
    check.basis = check.weakest_basis();
    check.incomplete.extend(unfinished);
    Ok(check)
}

/// The session with `target` in it, in memory, enabled at the severity it
/// asks, and the id of the decision.
pub fn overlay(session: &Session, target: Target) -> Result<(Session, String)> {
    let (mut candidate, id) = match target {
        Target::Id(id) => {
            if session.catalog.decision(&id).is_none() {
                return Err(format!("unknown decision `{id}`").into());
            }
            (session.clone(), id)
        }
        Target::Document(mut document) => {
            let (id, name) = local_name(&document)?;
            complete(&mut document);
            // A proposed decision is checked as it would be once accepted.
            if let Some(spec) = document.get_mut("spec").and_then(Value::as_object_mut) {
                spec.insert(
                    "status".to_owned(),
                    serde_json::to_value(DecisionStatus::Accepted)?,
                );
            }
            let local = layer(session, &name, &document)?;
            (session.overlaid(local)?, id)
        }
    };
    if !active_decisions(&candidate)?.contains(&id) {
        let level = candidate
            .catalog
            .decision(&id)
            .and_then(|d| d.severity)
            .unwrap_or(Severity::Warn);
        candidate.config = candidate.config.clone().with_rule(&id, level);
    }
    Ok((candidate, id))
}

/// The enabled decisions of tier `T` that select what `decision` selects, in
/// a language both apply to: as co-firing compares decisions.
pub fn gaps(session: &Session, decision: &Decision) -> Result<Vec<String>> {
    let active = active_decisions(session)?;
    let mut ids: Vec<String> = session
        .catalog
        .decisions()
        .filter(|d| d.id() != decision.id() && active.contains(d.id()))
        .filter(|d| sort(d) == sort(decision) && overlap(decision, d, None))
        .filter(|d| lighthouse_logic::tier(session.comp.cel(), d).tier == Tier::T)
        .map(|d| d.id().to_owned())
        .collect();
    ids.sort_unstable();
    Ok(ids)
}

/// The order decisions, other than `decision`, that apply to one of
/// `languages`: the ones the search holds it to there.
pub fn held_to(session: &Session, decision: &Decision, languages: &[&str]) -> Vec<String> {
    let orders = session.orders();
    let mut ids: Vec<String> = session
        .catalog
        .decisions()
        .filter(|other| {
            other.id() != decision.id()
                && other.automated()
                && orders(other)
                && languages.iter().any(|language| other.applies_to(language))
        })
        .map(|other| other.id().to_owned())
        .collect();
    ids.sort_unstable();
    ids
}

/// The decisions whose findings cover most of those of `id` over a run of the
/// project with `id` enabled, and a line when that run did not finish: what
/// co-firing counts is then less than the project has.
pub fn likely_implied(session: &Session, id: &str) -> Result<(Vec<LikelyImplied>, Option<String>)> {
    let request = CheckRequest::unremembered(&session.root, Overlays::new());
    let (checked, engine) = check_owning(session, &request)?;
    let language = |file: &Path| engine.language_of(file).map(str::to_owned);
    let likely = likely_implied_by(&session.catalog, &checked.run.diagnostics, &language, id)
        .iter()
        .map(implier)
        .collect();
    let unfinished = checked.run.incomplete.first().map(|gap| {
        format!(
            "the run of the project behind `likelyImpliedBy` did not finish ({} part(s) not analyzed, e.g. {})",
            checked.run.incomplete.len(),
            gap.reason
        )
    });
    Ok((likely, unfinished))
}

/// Whether the contradiction fails the check: no decision settles it.
fn is_open(c: &Contradiction) -> bool {
    c.resolved_by.is_none()
}

fn contradiction(c: &Contradicted, id: &str, detail: Detail) -> Contradiction {
    let with: Vec<String> = match c.names.len() {
        1 => c.names.clone(),
        _ => c.names.iter().filter(|n| *n != id).cloned().collect(),
    };
    let mut witness = c.witness.clone();
    if let (Detail::Compact, Some(map)) = (detail, witness.as_object_mut()) {
        map.remove("arrangements");
    }
    Contradiction {
        language: c.language.clone(),
        with,
        kind: match c.kind {
            NogoodKind::Order => Kind::Order,
            NogoodKind::Covering => Kind::Covering,
        },
        basis: Basis::Proven,
        witness,
        resolved_by: c.resolved_by.clone(),
    }
}

fn implier(c: &Cofire) -> LikelyImplied {
    LikelyImplied {
        by: c.by.clone(),
        language: c.language.clone(),
        cofire: (c.percent() * 10.0).round() / 10.0,
        subjects: c.subjects,
        basis: Basis::Empirical,
    }
}

fn interference(language: &str, found: lighthouse_engine::Interference) -> Interference {
    Interference {
        language: language.to_owned(),
        with: found.with,
        example: found.example,
        introduced: found.introduced,
        places: found.places,
        message: found.message,
    }
}
