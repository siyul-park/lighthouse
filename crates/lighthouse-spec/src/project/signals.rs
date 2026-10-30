//! The `signals` table of a project: how long the session signals the hooks
//! record are kept, and whether the agent is told about them.

use std::time;

use lighthouse_model::Duration;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How long signals are kept when the project does not say: 30 days, the
/// retention of ECC's observations.
const DEFAULT_RETENTION: time::Duration = time::Duration::from_secs(30 * 24 * 60 * 60);

/// Whether the agent hears about the signals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    /// Signals are recorded and counted; the agent is told nothing.
    #[default]
    Shadow,
    /// The hooks tell the agent about design decisions, open signals and the
    /// decisions of the project.
    On,
}

/// The `signals` table of a project.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SignalsSpec {
    /// How long a signal is kept, such as `30d`; older ones are purged when a
    /// new one is written. Default: `30d`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<Duration>,
    /// `shadow` records signals and prints nothing to the agent; `on` tells
    /// it. Default: `shadow`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<Surface>,
}

impl SignalsSpec {
    /// Whether the project says nothing about signals.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// How long a signal is kept.
    pub fn retention(&self) -> time::Duration {
        self.retention.map_or(DEFAULT_RETENTION, Duration::get)
    }

    /// Whether the agent hears about signals.
    pub fn surface(&self) -> Surface {
        self.surface.unwrap_or_default()
    }
}
