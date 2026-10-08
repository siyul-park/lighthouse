use std::{collections::BTreeMap, ops::Deref};

use lighthouse_model::Severity;
use lighthouse_resource::{Metadata, Resource, Spec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{Check, Enforcement, Error, Example, Fix, OptionsSchema, Scope, model::short_hash};

/// Label that says which pack a decision belongs to.
pub const PACK_LABEL: &str = "lighthouse/pack";
/// Label that says which section of its pack a decision is listed in.
pub const SECTION_LABEL: &str = "lighthouse/section";
/// Annotation a migrated decision keeps: the rule file its CEL check came from.
pub const MIGRATED_FROM: &str = "lighthouse/migrated-from";

/// What one language changes about a decision.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LanguageSpec {
    /// Values of the decision's options that replace their defaults here.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub options: Map<String, Value>,
    /// Wording of the decision for this language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuning: Option<String>,
}

/// A design decision with its enforcement: what was decided and why, and how
/// the decision is checked and fixed. The engine compiles `check` into the
/// rule that enforces it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionSpec {
    pub title: String,
    pub intent: String,
    pub scope: Scope,
    /// What the decision demands, in RFC 2119 words.
    pub requirement: String,
    pub enforcement: Enforcement,
    /// Replaces the default severity of the enforcement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    /// Fields a checker emits as diagnostic evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exceptions: Option<String>,
    /// Tunable values, as a JSON Schema object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<OptionsSchema>,
    /// What each language changes: option values and wording, by language id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub languages: BTreeMap<String, LanguageSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Check>,
    /// How the findings of the rule are fixed; the catalog is the one place
    /// that says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation: Option<String>,
    /// Review-level advice that is too noisy for the `recommended` preset;
    /// the `strict` preset enables it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub strict: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Example>,
}

impl Spec for DecisionSpec {
    const KIND: &'static str = "Decision";
}

/// One catalog entry: a `Decision` document. `metadata.name` is its
/// `<pack>/<name>` id; the labels say where the catalog lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Decision(Resource<DecisionSpec>);

impl Decision {
    /// A decision with this identity and content.
    pub fn new(metadata: Metadata, spec: DecisionSpec) -> Self {
        Self(Resource::new(metadata, spec))
    }

    /// The `<pack>/<name>` id.
    pub fn id(&self) -> &str {
        &self.0.metadata.name
    }

    /// The identity and labels of the decision.
    pub fn metadata(&self) -> &Metadata {
        &self.0.metadata
    }

    /// What the decision says.
    pub fn spec(&self) -> &DecisionSpec {
        &self.0.spec
    }

    /// The pack the labels name, else the prefix of the id.
    pub fn pack(&self) -> &str {
        self.label(PACK_LABEL)
            .unwrap_or_else(|| self.id().split_once('/').map_or("", |(pack, _)| pack))
    }

    /// The section the labels name; empty when they name none.
    pub fn section(&self) -> &str {
        self.label(SECTION_LABEL).unwrap_or_default()
    }

    /// The name inside the pack: the id without its pack.
    pub fn short_name(&self) -> &str {
        self.id()
            .split_once('/')
            .map_or(self.id(), |(_, name)| name)
    }

    fn label(&self, key: &str) -> Option<&str> {
        self.0.metadata.labels.get(key).map(String::as_str)
    }

    /// The same decision changed by `change`.
    pub fn map_spec(self, change: impl FnOnce(DecisionSpec) -> DecisionSpec) -> Self {
        let Resource { metadata, spec } = self.0;
        Self::new(metadata, change(spec))
    }

    /// Identifies this wording of the decision: the hash of its definition.
    pub fn version(&self) -> String {
        let text = serde_json::to_string(&json!({ "name": self.id(), "spec": self.spec() }))
            .expect("a decision serializes");
        short_hash(&text)
    }

    /// Defaults filled in and configured keys checked against the declared
    /// options; `language` selects its option values over the defaults.
    pub fn resolve_options(
        &self,
        configured: &Map<String, Value>,
        language: Option<&str>,
    ) -> Result<Map<String, Value>, Error> {
        let id = self.id();
        let spec = self.spec();
        let empty = OptionsSchema::default();
        let schema = spec.options.as_ref().unwrap_or(&empty);
        for (key, value) in configured {
            let property = schema
                .properties
                .get(key)
                .ok_or_else(|| Error::invalid(id, format!("unknown option `{key}`")))?;
            if !property.kind.accepts(value) {
                return Err(Error::invalid(
                    id,
                    format!("option `{key}` must be {}", property.kind),
                ));
            }
        }
        let overrides = language.and_then(|l| spec.languages.get(l));
        let mut resolved = Map::new();
        for (key, property) in &schema.properties {
            let value = configured
                .get(key)
                .or_else(|| overrides.and_then(|l| l.options.get(key)))
                .unwrap_or(&property.default);
            resolved.insert(key.clone(), value.clone());
        }
        Ok(resolved)
    }

    pub(crate) fn spec_mut(&mut self) -> &mut DecisionSpec {
        &mut self.0.spec
    }

    pub(crate) fn metadata_mut(&mut self) -> &mut Metadata {
        &mut self.0.metadata
    }

    pub(crate) fn into_resource(self) -> Resource<DecisionSpec> {
        self.0
    }
}

impl Deref for Decision {
    type Target = DecisionSpec;

    fn deref(&self) -> &DecisionSpec {
        &self.0.spec
    }
}

impl DecisionSpec {
    /// The override if the decision has one, else the default of its
    /// enforcement; `None` for a `doc` decision.
    pub fn severity(&self) -> Option<Severity> {
        self.severity
            .or_else(|| self.enforcement.default_severity())
    }

    /// Identifies what the decision demands: the hash of its requirement,
    /// enforcement, scope, check and options (types and defaults, with the
    /// values each language sets). Wording, examples, tuning and option
    /// descriptions do not change it, so a verdict stays valid across edits
    /// that leave the decision alone. Nothing about the envelope, the labels
    /// or the file format is part of it.
    pub fn semantic_version(&self) -> String {
        short_hash(&self.semantic_content().to_string())
    }

    fn semantic_content(&self) -> Value {
        let empty = OptionsSchema::default();
        let schema = self.options.as_ref().unwrap_or(&empty);
        let properties: Map<String, Value> = schema
            .properties
            .iter()
            .map(|(name, p)| {
                (
                    name.clone(),
                    json!({ "type": p.kind, "default": p.default }),
                )
            })
            .collect();
        let languages: Map<String, Value> = self
            .languages
            .iter()
            .filter(|(_, l)| !l.options.is_empty())
            .map(|(id, l)| (id.clone(), Value::Object(l.options.clone())))
            .collect();
        let options = json!({ "properties": properties, "languages": languages });
        json!({
            "requirement": squash(&self.requirement),
            "enforcement": self.enforcement,
            "scope": self.scope,
            "check": self.check,
            "options": options,
        })
    }
}

pub(crate) fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_false(value: &bool) -> bool {
    !value
}
