use std::{collections::BTreeMap, ops::Deref};

use lighthouse_model::Severity;
use lighthouse_resource::{Metadata, Resource, Spec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{Check, DecisionStatus, Error, Example, Fix, OptionsSchema, Scope};
use lighthouse_model::hash;

/// Label that says which pack a decision belongs to.
pub const PACK_LABEL: &str = "lighthouse/pack";
/// Label that says which section of its pack a decision is listed in.
pub const SECTION_LABEL: &str = "lighthouse/section";
/// Label that says which preset a decision joins beyond `recommended`; the
/// only value is `strict`.
pub const PRESET_LABEL: &str = "lighthouse/preset";
/// The value of [`PRESET_LABEL`] of a decision only the `strict` preset enables.
pub const STRICT: &str = "strict";

/// What one language changes about a decision: the values of its options.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LanguageSpec {
    /// Values of the decision's options that replace their defaults here.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub options: Map<String, Value>,
}

/// Where a decision comes from, in the vocabulary of W3C PROV.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Provenance {
    /// The sources the decision was derived from: a paper, a tool's rule, the
    /// lines of a document it covers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub was_derived_from: Vec<String>,
}

impl Provenance {
    /// Whether nothing is said about where the decision comes from.
    pub fn is_empty(&self) -> bool {
        self.was_derived_from.is_empty()
    }
}

/// A design decision, the way an architecture decision record states one:
/// what was decided and why, its status, and how it is enforced. The engine
/// compiles `check` into the rule that enforces it. A decision without a
/// `check` is documentation only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionSpec {
    pub title: String,
    /// Why the decision exists: the forces at play, as an architecture
    /// decision record's context states them.
    pub context: String,
    pub scope: Scope,
    /// What the decision demands, in RFC 2119 words.
    pub requirement: String,
    /// Where the decision stands: only an accepted one is enforced.
    #[serde(default, skip_serializing_if = "DecisionStatus::is_default")]
    pub status: DecisionStatus,
    /// The decisions this one replaces, by id. Each becomes `superseded`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supersedes: Vec<String>,
    /// What follows from the decision, good and bad.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consequences: Option<String>,
    /// The severity of the decision's findings, as authored. `error` is
    /// definitive: it needs no review and only a directive in the code
    /// waives it, and its fix may be safe. `warn` and `info` are review
    /// tasks: a reviewer's judgment may hide them and a fix is at most
    /// suggested. A decision without a `check` has none. A configuration
    /// `level` changes what is reported and the exit code, never this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
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
    /// Where the decision comes from.
    #[serde(default, skip_serializing_if = "Provenance::is_empty")]
    pub provenance: Provenance,
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

    /// Whether only the `strict` preset enables the decision: review-level
    /// advice that is too noisy for `recommended`.
    pub fn strict(&self) -> bool {
        self.label(PRESET_LABEL) == Some(STRICT)
    }

    /// The `<pack>/<name>` id.
    pub fn id(&self) -> &str {
        &self.0.metadata.name
    }

    /// The identity that outlives the id: a UUID v4 assigned once. `None` for
    /// a decision that was written without one.
    pub fn uid(&self) -> Option<&str> {
        self.0.metadata.uid.as_deref()
    }

    /// The same decision with this uid.
    pub fn with_uid(mut self, uid: impl Into<String>) -> Self {
        self.0.metadata.uid = Some(uid.into());
        self
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
        hash::short(&text, 8)
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
    /// The authored severity; `None` for a decision without a check.
    pub fn severity(&self) -> Option<Severity> {
        self.severity
    }

    /// Whether a program decides the decision's findings, as opposed to agent
    /// review (`judged`) or nothing (documentation).
    pub fn automated(&self) -> bool {
        self.check.as_ref().is_some_and(Check::is_automated)
    }

    /// Whether the decision is enforced: accepted, and it has a check.
    pub fn enforced(&self) -> bool {
        self.status.enforced() && self.check.is_some()
    }

    /// Identifies what the decision means: the hash of its requirement,
    /// severity, scope and options (types and defaults, with the values each
    /// language sets). A judgment stays valid exactly while this does not
    /// change. How the decision is checked is not part of it, so a decision
    /// may move from review to a rule, or from one provider to another, and
    /// keep its judgments. Wording, examples, option descriptions, the
    /// ADR fields, the envelope and the file format do not change it either.
    pub fn meaning_version(&self) -> String {
        hash::short(&self.meaning_content().to_string(), 8)
    }

    /// Identifies how the decision is checked: the hash of its `check`. It is
    /// recorded with findings for evaluation and never expires a judgment.
    pub fn check_revision(&self) -> String {
        hash::short(&json!(self.check).to_string(), 8)
    }

    fn meaning_content(&self) -> Value {
        json!({
            "requirement": squash(&self.requirement),
            "severity": self.severity,
            "scope": self.scope,
            "options": self.options_content(&str::to_owned),
        })
    }

    /// The options, by the names `name` gives them: their own, or the names
    /// they had before they were camelCase.
    pub(crate) fn options_content(&self, name: &dyn Fn(&str) -> String) -> Value {
        let empty = OptionsSchema::default();
        let schema = self.options.as_ref().unwrap_or(&empty);
        let properties: Map<String, Value> = schema
            .properties
            .iter()
            .map(|(n, p)| (name(n), json!({ "type": p.kind, "default": p.default })))
            .collect();
        let languages: Map<String, Value> = self
            .languages
            .iter()
            .filter(|(_, l)| !l.options.is_empty())
            .map(|(id, l)| {
                let renamed: Map<String, Value> = l
                    .options
                    .iter()
                    .map(|(k, v)| (name(k), v.clone()))
                    .collect();
                (id.clone(), Value::Object(renamed))
            })
            .collect();
        json!({ "properties": properties, "languages": languages })
    }
}

pub(crate) fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
