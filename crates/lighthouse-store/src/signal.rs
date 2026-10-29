//! The records of session signals: what the hooks write, what they and the
//! commands read back.

use lighthouse_model::{Attribution, SignalClass, SignalStatus};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

/// A signal to record. Its `id` is the caller's, so that seeing the same
/// thing twice records it once.
#[derive(Debug, Clone)]
pub struct NewSignal {
    pub id: String,
    pub session: String,
    /// The hook event that saw it: `UserPromptSubmit`, `PostToolUse`, `Stop`.
    pub event: String,
    pub class: SignalClass,
    /// Scrubbed and short: never the whole message.
    pub excerpt: String,
    /// What was measured of it, never the text.
    pub features: Value,
    /// The classifier rule that fired, or the decision of a finding.
    pub rule: String,
    pub by: Attribution,
}

/// A recorded signal.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Signal {
    pub id: String,
    pub session: String,
    pub time: String,
    pub event: String,
    pub class: SignalClass,
    pub excerpt: String,
    pub features: Value,
    pub rule: String,
    /// Who it is attributed to: the person for what they said, the agent for
    /// what its edits caused, a model for what one classified.
    pub by: Attribution,
    #[serde(flatten)]
    pub status: SignalStatus,
    /// Who settled it, as `--attributed-to` spells it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_by: Option<String>,
    /// Whether a stop has asked the agent about it.
    pub asked: bool,
}

/// Which signals a listing reads.
#[derive(Debug, Clone, Default)]
pub struct SignalFilter {
    pub session: Option<String>,
    /// A status name: `open`, `recorded`, `linked` or `dismissed`.
    pub status: Option<String>,
    /// Only the signals no stop has asked about.
    pub unasked: bool,
    /// Only the classes that state a decision, not `none`, `abstain` or
    /// `finding`.
    pub said: bool,
}

/// A decision created as a proposal, and what the dedupe check found.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub decision_uid: String,
    pub decision: String,
    pub time: String,
    /// The decisions it was found to possibly duplicate.
    pub duplicates: Vec<String>,
}
