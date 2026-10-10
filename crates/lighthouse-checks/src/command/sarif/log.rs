//! The part of a SARIF 2.1.0 log a `command` check reads. Only what turns a
//! result into a finding is modelled; every field may be absent, as the
//! tools that write logs differ in what they say.

use std::collections::BTreeMap;

use serde::Deserialize;

/// The only version of the format this reads.
pub(super) const VERSION: &str = "2.1.0";

#[derive(Debug, Deserialize)]
pub(super) struct Log {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub runs: Vec<Run>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Run {
    #[serde(default)]
    pub tool: Tool,
    #[serde(default)]
    pub original_uri_base_ids: BTreeMap<String, ArtifactLocation>,
    pub column_kind: Option<String>,
    #[serde(default)]
    pub results: Vec<SarifResult>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct Tool {
    #[serde(default)]
    pub driver: Driver,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct Driver {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub rules: Vec<ReportingDescriptor>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReportingDescriptor {
    #[serde(default)]
    pub id: String,
    pub help_uri: Option<String>,
    #[serde(default)]
    pub message_strings: BTreeMap<String, MessageString>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct MessageString {
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SarifResult {
    pub rule_id: Option<String>,
    pub rule_index: Option<usize>,
    pub level: Option<String>,
    #[serde(default)]
    pub message: Message,
    #[serde(default)]
    pub locations: Vec<Location>,
    #[serde(default)]
    pub suppressions: Vec<Suppression>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct Message {
    pub text: Option<String>,
    pub id: Option<String>,
    #[serde(default)]
    pub arguments: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Location {
    pub physical_location: Option<PhysicalLocation>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PhysicalLocation {
    pub artifact_location: Option<ArtifactLocation>,
    pub region: Option<Region>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ArtifactLocation {
    pub uri: Option<String>,
    pub uri_base_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Region {
    pub start_line: Option<u32>,
    pub start_column: Option<u32>,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct Suppression {
    pub status: Option<String>,
    pub justification: Option<String>,
}

impl Suppression {
    /// Whether the suppression is in force: `accepted`, or no status.
    pub(super) fn in_force(&self) -> bool {
        self.status.as_deref().is_none_or(|s| s == "accepted")
    }
}

impl Run {
    /// The rule a result is about: by `ruleIndex`, else by `ruleId`.
    pub(super) fn rule(&self, result: &SarifResult) -> Option<&ReportingDescriptor> {
        let rules = &self.tool.driver.rules;
        result.rule_index.and_then(|at| rules.get(at)).or_else(|| {
            rules
                .iter()
                .find(|r| Some(&r.id) == result.rule_id.as_ref())
        })
    }
}
