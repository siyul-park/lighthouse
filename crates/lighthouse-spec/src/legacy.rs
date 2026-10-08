//! The semantic version of a decision as builds before the resource model
//! computed it. Verdicts already recorded carry that value; a decision whose
//! content has not changed since must keep honoring them, so the store is
//! given both.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    Check, Decision, DecisionSpec, OptionType,
    decision::{MIGRATED_FROM, squash},
    model::short_hash,
};

/// The names the pattern format gave the option types, with their types.
const TYPE_NAMES: [(&str, OptionType); 5] = [
    ("int", OptionType::Integer),
    ("float", OptionType::Number),
    ("bool", OptionType::Boolean),
    ("string", OptionType::String),
    ("list", OptionType::Array),
];

/// An option as the pattern format declared it.
#[derive(Serialize)]
struct LegacyOption<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    default: &'a Value,
    description: &'a str,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    per_language: BTreeMap<&'a str, &'a Value>,
}

impl Decision {
    /// The semantic version a build from before the resource model computed
    /// for this decision's content: the hash of its requirement, enforcement,
    /// options with their per-language values and implementation. `None`
    /// when the decision has an implementation that never existed in that
    /// format: a CEL check that was not migrated from a rule file.
    pub fn legacy_semantic_version(&self) -> Option<String> {
        let spec: &DecisionSpec = self;
        let implementation = match &spec.check {
            None => Value::Null,
            Some(Check::Builtin(builtin)) => json!({ "builtin": builtin.id }),
            Some(Check::Cel(_)) => {
                json!({ "declarative": self.metadata().annotations.get(MIGRATED_FROM)? })
            }
        };
        let options: BTreeMap<&str, LegacyOption> = spec
            .options
            .iter()
            .flat_map(|schema| &schema.properties)
            .map(|(name, p)| {
                let per_language = spec
                    .languages
                    .iter()
                    .filter_map(|(lang, l)| Some((lang.as_str(), l.options.get(name)?)))
                    .collect();
                let option = LegacyOption {
                    kind: legacy_type(p.kind),
                    default: &p.default,
                    description: &p.description,
                    per_language,
                };
                (name.as_str(), option)
            })
            .collect();
        let decision = json!({
            "requirement": squash(&spec.requirement),
            "enforcement": spec.enforcement,
            "options": options,
            "implementation": implementation,
        });
        Some(short_hash(&decision.to_string()))
    }
}

/// The option type the pattern format called `name`.
pub(crate) fn from_legacy_type(name: &str) -> Option<OptionType> {
    TYPE_NAMES
        .iter()
        .find_map(|(legacy, kind)| (*legacy == name).then_some(*kind))
}

fn legacy_type(kind: OptionType) -> &'static str {
    TYPE_NAMES
        .iter()
        .find_map(|(legacy, k)| (*k == kind).then_some(*legacy))
        .expect("every option type has a legacy name")
}
