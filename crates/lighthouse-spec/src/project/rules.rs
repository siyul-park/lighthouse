use std::collections::BTreeMap;

use lighthouse_model::{Options, Safety, Severity};
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
    /// The agent that fixes this decision by editing files, before the
    /// project's `models.agent`.
    pub agent: Option<Vec<String>>,
    /// The safety every fix of this decision has, whoever makes it, whatever
    /// the decision and the provider claim.
    pub safety: Option<Safety>,
    /// How `lighthouse eval` judges this rule's check.
    pub evaluation: Option<Evaluation>,
    /// The layers that set the level, in the order they applied; empty for a
    /// rule not resolved from a project.
    pub trace: Trace,
}

/// The layers a rule's level went through. Provenance, not part of what a
/// rule says: it never takes part in equality.
#[derive(Debug, Clone, Default, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct Trace(pub Vec<Step>);

impl PartialEq for Trace {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// One layer that set a rule's level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub stage: Stage,
    /// The file the project was read from, or its name when it is shared.
    pub source: String,
    /// For an override, which one and what it selects: `overrides[1]: files
    /// internal/**`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<String>,
    pub level: Level,
}

/// Where a layer of a project's rules comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    /// A project the resolved one extends (a pack's `recommended`, a shared
    /// project).
    Extends,
    /// The `rules` of the resolved project.
    Project,
    /// An `[[overrides]]` entry the file matched.
    Override,
}

impl RuleConfig {
    /// A rule at `level`, with nothing else said.
    pub fn new(level: Option<Severity>) -> Self {
        Self {
            level,
            options: Options::new(),
            generated: None,
            classification: None,
            agent: None,
            safety: None,
            evaluation: None,
            trace: Trace::default(),
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
                agent: detail
                    .agent
                    .map(|names| names.iter().map(str::to_owned).collect()),
                safety: detail.safety,
                evaluation: detail.evaluation,
                ..Self::new(detail.level.into())
            },
        }
    }
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
    /// The agent, or the route of several, that fixes this decision by
    /// editing files, before the project's `models.agent`. Findings bound to
    /// different agents in one file are fixed by one run each.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<Names>,
    /// The safety of every fix of this decision, applied last: `safe` writes
    /// them with `--fix` (trust the agent here), `unsafe` previews them
    /// until `--unsafe-fixes`. Where layers disagree, `unsafe` wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safety: Option<Safety>,
    /// How `lighthouse eval` judges this rule's check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluation: Option<Evaluation>,
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

/// Where the rules being merged come from: what a [`Step`] records.
pub(crate) struct Place<'a> {
    pub stage: Stage,
    pub source: &'a str,
    pub matched: Option<String>,
}

/// Later level replaces the earlier one; options merge per key, later wins,
/// and an object-valued option (a limit per role) merges per key too; a
/// model route, an agent route and an evaluation policy the later sets replace
/// the earlier; a safety does too, unless either is `unsafe`.
pub(crate) fn merge(into: &mut Rules, from: &Rules, at: &Place) {
    for (id, next) in from {
        let step = Step {
            stage: at.stage.clone(),
            source: at.source.to_owned(),
            matched: at.matched.clone(),
            level: next.level.into(),
        };
        match into.get_mut(id) {
            Some(prev) => {
                prev.trace.0.push(step);
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
                if next.agent.is_some() {
                    prev.agent.clone_from(&next.agent);
                }
                prev.safety = match (prev.safety, next.safety) {
                    (Some(Safety::Unsafe), _) | (_, Some(Safety::Unsafe)) => Some(Safety::Unsafe),
                    (before, after) => after.or(before),
                };
            }
            None => {
                let mut first = next.clone();
                first.trace = Trace(vec![step]);
                into.insert(id.clone(), first);
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
