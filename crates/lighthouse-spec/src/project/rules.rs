use std::collections::BTreeMap;

use lighthouse_model::{Options, Severity};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;

use super::models::{Evaluation, Names};

/// Rule id to its configured level and options.
pub type Rules = BTreeMap<String, RuleConfig>;

/// What a project says about one rule: a level, and options for it.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleConfig {
    /// `None` disables the rule.
    pub level: Option<Severity>,
    pub options: Options,
    /// Whether generated code is checked by this rule; `None` leaves it to
    /// the project's `generated.check` and the decision's scope.
    pub generated: Option<bool>,
    /// The models that judge this decision, before the project's
    /// `models.classification`.
    pub classification: Option<Vec<String>>,
    /// How `lighthouse eval` judges this rule's check.
    pub evaluation: Option<Evaluation>,
}

impl RuleConfig {
    /// A rule at `level`, with nothing else said.
    pub fn new(level: Option<Severity>) -> Self {
        Self {
            level,
            options: Options::new(),
            generated: None,
            classification: None,
            evaluation: None,
        }
    }
}

impl From<RuleSetting> for RuleConfig {
    fn from(setting: RuleSetting) -> Self {
        match setting {
            RuleSetting::Level(level) => Self::new(level.into()),
            RuleSetting::Detailed(detail) => Self {
                options: detail.options,
                generated: detail.generated,
                classification: detail
                    .classification
                    .map(|names| names.iter().map(str::to_owned).collect()),
                evaluation: detail.evaluation,
                ..Self::new(detail.level.into())
            },
        }
    }
}

/// A level a project can set: a severity, or `off`, which only
/// configuration has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warn,
    Info,
    Off,
}

/// How a rule is written in a spec: `warn`, `off`, or
/// `{ level: warn, options: { max: 10 } }`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum RuleSetting {
    Level(Level),
    Detailed(RuleDetail),
}

impl<'de> Deserialize<'de> for RuleSetting {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::String(text) => level_of(&text).map(Self::Level).map_err(de::Error::custom),
            table @ Value::Object(_) => serde_json::from_value(table)
                .map(Self::Detailed)
                .map_err(de::Error::custom),
            _ => Err(de::Error::custom(
                "a rule is a level string or a table with `level`",
            )),
        }
    }
}

/// A level with options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleDetail {
    #[serde(deserialize_with = "level_field")]
    pub level: Level,
    /// The decision's options, as its own schema declares them.
    #[serde(default, skip_serializing_if = "Options::is_empty")]
    pub options: Options,
    /// Check generated code with this rule (`true`) or not (`false`),
    /// whatever the decision's scope and the project's `generated.check` say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<bool>,
    /// The model, or the route of several, that judges this decision, before
    /// the project's `models.classification`. When it abstains, the subject is
    /// left to agent review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Names>,
    /// How `lighthouse eval` judges this rule's check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluation: Option<Evaluation>,
}

impl From<Level> for Option<Severity> {
    fn from(level: Level) -> Self {
        match level {
            Level::Error => Some(Severity::Error),
            Level::Warn => Some(Severity::Warn),
            Level::Info => Some(Severity::Info),
            Level::Off => None,
        }
    }
}

impl From<Option<Severity>> for Level {
    fn from(severity: Option<Severity>) -> Self {
        match severity {
            Some(Severity::Error) => Self::Error,
            Some(Severity::Warn) => Self::Warn,
            Some(Severity::Info) => Self::Info,
            None => Self::Off,
        }
    }
}

/// Later level replaces the earlier one; options merge per key, later wins,
/// and an object-valued option (a limit per role) merges per key too; a
/// model route and an evaluation policy the later sets replace the earlier.
pub(crate) fn merge(into: &mut Rules, from: &Rules) {
    for (id, next) in from {
        match into.get_mut(id) {
            Some(prev) => {
                prev.level = next.level;
                for (key, value) in &next.options {
                    let merged = match prev.options.get(key) {
                        Some(before) => crate::options::merge(before, value),
                        None => value.clone(),
                    };
                    prev.options.insert(key.clone(), merged);
                }
                prev.generated = next.generated.or(prev.generated);
                prev.evaluation = next.evaluation.or(prev.evaluation);
                if next.classification.is_some() {
                    prev.classification.clone_from(&next.classification);
                }
            }
            None => {
                into.insert(id.clone(), next.clone());
            }
        }
    }
}

fn level_field<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Level, D::Error> {
    let text = String::deserialize(deserializer)?;
    level_of(&text).map_err(de::Error::custom)
}

fn level_of(text: &str) -> Result<Level, String> {
    match text {
        "error" => Ok(Level::Error),
        "warn" => Ok(Level::Warn),
        "info" => Ok(Level::Info),
        "off" => Ok(Level::Off),
        "review" => Err("`review` is not a level any more; use `info`".to_owned()),
        other => Err(format!(
            "unknown level `{other}` (expected error, warn, info or off)"
        )),
    }
}
