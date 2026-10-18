//! The projects every pack has: `<pack>/recommended` holds each enforced
//! decision at its authored severity, and `<pack>/strict` the same with every
//! kind of a decision whose `kinds` option switches only some on by default.
//! `strict` is present only when some decision of the pack has such an option.
//! They derive from the decisions, so a catalog never lists them.

use lighthouse_model::{
    Options,
    annotation::{ANNOTATION_REASON, UNUSED_ALLOW},
};
use serde_json::Value;

use super::{Level, Metadata, ProjectSpec, Resource, RuleSetting, rules::RuleDetail};
use crate::{BuiltinCheck, Catalog, CheckKind, Decision, FORMATTED, Pack, options::KINDS};

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
        if rules.iter().any(|d| every_kind(d).is_some()) {
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
            let setting = match every_kind(d).filter(|_| strict) {
                Some(kinds) => RuleSetting::Detailed(RuleDetail {
                    level,
                    options: Options::from_iter([(KINDS.to_owned(), Value::Array(kinds))]),
                    generated: None,
                    classification: None,
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

/// Every kind a decision can report, when its `kinds` option switches only
/// some of them on by default.
fn every_kind(decision: &Decision) -> Option<Vec<Value>> {
    let kinds = decision.options.as_ref()?.properties.get(KINDS)?;
    let all = &kinds.items.as_ref()?.values;
    let on = kinds.default.as_array()?;
    all.iter()
        .any(|kind| !on.contains(kind))
        .then(|| all.clone())
}

/// Whether the engine runs a rule for the decision: its check is a program,
/// or one of the named rules the engine serves itself.
fn runs_as_rule(decision: &Decision) -> bool {
    let Some(check) = &decision.check else {
        return false;
    };
    match &check.kind {
        CheckKind::Cel(_) | CheckKind::Command(_) | CheckKind::Builtin(BuiltinCheck::Op(_)) => true,
        CheckKind::Builtin(BuiltinCheck::Named(named)) => {
            [ANNOTATION_REASON, UNUSED_ALLOW, FORMATTED].contains(&named.id.as_str())
        }
        CheckKind::Model(_) => false,
    }
}
