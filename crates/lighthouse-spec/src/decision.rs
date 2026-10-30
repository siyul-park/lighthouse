use std::{collections::BTreeMap, ops::Deref};

use lighthouse_model::{Safety, Severity, squash, ucm::Applicability};
use lighthouse_resource::{Metadata, Resource, Spec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    Catalog, Check, CheckKind, Error, Example, ExampleKind, Fix, OptionsSchema, Program, Scope,
};
use lighthouse_protocol::{DecisionRef, hash};

/// Where a decision stands in its life, as an architecture decision record
/// does. Only an accepted decision is enforced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DecisionStatus {
    /// Under discussion: written down, not yet enforced.
    Proposed,
    #[default]
    Accepted,
    /// Considered and turned down; kept so the question is not asked again.
    Rejected,
    /// Replaced by another decision (named in that decision's `supersedes`).
    Superseded,
    /// No longer wanted, and nothing replaces it.
    Deprecated,
}

impl DecisionStatus {
    /// Whether decisions in this state are enforced.
    pub fn enforced(self) -> bool {
        self == Self::Accepted
    }

    /// Whether this is the default, left out of written files.
    pub fn is_default(&self) -> bool {
        *self == Self::Accepted
    }
}

impl std::fmt::Display for DecisionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Proposed => "proposed",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Superseded => "superseded",
            Self::Deprecated => "deprecated",
        })
    }
}

/// What one language changes about a decision: the values of its options and
/// the programs that check and fix its files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LanguageSpec {
    /// Values of the decision's options that replace their defaults here.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub options: Map<String, Value>,
    /// The program that checks the files of this language, in place of the
    /// decision's own `command` check, which it needs to be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Program>,
    /// The program that fixes the findings in files of this language, in
    /// place of the decision's own `command` fix, which it needs to be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<Program>,
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
    /// unsafe. A decision without a `check` has none. A configuration
    /// `level` changes what is reported and the exit code, never this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    /// Tunable values, as a JSON Schema object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<OptionsSchema>,
    /// The languages the decision applies to, by language id, with what each
    /// changes: option values and the programs of a command. Absent or empty,
    /// it applies to every language; otherwise to exactly these, and `{}` lists
    /// a language that changes nothing.
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
/// `<pack>/<name>` id; the pack file lists it in a section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Decision(Resource<DecisionSpec>);

impl Decision {
    /// A decision with this identity and content.
    pub fn new(metadata: Metadata, spec: DecisionSpec) -> Self {
        Self(Resource::new(metadata, spec))
    }

    /// The decision as a model is told about it: its requirement, what the check
    /// asks (the `prompt`, else the requirement) and its examples as shots.
    pub fn reference(&self) -> DecisionRef {
        let prompt = self.check.as_ref().and_then(|check| match &check.kind {
            CheckKind::Cel(cel) => cel.prompt.clone(),
            CheckKind::Model(model) => model.prompt.clone(),
            _ => None,
        });
        let shots = |kind: ExampleKind| -> Vec<lighthouse_protocol::Example> {
            self.examples
                .iter()
                .filter(|example| example.kind == kind)
                .map(|example| lighthouse_protocol::Example {
                    language: example.language.clone(),
                    code: example
                        .files
                        .iter()
                        .map(|f| format!("// {}\n{}", f.path, f.text()))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    note: example.name.clone(),
                })
                .collect()
        };
        DecisionRef {
            id: self.id().to_owned(),
            requirement: self.requirement.clone(),
            prompt: prompt.unwrap_or_else(|| self.requirement.clone()),
            valid: shots(ExampleKind::Valid),
            invalid: shots(ExampleKind::Invalid),
        }
    }

