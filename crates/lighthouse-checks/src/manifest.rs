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
        applicability: decision.applicability(),
        caching: None,
    })
}

/// The options of `language` over the decision's defaults, with a failure
/// reported as the options error of this rule. The options a decision reads
/// from another (`options.from`) are in `configured`: the engine puts the ones
/// the project sets for the source at the file there.
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

/// Accepts the options a project sets on the decision: known keys of the
/// declared types, and none of those it reads from another decision, which are
/// set there.
pub(crate) fn validate_options(decision: &Decision, configured: &Options) -> Result<(), Error> {
    if let (Some(from), Some(schema)) = (decision.options_from(), decision.options.as_ref())
        && let Some(key) = schema
            .inherited
            .iter()
            .find(|k| configured.contains_key(*k))
    {
        return Err(Error::Options {
            rule: decision.id().to_owned(),
            message: format!("option `{key}` is read from `{from}`: set it there"),
        });
    }
    rule_options(decision, configured, None).map(drop)
}
