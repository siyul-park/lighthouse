//! Environments, labels and nogoods, the part of an assumption-based truth
//! maintenance system (de Kleer 1986) the conflicts of decisions need. An
//! assumption is a decision; an environment is a set of them; the label of a
//! datum is the minimal environments it holds in; a nogood is a minimal
//! environment that cannot hold together. Nothing here runs a decision: the
//! callers give which decisions flag which arrangements.

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use lighthouse_protocol::hash;
use lighthouse_spec::{Decision, Error};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The most decisions whose hitting sets are enumerated: every subset is
/// tried, so the work doubles with each.
pub const MAX_DECISIONS: usize = 10;

/// A decision as an assumption: its key and the [`digest`] of what it asks, so
/// a decision that changed (or runs with other options) is another assumption.
/// A decision that later gains a uid is another assumption too.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Assumption {
    pub uid: String,
    pub digest: String,
}

impl Assumption {
    /// The assumption `decision` is when it runs with the `configured`
    /// options for `language`; see [`digest`].
    pub fn of(
        decision: &Decision,
        configured: &Map<String, Value>,
        language: Option<&str>,
    ) -> Result<Self, Error> {
        Ok(Self {
            uid: decision.key().to_owned(),
            digest: digest(decision, configured, language)?,
        })
    }
}

/// A sorted set of assumptions.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Env(BTreeSet<Assumption>);

impl Env {
    /// The environment of these assumptions.
    pub fn new(assumptions: impl IntoIterator<Item = Assumption>) -> Self {
        Self(assumptions.into_iter().collect())
    }

    /// How many it holds.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// What it holds, in order.
    pub fn iter(&self) -> impl Iterator<Item = &Assumption> {
        self.0.iter()
    }

    /// Whether `assumption` is in this environment.
    pub fn has(&self, assumption: &Assumption) -> bool {
        self.0.contains(assumption)
    }

    /// Whether every assumption of this environment is in `other`.
    pub fn is_subset(&self, other: &Self) -> bool {
        self.0.is_subset(&other.0)
    }

    /// The assumptions of both.
    pub fn union(&self, other: &Self) -> Self {
        Self(self.0.union(&other.0).cloned().collect())
    }

    /// This environment without `assumption`.
    pub fn without(&self, assumption: &Assumption) -> Self {
        let mut rest = self.0.clone();
        rest.remove(assumption);
        Self(rest)
    }
}

/// A minimal antichain of environments: none is a subset of another.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Label(BTreeSet<Env>);

impl Label {
    /// The label of these environments, kept minimal.
    pub fn new(envs: impl IntoIterator<Item = Env>) -> Self {
        let mut label = Self::default();
        for env in envs {
            label.insert(env);
        }
        label
    }

    /// Keeps `env` and drops the stored supersets of it, unless a stored
    /// environment is a subset of it; whether it was kept.
    pub fn insert(&mut self, env: Env) -> bool {
        if self.0.iter().any(|stored| stored.is_subset(&env)) {
            return false;
        }
        self.0.retain(|stored| !env.is_subset(stored));
        self.0.insert(env);
        true
    }

    /// Whether `env` is stored.
    pub fn contains(&self, env: &Env) -> bool {
        self.0.contains(env)
    }

    /// Whether a stored environment is a subset of `env`: `env` holds the
    /// datum, or, for nogoods, is ruled out.
    pub fn covers(&self, env: &Env) -> bool {
        self.0.iter().any(|stored| stored.is_subset(env))
    }

    /// What it holds, in order.
    pub fn iter(&self) -> impl Iterator<Item = &Env> {
        self.0.iter()
    }

    /// How many it holds.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The label of both data holding: the unions of an environment of each,
    /// kept minimal.
    pub fn product(&self, other: &Self) -> Self {
        Self::new(
            self.0
                .iter()
                .flat_map(|a| other.0.iter().map(move |b| a.union(b))),
        )
    }
}

/// The minimal environments that cannot hold together. An environment with a
/// stored subset is never stored, so it is never reported again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Nogoods(Label);

impl Nogoods {
    /// Stores `env` and drops the stored supersets of it; whether it was
    /// stored. A nogood with a stored subset is refused, and so is the empty
    /// environment: it would rule out every environment, which says nothing
    /// about decisions.
    pub fn insert(&mut self, env: Env) -> bool {
        debug_assert!(!env.is_empty(), "the empty environment is no nogood");
        !env.is_empty() && self.0.insert(env)
    }

