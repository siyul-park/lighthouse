//! The projects every pack has: `<pack>/recommended` holds each enforced
//! decision that is not `strict` at its authored severity, and
//! `<pack>/strict` holds every enforced one, present only when some decision
//! of the pack is strict. They derive from the decisions, so a catalog never
//! lists them.

use lighthouse_model::annotation::{ANNOTATION_REASON, UNUSED_ALLOW};

use super::{Metadata, ProjectSpec, Resource, RuleSetting};
use crate::{BuiltinCheck, Catalog, CheckKind, Decision, Pack};

/// The standard projects of every pack of `catalog`, pack by pack.
pub(crate) fn standard(catalog: &Catalog) -> Vec<Resource<ProjectSpec>> {
    let mut projects = Vec::new();
    for pack in &catalog.packs {
        let rules: Vec<&Decision> = pack
            .sections
            .iter()
            .flat_map(|s| &s.decisions)
            .filter(|d| runs_as_rule(d) && d.enforced())
            .collect();
        projects.push(project(pack, "recommended", &rules, |d| !d.strict));
        if rules.iter().any(|d| d.strict) {
            projects.push(project(pack, "strict", &rules, |_| true));
        }
    }
    projects
}

fn project(
    pack: &Pack,
    name: &str,
    rules: &[&Decision],
    include: fn(&Decision) -> bool,
) -> Resource<ProjectSpec> {
    let rules = rules
        .iter()
        .filter(|d| include(d))
        .filter_map(|d| Some((d.id().to_owned(), RuleSetting::level(d.severity()?))))
        .collect();
    Resource::new(
        Metadata::named(format!("{}/{name}", pack.id)),
        ProjectSpec {
            rules,
            ..ProjectSpec::default()
        },
    )
}

/// Whether the engine runs a rule for the decision: its check is a program,
/// or one of the two annotation rules the engine serves itself.
fn runs_as_rule(decision: &Decision) -> bool {
    let Some(check) = &decision.check else {
        return false;
    };
    match &check.kind {
        CheckKind::Cel(_) | CheckKind::Command(_) | CheckKind::Builtin(BuiltinCheck::Op(_)) => true,
        CheckKind::Builtin(BuiltinCheck::Named(named)) => {
            [ANNOTATION_REASON, UNUSED_ALLOW].contains(&named.id.as_str())
        }
        CheckKind::Rpc(_) | CheckKind::Model(_) => false,
    }
}
