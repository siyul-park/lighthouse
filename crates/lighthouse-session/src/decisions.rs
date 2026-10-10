//! Looking at decisions and running their examples.

use std::{cell::RefCell, collections::BTreeSet, fmt::Write};

use lighthouse_engine::RuleTester;
use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, Decision};
use serde::Serialize;

use crate::{Result, Session};

/// The language examples without a provider run in.
const NEUTRAL: &str = "text";

/// One line of a decision listing.
#[derive(Debug, Clone, Serialize)]
pub struct DecisionRow {
    pub id: String,
    /// `not-enforced` (its status is not accepted), `implemented` (a program checks it), `judged` (agent review checks it), `unimplemented` (a check is named but nothing registers it) or `doc`.
    pub status: &'static str,
    pub severity: String,
    pub title: String,
}

/// The outcome of running examples.
pub struct DecisionTest {
    pub decisions: usize,
    pub languages: usize,
    pub runs: usize,
    /// One line per failing example; empty when every example holds.
    pub failures: Vec<String>,
}

/// The decisions the registry has a rule for, with their catalog titles; with
/// `all`, every catalog decision too, implemented or not.
pub fn decision_rows(catalog: &Catalog, registry: &Registry, all: bool) -> Vec<DecisionRow> {
    let mut rows = Vec::new();
    if all {
        for decision in catalog.decisions() {
            rows.push(DecisionRow {
                id: decision.id().to_owned(),
                status: status(decision, registry),
                severity: decision
                    .severity()
                    .map_or_else(|| "-".to_owned(), |s| s.to_string()),
                title: decision.title.clone(),
            });
        }
    }
    for rule in registry.rules() {
        let meta = rule.manifest();
        if all && catalog.decision(&meta.id).is_some() {
            continue;
        }
        rows.push(DecisionRow {
            id: meta.id.clone(),
            status: "implemented",
            severity: meta.severity.to_string(),
            title: meta.description.clone(),
        });
    }
    rows
}

/// The text that explains a decision or rule: intent, requirement, examples and
/// status, then the rule's analyzers and capabilities.
pub fn explain(catalog: &Catalog, registry: &Registry, id: &str) -> Result<String> {
    let decision = catalog.decision(id);
    let rule = registry.rule(id);
    let mut out = String::new();
    match (decision, rule) {
        (Some(decision), _) => {
            out.push_str(&lighthouse_spec::decision_markdown(decision, 1));
            let _ = writeln!(out, "Status: {}", status(decision, registry));
        }
        (None, Some(rule)) => {
            let meta = rule.manifest();
            let _ = writeln!(
                out,
                "{}  (default: {})\n\n{}\n\n{}",
                meta.id, meta.severity, meta.description, meta.docs
            );
            let _ = writeln!(out, "Scope: {:?}", meta.scope);
            if let Some(citation) = &meta.citation {
                let _ = writeln!(out, "Method: {citation}");
            }
        }
        (None, None) => return Err(format!("unknown decision or rule `{id}`").into()),
    }
    if let Some(rule) = rule {
        let meta = rule.manifest();
        if !meta.analyzers.is_empty() {
            let _ = writeln!(out, "Analyzers: {}", meta.analyzers.join(", "));
        }
        if !meta.capabilities.is_empty() {
            let caps: Vec<_> = meta.capabilities.iter().map(ToString::to_string).collect();
            let _ = writeln!(out, "Capabilities: {}", caps.join(", "));
        }
    }
    Ok(out)
}

/// Runs the examples of the checked decisions named by `ids` (all when
/// empty) in every language the project's plugins provide, or in `language`.
pub fn test_decisions(
    session: &Session,
    ids: &[String],
    language: Option<&str>,
) -> Result<DecisionTest> {
    test_catalog(session, &session.catalog()?, ids, language)
}

/// Like [`test_decisions`], over `catalog` instead of the project's own.
pub(crate) fn test_catalog(
    session: &Session,
    catalog: &Catalog,
    ids: &[String],
    language: Option<&str>,
) -> Result<DecisionTest> {
    let catalog = catalog.clone();
    let decisions = selected(&catalog, ids)?;
    let (probe, registered) = session.registry()?;
    if let Some(gap) = registered.incomplete.first() {
        return Err(format!("a language plugin did not start: {}", gap.reason).into());
    }
    let languages: BTreeSet<String> = match language {
        Some(language) => BTreeSet::from([language.to_owned()]),
        None => probe
            .languages()
            .map(|(_, l)| l.manifest().id.clone())
            .filter(|l| l != NEUTRAL)
            .collect(),
    };
    drop(probe);
    let languages = if languages.is_empty() {
        BTreeSet::from([NEUTRAL.to_owned()])
    } else {
        languages
    };
    let mut failures = Vec::new();
    let mut runs = 0;
    let restart_error = RefCell::new(None);
    for language in &languages {
        // Each example needs a fresh registry; if plugins cannot be started
        // again, the example still runs (on the in-process rules) and the
        // whole test is reported as an error below.
        let fresh = || {
            session
                .registry()
                .map(|(registry, _)| registry)
                .unwrap_or_else(|e| {
                    restart_error.borrow_mut().get_or_insert(e.to_string());
                    lighthouse_checks::registry()
                })
        };
        let tester = RuleTester::new(fresh, &catalog)
            .language(language)
            .trusted(session.trusted());
        for decision in &decisions {
            runs += 1;
            failures.extend(
                tester
                    .check(decision)
                    .into_iter()
                    .map(|f| format!("[{language}] {f}")),
            );
        }
    }
    if let Some(e) = restart_error.into_inner() {
        return Err(format!("plugins could not be started for every example: {e}").into());
    }
    Ok(DecisionTest {
        decisions: decisions.len(),
        languages: languages.len(),
        runs,
        failures,
    })
}

fn status(decision: &Decision, registry: &Registry) -> &'static str {
    if decision.check.is_some() && !decision.status.enforced() {
        // Written down, tested, not enforced.
        "not-enforced"
    } else if registry.rule(decision.id()).is_some() {
        "implemented"
    } else if decision.check.is_none() {
        "doc"
    } else if !decision.automated() {
        "judged"
    } else {
        "unimplemented"
    }
}

fn selected<'c>(catalog: &'c Catalog, ids: &[String]) -> Result<Vec<&'c Decision>> {
    if ids.is_empty() {
        return Ok(catalog.decisions().filter(|d| d.automated()).collect());
    }
    ids.iter()
        .map(|id| {
            let decision = catalog
                .decision(id)
                .ok_or_else(|| format!("unknown decision `{id}`"))?;
            if !decision.automated() {
                return Err(format!("`{id}` has no automated check to test").into());
            }
            Ok(decision)
        })
        .collect()
}
