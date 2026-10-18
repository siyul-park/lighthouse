//! The environment of a project: how this machine computes and applies, as
//! opposed to what the project means. The keys of the project file that are
//! environment may also come from the user's file and the repository-local
//! file; the keys that decide findings, or what may change the shared
//! repository, are the project's alone.

use std::collections::BTreeMap;

use lighthouse_resource::Spec;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    RuleConfig, Rules,
    fixing::ApprovalSpec,
    models::{ModelsSpec, Names},
    rules::RuleSetting,
    spec::ProjectSpec,
};
use crate::Error;

/// The tables of the project file that decide findings or what may change the
/// shared repository.
const MEANING: [&str; 6] = [
    "plugins",
    "extends",
    "languages",
    "overrides",
    "generated",
    "fix",
];

/// The keys of a rule that are meaning; its model bindings are environment.
const MEANING_RULE: [&str; 6] = [
    "level",
    "options",
    "generated",
    "check",
    "fix",
    "evaluation",
];

/// What a user's file and a repository-local file hold: the keys of the
/// project that are about this machine. Later layers win key by key; a
/// provider is replaced whole by name, a binding is replaced, never joined.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentSpec {
    /// The bindings, limits and providers of this machine; the same table as
    /// the project's `models`.
    #[serde(default, skip_serializing_if = "ModelsSpec::is_empty")]
    pub models: ModelsSpec,
    /// The model bindings of single decisions, by decision id; they override
    /// the `models` binding of the same task for that decision.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, EnvironmentRule>,
    /// What happens to a fix by a model without a person's hand.
    #[serde(default, skip_serializing_if = "ApprovalSpec::is_empty")]
    pub approval: ApprovalSpec,
}

/// The bindings one decision has of its own. Its level, options and every
/// other key are the project's.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentRule {
    /// The model, or the route of several, that judges this decision, before
    /// `models.classification`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Names>,
    /// The agent, or the route of several, that fixes this decision by
    /// editing files, before `models.edit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<Names>,
}

impl Spec for EnvironmentSpec {
    const KIND: &'static str = "Environment";
}

impl EnvironmentSpec {
    /// Parses a user's or a repository-local file; `origin` names it in
    /// errors. A key that is the project's to set is an error that names the
    /// key and the file.
    pub fn parse(origin: &str, text: &str) -> Result<Self, Error> {
        let fail = |message: String| Error::Config(format!("{origin}: {message}"));
        let table: toml::Table = toml::from_str(text).map_err(|e| fail(e.to_string()))?;
        if let Some(key) = table.keys().find(|key| MEANING.contains(&key.as_str())) {
            return Err(fail(project_only(key)));
        }
        let rules = table.get("rules").and_then(toml::Value::as_table);
        for (id, rule) in rules.into_iter().flatten() {
            let keys = rule.as_table().into_iter().flat_map(toml::Table::keys);
            if let Some(key) = keys
                .into_iter()
                .find(|k| MEANING_RULE.contains(&k.as_str()))
            {
                return Err(fail(project_only(&format!("rules.\"{id}\".{key}"))));
            }
        }
        table.try_into().map_err(|e| fail(e.to_string()))
    }

    /// The environment keys the project file itself sets.
    pub(super) fn of_project(project: &ProjectSpec) -> Self {
        let rules = project
            .rules
            .iter()
            .filter_map(|(id, setting)| match setting {
                RuleSetting::Detailed(detail)
                    if detail.classification.is_some() || detail.edit.is_some() =>
                {
                    Some((
                        id.clone(),
                        EnvironmentRule {
                            classification: detail.classification.clone(),
                            edit: detail.edit.clone(),
                        },
                    ))
                }
                _ => None,
            })
            .collect();
        Self {
            models: project.models.clone(),
            rules,
            approval: project.approval,
        }
    }

    /// These keys with the keys `later` sets taking their place.
    pub(super) fn over(&mut self, later: &Self) {
        let (models, from) = (&mut self.models, &later.models);
        models.classification = from.classification.clone().or(models.classification.take());
        models.edit = from.edit.clone().or(models.edit.take());
        models.routing = from.routing.or(models.routing);
        models.budget = from.budget.or(models.budget);
        models.explore = from.explore.or(models.explore);
        models.providers.extend(from.providers.clone());
        for (id, next) in &later.rules {
            let rule = self.rules.entry(id.clone()).or_default();
            rule.classification = next.classification.clone().or(rule.classification.take());
            rule.edit = next.edit.clone().or(rule.edit.take());
        }
        self.approval.fix = later.approval.fix.or(self.approval.fix);
    }
}

/// Gives the decisions of `env` their bindings, over whatever the project
/// said. A decision the project does not enable stays disabled: an entry that
/// binds and has no level enables nothing.
pub(super) fn bind(into: &mut Rules, env: &BTreeMap<String, EnvironmentRule>) {
    for (id, next) in env {
        let rule = into
            .entry(id.clone())
            .or_insert_with(|| RuleConfig::new(None));
        if let Some(names) = &next.classification {
            rule.classification = Some(names.iter().map(str::to_owned).collect());
        }
        if let Some(names) = &next.edit {
            rule.edit = Some(names.iter().map(str::to_owned).collect());
        }
    }
}

fn project_only(key: &str) -> String {
    format!(
        "`{key}` decides what findings mean or what may change the repository, so only lighthouse.toml sets it"
    )
}
