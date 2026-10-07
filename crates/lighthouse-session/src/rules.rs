//! Looking at rules and running their examples.

use std::{cell::RefCell, collections::BTreeSet, fmt::Write};

use lighthouse_engine::RuleTester;
use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, Enforcement, Pattern};
use serde::Serialize;

use crate::{Result, Session};

/// The language examples without a provider run in.
const NEUTRAL: &str = "text";

/// One line of a rule listing.
#[derive(Debug, Clone, Serialize)]
pub struct RuleRow {
    pub id: String,
    /// `implemented`, `unimplemented` or `doc`.
    pub status: &'static str,
    pub severity: String,
    pub title: String,
}

/// The outcome of running examples.
pub struct RuleTest {
    pub patterns: usize,
    pub languages: usize,
    pub runs: usize,
    /// One line per failing example; empty when every example holds.
    pub failures: Vec<String>,
}

/// The rules of the registry with their catalog titles; with `all`, every
/// catalog pattern too, implemented or not.
pub fn rule_rows(catalog: &Catalog, registry: &Registry, all: bool) -> Vec<RuleRow> {
    let mut rows = Vec::new();
    if all {
        for pattern in catalog.patterns() {
            rows.push(RuleRow {
                id: pattern.id.clone(),
                status: status(pattern, registry),
                severity: pattern
                    .severity()
                    .map_or_else(|| "-".to_owned(), |s| s.to_string()),
                title: pattern.title.clone(),
            });
        }
    }
    for rule in registry.rules() {
        let meta = rule.manifest();
        if all && catalog.pattern(&meta.id).is_some() {
            continue;
        }
        rows.push(RuleRow {
            id: meta.id.clone(),
            status: "implemented",
            severity: meta.severity.to_string(),
            title: meta.description.clone(),
        });
    }
    rows
}

/// The text that explains a pattern or rule: intent, requirement, examples and
/// status, then the rule's analyzers and capabilities.
pub fn explain(catalog: &Catalog, registry: &Registry, id: &str) -> Result<String> {
    let pattern = catalog.pattern(id);
    let rule = registry.rule(id);
    let mut out = String::new();
    match (pattern, rule) {
        (Some(pattern), _) => {
            out.push_str(&lighthouse_spec::pattern_markdown(pattern, 1));
            let _ = writeln!(out, "Status: {}", status(pattern, registry));
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
        (None, None) => return Err(format!("unknown pattern or rule `{id}`").into()),
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

/// Runs the examples of the implemented patterns named by `ids` (all when
/// empty) in every language the project's plugins provide, or in `language`.
pub fn test_rules(session: &Session, ids: &[String], language: Option<&str>) -> Result<RuleTest> {
    let catalog = session.catalog()?;
    let patterns = selected(&catalog, ids)?;
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
                    lighthouse_builtin::registry()
                })
        };
        let tester = RuleTester::new(fresh, &catalog)
            .language(language)
            .trusted(session.trusted());
        for pattern in &patterns {
            runs += 1;
            failures.extend(
                tester
                    .check(pattern)
                    .into_iter()
                    .map(|f| format!("[{language}] {f}")),
            );
        }
    }
    if let Some(e) = restart_error.into_inner() {
        return Err(format!("plugins could not be started for every example: {e}").into());
    }
    Ok(RuleTest {
        patterns: patterns.len(),
        languages: languages.len(),
        runs,
        failures,
    })
}

fn status(pattern: &Pattern, registry: &Registry) -> &'static str {
    if registry.rule(&pattern.id).is_some() {
        "implemented"
    } else if pattern.enforcement == Enforcement::Doc {
        "doc"
    } else {
        "unimplemented"
    }
}

fn selected<'c>(catalog: &'c Catalog, ids: &[String]) -> Result<Vec<&'c Pattern>> {
    if ids.is_empty() {
        return Ok(catalog
            .patterns()
            .filter(|p| p.implementation.is_some())
            .collect());
    }
    ids.iter()
        .map(|id| {
            let pattern = catalog
                .pattern(id)
                .ok_or_else(|| format!("unknown pattern `{id}`"))?;
            if pattern.implementation.is_none() {
                return Err(format!("`{id}` has no implementation to test").into());
            }
            Ok(pattern)
        })
        .collect()
}
