use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The text given to a review term's `from_str` names no such term.
#[derive(Debug, Error)]
#[error("unknown {what} `{text}` (expected {expected})")]
pub struct UnknownTerm {
    what: &'static str,
    text: String,
    expected: String,
}

/// A verdict and a reason that do not belong together.
#[derive(Debug, Error)]
#[error("reason `{reason}` does not fit verdict `{verdict}` (expected {expected})")]
pub struct MismatchedReason {
    verdict: Verdict,
    reason: Reason,
    expected: String,
}

macro_rules! vocabulary {
    ($ty:ident, $what:literal { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $ty {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The text the variant is stored, printed and parsed as.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $ty {
            type Err = UnknownTerm;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|v| v.as_str() == s)
                    .ok_or_else(|| UnknownTerm {
                        what: $what,
                        text: s.to_owned(),
                        expected: names(Self::ALL.iter().map(|v| v.as_str())),
                    })
            }
        }
    };
}

/// What a reviewer decided about a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// The finding is right.
    Confirmed,
    /// The finding is wrong or not wanted; it is suppressed from then on.
    Rejected,
    /// Not decided yet; the finding stays visible.
    Deferred,
}

vocabulary!(Verdict, "verdict" {
    Confirmed => "confirmed",
    Rejected => "rejected",
    Deferred => "deferred",
});

impl Verdict {
    /// The reasons a verdict may carry; `Reason::Unspecified` is allowed
    /// where listed, and required of `Deferred`.
    pub fn reasons(self) -> &'static [Reason] {
        match self {
            Self::Confirmed => &[Reason::Fixed, Reason::AcceptedDebt, Reason::Unspecified],
            Self::Rejected => &[
                Reason::FalsePositive,
                Reason::IntentionalException,
                Reason::ScopeTooBroad,
                Reason::ProjectAllowed,
                Reason::NotWorthFixing,
            ],
            Self::Deferred => &[Reason::Unspecified],
        }
    }

    /// Checks that `reason` belongs to this verdict.
    pub fn validate(self, reason: Reason) -> Result<(), MismatchedReason> {
        if self.reasons().contains(&reason) {
            return Ok(());
        }
        Err(MismatchedReason {
            verdict: self,
            reason,
            expected: names(self.reasons().iter().map(|r| r.as_str())),
        })
    }
}

/// Why a reviewer reached a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Reason {
    /// Confirmed and the code was changed.
    #[serde(rename = "fixed")]
    Fixed,
    /// Confirmed and left as known debt.
    #[serde(rename = "accepted-debt")]
    AcceptedDebt,
    /// The rule misjudged this code.
    #[serde(rename = "false-positive")]
    FalsePositive,
    /// The code is right to break the rule here.
    #[serde(rename = "intentional-exception")]
    IntentionalException,
    /// The rule matches more than it should; a hint to narrow the rule.
    #[serde(rename = "scope-too-broad")]
    ScopeTooBroad,
    /// The project's conventions allow it.
    #[serde(rename = "project-allowed")]
    ProjectAllowed,
    /// True but not worth the change.
    #[serde(rename = "not-worth-fixing")]
    NotWorthFixing,
    /// No reason given: what a deferred verdict carries.
    #[serde(rename = "none")]
    Unspecified,
}

vocabulary!(Reason, "reason" {
    Fixed => "fixed",
    AcceptedDebt => "accepted-debt",
    FalsePositive => "false-positive",
    IntentionalException => "intentional-exception",
    ScopeTooBroad => "scope-too-broad",
    ProjectAllowed => "project-allowed",
    NotWorthFixing => "not-worth-fixing",
    Unspecified => "none",
});

/// Who reviewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewerKind {
    Agent,
    Human,
}

vocabulary!(ReviewerKind, "reviewer kind" {
    Agent => "agent",
    Human => "human",
});

/// What a verdict teaches a model about the rule that raised the finding.
/// An unreviewed finding has no label: it is not a negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Label {
    /// Confirmed: the rule was right.
    Positive,
    /// Rejected as a false positive or as too broad: the rule was wrong.
    Negative,
    /// Rejected for a reason that says nothing about precision (an exception,
    /// a project convention, not worth fixing): a target of its own.
    Separate,
    /// Deferred: undecided.
    Unlabeled,
}

vocabulary!(Label, "label" {
    Positive => "positive",
    Negative => "negative",
    Separate => "separate",
    Unlabeled => "unlabeled",
});

impl Label {
    /// The label of a verdict with its reason.
    pub fn of(verdict: Verdict, reason: Reason) -> Self {
        match (verdict, reason) {
            (Verdict::Confirmed, _) => Self::Positive,
            (Verdict::Rejected, Reason::FalsePositive | Reason::ScopeTooBroad) => Self::Negative,
            (Verdict::Rejected, _) => Self::Separate,
            (Verdict::Deferred, _) => Self::Unlabeled,
        }
    }
}

fn names<'a>(terms: impl Iterator<Item = &'a str>) -> String {
    terms.collect::<Vec<_>>().join(", ")
}
