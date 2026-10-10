use std::{fmt, str::FromStr};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Diagnostic;

/// The text given to a term's `from_str` names no such term.
#[derive(Debug, Error)]
#[error("unknown {what} `{text}` (expected {expected})")]
pub struct UnknownTerm {
    what: &'static str,
    text: String,
    expected: String,
}

impl UnknownTerm {
    fn new(what: &'static str, text: &str, expected: String) -> Self {
        Self {
            what,
            text: text.to_owned(),
            expected,
        }
    }
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
                    .ok_or_else(|| UnknownTerm::new(
                        $what,
                        s,
                        names(Self::ALL.iter().map(|v| v.as_str())),
                    ))
            }
        }
    };
}

/// A recorded label on one subject, as SARIF's `result.kind` says it: not
/// ground truth. `pass` and `fail` label the check's conformance part;
/// `notApplicable` labels its applicability part (the decision should not
/// apply here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum Judgment {
    /// The subject conforms: a finding about it was a false positive.
    #[serde(rename = "pass")]
    Pass,
    /// The subject violates the decision: a finding about it is right.
    #[serde(rename = "fail")]
    Fail,
    /// The decision does not apply to the subject.
    #[serde(rename = "notApplicable")]
    NotApplicable,
}

vocabulary!(Judgment, "judgment" {
    Pass => "pass",
    Fail => "fail",
    NotApplicable => "notApplicable",
});

/// How a suppression was declared, in the words of SARIF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum SuppressionKind {
    /// By a directive in the code.
    #[serde(rename = "inSource")]
    InSource,
    /// Outside the code: a recorded decision to leave the finding in place.
    #[serde(rename = "external")]
    External,
}

vocabulary!(SuppressionKind, "suppression kind" {
    InSource => "inSource",
    External => "external",
});

/// Whether a suppression is in force, in the words of SARIF.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum SuppressionStatus {
    /// In force.
    #[default]
    #[serde(rename = "accepted")]
    Accepted,
    /// Proposed, not yet in force.
    #[serde(rename = "underReview")]
    UnderReview,
    /// Refused.
    #[serde(rename = "rejected")]
    Rejected,
}

vocabulary!(SuppressionStatus, "suppression status" {
    Accepted => "accepted",
    UnderReview => "underReview",
    Rejected => "rejected",
});

impl SuppressionStatus {
    fn is_default(&self) -> bool {
        *self == Self::Accepted
    }
}

/// A finding that is right but is left in place on purpose: SARIF's
/// `suppression`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Suppression {
    pub kind: SuppressionKind,
    /// `accepted` unless said otherwise.
    #[serde(default, skip_serializing_if = "SuppressionStatus::is_default")]
    pub status: SuppressionStatus,
    /// Why the finding stays.
    pub justification: String,
}

impl Suppression {
    /// An accepted suppression a directive in the code declares.
    pub fn in_source(justification: impl Into<String>) -> Self {
        Self::new(SuppressionKind::InSource, justification)
    }

    /// An accepted suppression recorded outside the code.
    pub fn external(justification: impl Into<String>) -> Self {
        Self::new(SuppressionKind::External, justification)
    }

    /// An accepted suppression of the given kind.
    fn new(kind: SuppressionKind, justification: impl Into<String>) -> Self {
        Self {
            kind,
            status: SuppressionStatus::Accepted,
            justification: justification.into(),
        }
    }

    /// Whether the suppression is in force.
    pub fn in_force(&self) -> bool {
        self.status == SuppressionStatus::Accepted
    }
}

/// A finding a suppression keeps out of the report. A run states how many
/// there were, and SARIF keeps them with their suppression.
#[derive(Debug, Clone, PartialEq)]
pub struct Suppressed {
    pub diagnostic: Diagnostic,
    pub suppression: Suppression,
}

/// What kind of agent a record is attributed to, a W3C PROV class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum AgentKind {
    /// A human.
    Person,
    /// A program, such as a coding agent or a model.
    SoftwareAgent,
}

impl AgentKind {
    /// The PROV class: `Person` or `SoftwareAgent`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Person => "Person",
            Self::SoftwareAgent => "SoftwareAgent",
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Reads the PROV class, or the words `human` and `agent`, which the command
/// line and the environment take.
impl FromStr for AgentKind {
    type Err = UnknownTerm;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "Person" | "person" | "human" => Ok(Self::Person),
            "SoftwareAgent" | "softwareAgent" | "agent" => Ok(Self::SoftwareAgent),
            _ => Err(UnknownTerm::new(
                "reviewer kind",
                text,
                "agent, human, Person or SoftwareAgent".to_owned(),
            )),
        }
    }
}

/// `prov:wasAttributedTo`: who a record is attributed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Attribution {
    #[serde(rename = "type")]
    pub kind: AgentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// What a judgment teaches a model about the decision that raised the
/// finding. An unjudged finding has no label: it is not a negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Label {
    /// `fail`: the check was right.
    Positive,
    /// `pass` or `notApplicable`: the check was wrong.
    Negative,
    /// `fail` that is left in place on purpose (an external suppression): says
    /// nothing about precision, a target of its own.
    Separate,
}

vocabulary!(Label, "label" {
    Positive => "positive",
    Negative => "negative",
    Separate => "separate",
});

impl Label {
    /// The label of a judgment, whether an external suppression went with it.
    pub fn of(judgment: Judgment, suppressed: bool) -> Self {
        match (judgment, suppressed) {
            (Judgment::Fail, false) => Self::Positive,
            (Judgment::Fail, true) => Self::Separate,
            (Judgment::Pass | Judgment::NotApplicable, _) => Self::Negative,
        }
    }
}

pub(crate) fn names<'a>(terms: impl Iterator<Item = &'a str>) -> String {
    terms.collect::<Vec<_>>().join(", ")
}
