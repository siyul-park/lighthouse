//! What a decision tells the plugin registry about its rule: the manifest
//! and the options a run resolves for it.

use lighthouse_model::Options;
use lighthouse_plugin::{Error, RuleManifest};
use lighthouse_spec::{CheckKind, Decision};
use serde_json::{Map, Value};

/// Rule metadata of a checked decision; `None` while no program checks it. A
/// `model` check is run by the rule that puts its subjects to the models, so
/// it has a manifest too, but it is not automated: presets leave it off.
/// Analyzers and capabilities belong to the check.
pub(crate) fn rule_manifest(decision: &Decision) -> Option<RuleManifest> {
    decision
        .check
        .as_ref()
        .filter(|c| c.kind.deterministic() || matches!(c.kind, CheckKind::Model(_)))?;
    Some(RuleManifest {
        id: decision.id().to_owned(),
        uid: decision.uid().map(str::to_owned),
        severity: decision.severity?,
        scope: decision.scope.subject.run_scope(),
        description: decision.title.clone(),
        docs: decision.requirement.clone(),
        analyzers: Vec::new(),
        capabilities: Vec::new(),
        applicability: decision.scope.applicability(),
        caching: None,
    })
}

/// The options of `language` over the decision's defaults, with a failure
/// reported as the options error of this rule.
pub(crate) fn rule_options(
    decision: &Decision,
    configured: &Options,
    language: Option<&str>,
) -> Result<Map<String, Value>, Error> {
    decision
        .resolve_options(configured, language)
        .map_err(|e| Error::Options {
            rule: decision.id().to_owned(),
            message: e.to_string(),
        })
}
