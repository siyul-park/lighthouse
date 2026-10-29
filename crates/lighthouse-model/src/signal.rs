//! The vocabulary of session signals: what a person's message or an edit's
//! findings told Lighthouse about a design decision, and where each stands.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a signal says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SignalClass {
    /// A rule to follow: "always", "never", "must", "don't".
    Directive,
    /// A turn away from what the agent did or chose: "no, …", "instead".
    Correction,
    /// A stated taste: "X is better", "I prefer".
    Preference,
    /// A finding the session introduced or resolved.
    Finding,
    /// A message a rule proved says no decision (an acknowledgement).
    None,
    /// A message no rule decides; a bound model could.
    Abstain,
}

impl SignalClass {
    /// The classes that state a design decision, which are worth surfacing.
    pub const SAID: [Self; 3] = [Self::Directive, Self::Correction, Self::Preference];
}

vocabulary!(SignalClass, "signal class" {
    Directive => "directive",
    Correction => "correction",
    Preference => "preference",
    Finding => "finding",
    None => "none",
    Abstain => "abstain",
});

/// Where a signal stands: open, or settled by recording a decision, linking
/// an existing one, or dismissing it with a reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum SignalStatus {
    Open,
    /// A decision was recorded from it: the uid of that decision.
    Recorded {
        decision: String,
    },
    /// An existing decision already says it: its uid.
    Linked {
        decision: String,
    },
    Dismissed {
        reason: String,
    },
}

impl SignalStatus {
    /// The status name as the store keeps it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Recorded { .. } => "recorded",
            Self::Linked { .. } => "linked",
            Self::Dismissed { .. } => "dismissed",
        }
    }

    /// The decision uid or the reason that goes with the status.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Open => None,
            Self::Recorded { decision } | Self::Linked { decision } => Some(decision),
            Self::Dismissed { reason } => Some(reason),
        }
    }

    /// The status from the name and detail the store keeps.
    pub fn from_parts(name: &str, detail: Option<String>) -> Option<Self> {
        let detail = detail.unwrap_or_default();
        Some(match name {
            "open" => Self::Open,
            "recorded" => Self::Recorded { decision: detail },
            "linked" => Self::Linked { decision: detail },
            "dismissed" => Self::Dismissed { reason: detail },
            _ => return None,
        })
    }
}