    /// Whether `env` is one of the nogoods.
    pub fn contains(&self, env: &Env) -> bool {
        self.0.contains(env)
    }

    /// Whether `env` holds a nogood, so cannot be consistent.
    pub fn rules_out(&self, env: &Env) -> bool {
        self.0.covers(env)
    }

    /// What it holds, in order.
    pub fn iter(&self) -> impl Iterator<Item = &Env> {
        self.0.iter()
    }

    /// How many it holds.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Why the hitting sets are not found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsolved {
    /// More decisions than [`MAX_DECISIONS`] flag the arrangements. The
    /// caller reports the cell as `basis: incomplete`, never as "no
    /// conflict".
    TooMany(usize),
    /// The shape has no arrangement, so nothing can be flagged.
    NoArrangement,
}

impl std::fmt::Display for Unsolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooMany(n) => write!(
                f,
                "{n} decisions flag the arrangements; at most {MAX_DECISIONS} are combined"
            ),
            Self::NoArrangement => f.write_str("no arrangement to flag"),
        }
    }
}

impl std::error::Error for Unsolved {}

/// Everything about a decision that changes which arrangements it flags, as
/// one digest: how its check is written, what it means (requirement,
/// severity, scope, option schema) and the effective options it runs with:
/// the `configured` ones resolved for `language` (defaults applied), so a
/// configuration that spells a default out is the same identity as one that
/// leaves it out. Fails as [`Decision::resolve_options`] does, on an option
/// the decision does not declare or a value of the wrong type.
/// The one definition of that identity: the cache of a witness search keys on
/// it too, so a stored answer and an assumption never disagree about what
/// changed.
pub fn digest(
    decision: &Decision,
    configured: &Map<String, Value>,
    language: Option<&str>,
) -> Result<String, Error> {
    let options = decision.resolve_options(configured, language)?;
    Ok(hash::short_digest(
        &json!({
            "check": decision.check_revision(),
            "meaning": decision.meaning_version(),
            "options": options,
        }),
        8,
    ))
}

/// Whether `env` flags every arrangement: it holds a decision of each
/// `flagged` (the decisions that flag one arrangement). The shape is then
/// unsatisfiable under `env`.
pub fn is_hitting(env: &Env, flagged: &[Env]) -> bool {
    flagged
        .iter()
        .all(|f| f.iter().any(|assumption| env.has(assumption)))
}

/// Whether `env` is a minimal witness: it flags every arrangement, and
/// removing any one decision leaves an arrangement unflagged.
pub fn is_minimal_hitting(env: &Env, flagged: &[Env]) -> bool {
    is_hitting(env, flagged)
        && env
            .iter()
            .all(|assumption| !is_hitting(&env.without(assumption), flagged))
}

/// The minimal sets of decisions that flag every arrangement: the label of
/// "every arrangement is flagged", the product, kept minimal, of the labels
/// `{{D} : D flags a}` of the arrangements. An arrangement no decision flags
/// leaves no set.
pub fn hitting_sets(flagged: &[Env]) -> Result<Label, Unsolved> {
    let decisions: BTreeSet<&Assumption> = flagged.iter().flat_map(Env::iter).collect();
    if decisions.len() > MAX_DECISIONS {
        return Err(Unsolved::TooMany(decisions.len()));
    }
    let mut labels = flagged
        .iter()
        .map(|f| Label::new(f.iter().map(|a| Env::new([a.clone()]))));
    let first = labels.next().ok_or(Unsolved::NoArrangement)?;
    Ok(labels.fold(first, |all, one| all.product(&one)))
}

/// Whether a member of `env` is superseded by another member: the conflict
/// is then settled by the order of the decisions, not a finding.
/// `supersedes(newer, older)` must hold through chains (the transitive
/// closure of the `supersedes` of the decisions): with A over B over C,
/// `{A, C}` is resolved.
pub fn is_resolved(env: &Env, supersedes: impl Fn(&Assumption, &Assumption) -> bool) -> bool {
    env.iter()
        .any(|a| env.iter().any(|b| a != b && supersedes(b, a)))
}
