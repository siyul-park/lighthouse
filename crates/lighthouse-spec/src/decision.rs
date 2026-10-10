use std::{collections::BTreeMap, ops::Deref};

use lighthouse_model::Severity;
use lighthouse_resource::{Metadata, Resource, Spec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{Check, Error, Example, Fix, OptionsSchema, Scope, Status};
use lighthouse_model::hash;

/// Label that says which pack a decision belongs to.
pub const PACK_LABEL: &str = "lighthouse/pack";
/// Label that says which section of its pack a decision is listed in.
pub const SECTION_LABEL: &str = "lighthouse/section";
/// Annotation a migrated decision keeps: the rule file its CEL check came from.
pub const MIGRATED_FROM: &str = "lighthouse/migrated-from";
/// Annotation of a decision that was a bespoke builtin rule before it became
/// a spec over standard operations: the id that rule had. Verdicts recorded
/// under the old rule keep applying while the decision means the same.
pub const WAS_BUILTIN: &str = "lighthouse/was-builtin";
/// Annotation of a decision whose `enforcement` (since replaced by `severity`)
/// cannot be told from its severity: `mechanical`, `heuristic`, `judgment` or
/// `doc`. It keeps the versions older verdicts were recorded under.
pub const WAS_ENFORCEMENT: &str = "lighthouse/was-enforcement";

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

/// A design decision, the way an architecture decision record states one:
/// what was decided and why, its status, and how it is enforced. The engine
/// compiles `check` into the rule that enforces it. A decision without a
/// `check` is documentation only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionSpec {
    pub title: String,
    pub intent: String,
    pub scope: Scope,
    /// What the decision demands, in RFC 2119 words.
    pub requirement: String,
    /// Where the decision stands: only an accepted one is enforced.
    #[serde(default, skip_serializing_if = "Status::is_default")]
    pub status: Status,
    /// The decisions this one replaces, by id. Each becomes `superseded`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supersedes: Vec<String>,
    /// What follows from the decision, good and bad.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consequences: Option<String>,
    /// The severity of the decision's findings, as authored. `error` is
    /// definitive: it needs no verdict and only an annotation in the code
    /// waives it, and its fix may be safe. `warn` and `info` are review
    /// tasks: a reviewer's verdict may hide them and a fix is at most
    /// suggested. A decision without a `check` has none. A configuration
    /// `level` changes what is reported and the exit code, never this.
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

impl Decision {
    /// The `enforcement` this decision had before severities replaced it:
    /// what its annotation recorded, else what its severity implies.
    pub(crate) fn was_enforcement(&self) -> &str {
        if let Some(recorded) = self.metadata().annotations.get(WAS_ENFORCEMENT) {
            return recorded;
        }
        match self.severity {
            None => "doc",
            Some(Severity::Error) => "mechanical",
            Some(Severity::Warn) => "heuristic",
            Some(Severity::Info) => "judgment",
        }
    }

    /// The semantic version a build between the resource model and severities
    /// computed for this decision: it hashed the `check` and the enforcement.
    /// Verdicts recorded then keep applying while the meaning is unchanged.
    ///
    /// That build never shipped outside development, so nothing real was
    /// recorded under this version; it is kept so that a store written while
    /// developing keeps working, and it is cheap.
    pub fn previous_semantic_version(&self) -> String {
        let check = match (self.metadata().annotations.get(WAS_BUILTIN), &self.check) {
            (Some(id), _) => json!({ "type": "builtin", "id": id }),
            (None, Some(check)) if check.is_automated() => json!(check),
            (None, _) => Value::Null,
        };
        let content = json!({
            "requirement": squash(&self.requirement),
            "enforcement": self.was_enforcement(),
            "scope": self.scope,
            "check": check,
            "options": self.options_content(),
        });
        hash::short(&content.to_string(), 8)
    }

    /// Every version older builds recorded verdicts under that still apply
    /// to this decision while its meaning is unchanged.
    pub fn earlier_versions(&self) -> Vec<String> {
        let mut versions = vec![self.previous_semantic_version()];
        versions.extend(self.legacy_semantic_version());
        versions.dedup();
        versions
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
    /// language sets). A verdict stays valid exactly while this does not
    /// change. How the decision is checked is not part of it, so a decision
    /// may move from review to a rule, or from one provider to another, and
    /// keep its verdicts. Wording, examples, tuning, option descriptions, the
    /// ADR fields, the envelope and the file format do not change it either.
    pub fn meaning_version(&self) -> String {
        hash::short(&self.meaning_content().to_string(), 8)
    }

    /// Identifies how the decision is checked: the hash of its `check`. It is
    /// recorded with findings for evaluation and never expires a verdict.
    pub fn check_revision(&self) -> String {
        hash::short(&json!(self.check).to_string(), 8)
    }

    fn meaning_content(&self) -> Value {
        json!({
            "requirement": squash(&self.requirement),
            "severity": self.severity,
            "scope": self.scope,
            "options": self.options_content(),
        })
    }

    fn options_content(&self) -> Value {
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
        json!({ "properties": properties, "languages": languages })
    }
}

pub(crate) fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_false(value: &bool) -> bool {
    !value
}
