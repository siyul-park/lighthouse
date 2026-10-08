use std::collections::BTreeMap;

use lighthouse_model::Severity;
use lighthouse_resource::Spec;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{Decision, Error, Example, LanguageSpec};

/// The spec of the `DecisionOverride` kind: a project's adjustment of a
/// decision of a lower layer. `severity` and `exceptions` replace, `options`
/// replace defaults per option, `languages` replace per language and key, and
/// `examples` are appended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionOverrideSpec {
    /// Id of the decision adjusted.
    pub extends: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exceptions: Option<String>,
    /// New defaults, by option name.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub options: Map<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub languages: BTreeMap<String, LanguageSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Example>,
}

impl Spec for DecisionOverrideSpec {
    const KIND: &'static str = "DecisionOverride";
}

impl DecisionOverrideSpec {
    /// Applies the override to `decision`.
    pub(crate) fn apply(&self, decision: &mut Decision) -> Result<(), Error> {
        let spec = decision.spec_mut();
        if self.severity.is_some() {
            spec.severity = self.severity;
        }
        if self.exceptions.is_some() {
            spec.exceptions.clone_from(&self.exceptions);
        }
        for (language, change) in &self.languages {
            let into = spec.languages.entry(language.clone()).or_default();
            into.options.extend(change.options.clone());
            if change.tuning.is_some() {
                into.tuning.clone_from(&change.tuning);
            }
        }
        for (name, default) in &self.options {
            let property = spec
                .options
                .as_mut()
                .and_then(|schema| schema.properties.get_mut(name))
                .ok_or_else(|| {
                    Error::invalid(&self.extends, format!("extends an unknown option `{name}`"))
                })?;
            property.default = default.clone();
        }
        spec.examples.extend(self.examples.iter().cloned());
        Ok(())
    }
}
