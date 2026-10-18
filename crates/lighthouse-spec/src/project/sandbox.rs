//! What a project says about the sandbox its programs run in. It is
//! environment: any layer may set it, and trust covers what it says.

use lighthouse_process::Policy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The `sandbox` table of a project: every program Lighthouse runs for it
/// (command checks, fixers, formatters, `[fix] verify`, agents and command
/// providers) runs under the OS sandbox. It reads everything but the secrets
/// of the user and the project, writes only its scratch place, and reaches no
/// network but the hosts listed here and in its provider.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SandboxSpec {
    /// Whether programs run in the sandbox. Default: `true`. `false` is an
    /// error unless the project is trusted (`lighthouse trust`), and every
    /// run says that programs run with your full access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// The hosts that command checks and `[fix] verify` commands may reach,
    /// through the proxy of the run: a name, or `*.suffix` for every host
    /// below it, either with `:port`. A provider lists its own. Default: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<Vec<String>>,
}

impl SandboxSpec {
    /// Whether the project says nothing about the sandbox.
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none() && self.network.is_none()
    }

    /// These keys with the keys `later` sets taking their place.
    pub(super) fn over(&mut self, later: &Self) {
        self.enabled = later.enabled.or(self.enabled);
        self.network = later.network.clone().or(self.network.take());
    }
}

impl From<SandboxSpec> for Policy {
    fn from(spec: SandboxSpec) -> Self {
        Self {
            enabled: spec.enabled.unwrap_or(true),
            network: spec.network.unwrap_or_default(),
        }
    }
}
