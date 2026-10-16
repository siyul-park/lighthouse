use std::{fmt, str::FromStr};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::UnknownTerm;

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

/// The role a software agent had in a record, PROV's `hadRole`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum Role {
    /// A coding agent that reviewed the finding.
    #[serde(rename = "agent")]
    Agent,
    /// A model that classified the subject.
    #[serde(rename = "model")]
    Model,
}

vocabulary!(Role, "role" {
    Agent => "agent",
    Model => "model",
});

/// `prov:wasAttributedTo`: who a record is attributed to, with the role the
/// agent had (`prov:hadRole`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Attribution {
    #[serde(rename = "type")]
    pub kind: AgentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "hadRole", default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

impl Attribution {
    /// A person, or a coding agent: a software agent is one unless it says
    /// otherwise.
    pub fn new(kind: AgentKind, id: Option<String>) -> Self {
        let role = (kind == AgentKind::SoftwareAgent).then_some(Role::Agent);
        Self { kind, id, role }
    }

    /// A model, as `<provider>/<model>@<version>`.
    pub fn model(id: impl Into<String>) -> Self {
        Self {
            kind: AgentKind::SoftwareAgent,
            id: Some(id.into()),
            role: Some(Role::Model),
        }
    }

    /// How much a judgment by this agent weighs against another's on the same
    /// subject: a person over a coding agent over a model.
    pub fn strength(&self) -> u8 {
        match (self.kind, self.role) {
            (AgentKind::Person, _) => 3,
            (AgentKind::SoftwareAgent, Some(Role::Model)) => 1,
            (AgentKind::SoftwareAgent, _) => 2,
        }
    }
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
    pub fn new(judgment: Judgment, suppressed: bool) -> Self {
        match (judgment, suppressed) {
            (Judgment::Fail, false) => Self::Positive,
            (Judgment::Fail, true) => Self::Separate,
            (Judgment::Pass | Judgment::NotApplicable, _) => Self::Negative,
        }
    }
}