    /// Whether this is a decision of the bundled catalog as it ships: the
    /// programs it names are Lighthouse's own, and run without the trust a
    /// program of the project or of a plugin needs.
    pub fn bundled(&self, shipped: &Catalog) -> bool {
        shipped
            .decision(self.id())
            .is_some_and(|shipped| shipped == self)
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

    /// How the decision log names the decision: its uid, else its id.
    pub fn key(&self) -> &str {
        self.uid().unwrap_or_else(|| self.id())
    }

    /// Whether `key` names the decision: its uid or its id.
    pub fn is_key(&self, key: &str) -> bool {
        self.uid() == Some(key) || self.id() == key
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

    /// The pack the decision belongs to: the prefix of its id.
    pub fn pack(&self) -> &str {
        self.id().split_once('/').map_or("", |(pack, _)| pack)
    }

    /// The name inside the pack: the id without its pack.
    pub fn short_name(&self) -> &str {
        self.id()
            .split_once('/')
            .map_or(self.id(), |(_, name)| name)
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

    /// Whether the decision applies to code in `language`: it lists no
    /// language, or lists this one.
    pub fn applies_to(&self, language: &str) -> bool {
        self.languages.is_empty() || self.languages.contains_key(language)
    }

    /// What the decision's subjects may be: its scope's code, in the languages
    /// it applies to.
    pub fn applicability(&self) -> Applicability {
        self.scope
            .applicability()
            .only(self.languages.keys().cloned())
    }

    /// The id of the decision whose options this one reads (`options.from`).
    pub fn options_from(&self) -> Option<&str> {
        self.options.as_ref()?.from.as_deref()
    }

    /// The same decision over the options of `source`: the options `source`
    /// declares and this does not, with their defaults and the values the
    /// languages of `source` set, are this decision's too, and listed in
    /// `options.inherited`; the languages it applies to stay its own.
    #[must_use]
    pub fn inheriting(&self, source: &Decision) -> Self {
        let mut spec = self.spec().clone();
        let schema = spec.options.get_or_insert_with(OptionsSchema::default);
        let own: Vec<String> = schema.properties.keys().cloned().collect();
        if let Some(from) = &source.options {
            for (key, property) in &from.properties {
                if !own.contains(key) {
                    schema.properties.insert(key.clone(), property.clone());
                    schema.inherited.push(key.clone());
                }
            }
        }
        for (language, source_language) in &source.languages {
            for (key, value) in &source_language.options {
                if schema.inherited.contains(key) {
                    schema
                        .inherited_languages
                        .entry(language.clone())
                        .or_default()
                        .insert(key.clone(), value.clone());
                }
            }
        }
        Self::new(self.metadata().clone(), spec)
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
            property
                .check(value, &format!("option `{key}`"))
                .map_err(|problem| Error::invalid(id, problem))?;
        }
        let own = language
            .and_then(|l| spec.languages.get(l))
            .map(|l| &l.options);
        let inherited = language.and_then(|l| schema.inherited_languages.get(l));
        let mut resolved = Map::new();
        for (key, property) in &schema.properties {
            let layers = [
                own.and_then(|options| options.get(key)),
                inherited.and_then(|options| options.get(key)),
                configured.get(key),
            ];
            let value = layers
                .into_iter()
                .flatten()
                .fold(property.default.clone(), |base, over| {
                    crate::options::merge(&base, over)
                });
            resolved.insert(key.clone(), value);
        }
        Ok(resolved)
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
    /// The widest safety a fix of this decision can have, whoever makes the
    /// fix: the one place that says. A fix is `safe` only when a program makes
    /// it and the decision authors `error`, and then as far as its `safety`
    /// allows; every other fix, a model's among them, is `unsafe`. A
    /// provider's claim can make it `unsafe`, never `safe` again. The
    /// `configured` safety of the project (`[rules."<id>"] safety`) is
    /// applied last and replaces all of that, as ruff's `extend-safe-fixes`
    /// does.
    pub fn fix_safety(&self, configured: Option<Safety>) -> Safety {
        configured.unwrap_or_else(|| match &self.fix {
            Some(fix) if fix.deterministic() && self.severity == Some(Severity::Error) => {
                fix.safety
            }
            _ => Safety::Unsafe,
        })
    }

    /// Whether a program decides the decision's findings, as opposed to agent
    /// review (`judged`) or nothing (documentation).
    pub fn automated(&self) -> bool {
        self.check.as_ref().is_some_and(|c| c.kind.deterministic())
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
            "options": self.options_content(),
        })
    }

    /// The option types and defaults, and the values each language sets.
    fn options_content(&self) -> Value {
        let empty = OptionsSchema::default();
        let schema = self.options.as_ref().unwrap_or(&empty);
        let properties: Map<String, Value> = schema
            .properties
            .iter()
            .map(|(name, p)| (name.clone(), option_content(p)))
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

/// The severity a finding's decision authored, else the severity the finding
/// has: what decides whether a judgment may hide it and a fix may be safe.
pub fn authored_severity(severity: Severity, decision: Option<&Decision>) -> Severity {
    decision.and_then(|d| d.severity).unwrap_or(severity)
}

/// What an option contributes to the meaning: its type and default, and the
/// alternatives or named shape a plain type does not say.
fn option_content(property: &crate::OptionSchema) -> Value {
    let mut content = json!({ "type": property.kind, "default": property.default });
    if !property.one_of.is_empty() || property.reference.is_some() {
        content["shape"] = json!(property.shape);
    }
    content
}
