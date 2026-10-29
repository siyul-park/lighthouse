//! The projects every pack has: `<pack>/recommended` holds each enforced
//! decision at its authored severity, and `<pack>/strict` also enables options
//! marked `strictDefault` and every value of a partially enabled enum option.
//! `strict` is present only when some decision of the pack has such an option.
//! They derive from the decisions, so a catalog never lists them.

use lighthouse_model::Options;
use serde_json::Value;

use super::{Level, Metadata, ProjectSpec, Resource, RuleSetting, rules::RuleDetail};
use crate::{Catalog, CheckKind, Decision, OptionType, Pack, options::KINDS};

/// The standard projects of every pack of `catalog`, pack by pack.
pub(crate) fn standard(catalog: &Catalog) -> Vec<Resource<ProjectSpec>> {
    let mut projects = Vec::new();
    for pack in catalog.packs() {
        let rules: Vec<&Decision> = pack
            .sections
            .iter()
            .flat_map(|s| &s.decisions)
            .filter(|d| runs_as_rule(d) && d.enforced())
            .collect();
        projects.push(project(pack, &rules, false));
        if rules.iter().any(|d| strict_options(d).is_some()) {
            projects.push(project(pack, &rules, true));
        }
    }
    projects
}

fn project(pack: &Pack, rules: &[&Decision], strict: bool) -> Resource<ProjectSpec> {
    let rules = rules
        .iter()
        .filter_map(|d| {
            let level = Level::from(Some(d.severity?));
            let setting = match strict_options(d).filter(|_| strict) {
                Some(options) => RuleSetting::Detailed(RuleDetail {
                    level,
                    options,
                    generated: None,
                    classification: None,
                    agent: None,
                    safety: None,
                    evaluation: None,
                }),
                None => RuleSetting::Level(level),
            };
            Some((d.id().to_owned(), setting))
        })
        .collect();

    let name = if strict { "strict" } else { "recommended" };
    Resource::new(
        Metadata::named(format!("{}/{name}", pack.id)),
        ProjectSpec {
            rules,
            ..ProjectSpec::default()
        },
    )
}

/// Options that the strict preset enables beyond the recommended defaults.
fn strict_options(decision: &Decision) -> Option<Options> {
    let schema = decision.options.as_ref()?;
    let mut values = Vec::new();
    for (name, property) in &schema.properties {
        if property.strict_default {
            let value = match property.kind {
                Some(OptionType::Boolean) => Value::Bool(true),
                Some(OptionType::Array) => {
                    let items = property.items.as_deref()?;
                    if items.values.is_empty() {
                        return None;
                    }
                    Value::Array(items.values.clone())
                }
                _ => return None,
            };
            values.push((name.clone(), value));
            continue;
        }
        // Existing enum options may deliberately enable only a subset by
        // default. Strict enables the full enum without requiring each rule to
        // invent a kind for a single concern.
        if name == KINDS {
            let all = &property.items.as_ref()?.values;
            let enabled = property.default.as_array()?;
            if all.iter().any(|kind| !enabled.contains(kind)) {
                values.push((name.clone(), Value::Array(all.clone())));
            }
        }
    }
    (!values.is_empty()).then(|| Options::from_iter(values))
}

/// Whether the engine runs a rule for the decision: its check is a program.
fn runs_as_rule(decision: &Decision) -> bool {
    matches!(
        decision.check.as_ref().map(|check| &check.kind),
        Some(CheckKind::Cel(_) | CheckKind::Command(_))
    )
}
