//! Looking at decisions and running their examples.

use std::{cell::RefCell, collections::BTreeSet, fmt::Write, fs};

use lighthouse_engine::RuleTester;
use lighthouse_plugin::Registry;
use lighthouse_report::docs::{Density, decision_markdown};
use lighthouse_spec::{Catalog, Decision, authored_severity};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{Result, Selector, Session, config_file};

/// A decision shown on its own: titled at heading 1, examples named without links.
const ENTRY: Density<'static> = Density::Entry {
    level: 1,
    sources: None,
};

/// The language examples without a provider run in.
const NEUTRAL: &str = "text";

/// One line of a decision listing.
#[derive(Debug, Clone, Serialize)]
pub struct DecisionRow {
    pub id: String,
    pub status: Implementation,
    pub severity: String,
    pub title: String,
}

/// How far a decision is carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Implementation {
    /// Its status is not accepted: written down, tested, not enforced.
    NotEnforced,
    /// A program checks it.
    Implemented,
    /// Agent review checks it.
    Judged,
    /// A check is named but nothing registers it.
    Unimplemented,
    /// It has no check.
    Doc,
}

lighthouse_model::vocabulary!(Implementation, "implementation" {
    NotEnforced => "not-enforced",
    Implemented => "implemented",
    Judged => "judged",
    Unimplemented => "unimplemented",
    Doc => "doc",
});

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
                    .severity
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
            status: Implementation::Implemented,
            severity: meta.severity.to_string(),
            title: meta.description.clone(),
        });
    }
    rows
}

/// The rules the project's configuration enables for at least one file: the
/// projects it extends and the entries it sets, resolved over the catalog.
pub fn active_decisions(session: &Session) -> Result<BTreeSet<String>> {
    Ok(lighthouse_engine::active(
        &session.config,
        session.catalog.projects(),
    )?)
}

/// The effective configuration of the project: its root, the plugins and
/// projects it names, the decisions in force and the text of its file.
pub fn config_summary(session: &Session) -> Result<Value> {
    let plugins: Vec<&str> = session
        .config
        .plugins()
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    let file = config_file(&session.root).and_then(|path| fs::read_to_string(path).ok());
    Ok(json!({
        "root": session.root.display().to_string(),
        "plugins": plugins,
        "extends": session.config.extends(),
        "activeRules": active_decisions(session)?,
        "file": file,
    }))
}

/// The index of the project's catalog, one decision per line: id, authored
/// severity, status and title, tab-separated.
pub fn catalog_index(session: &Session) -> Result<String> {
    let mut out = String::new();
    for decision in session.catalog.decisions() {
        let severity = decision.severity.map_or_else(
            || "doc".to_owned(),
            |s| authored_severity(s, Some(decision)).to_string(),
        );
        let _ = writeln!(
            out,
            "{}\t{severity}\t{}\t{}",
            decision.id(),
            decision.status,
            decision.title
        );
    }
    Ok(out)
}

/// One decision of the project's catalog rendered as Markdown.
pub fn decision_text(session: &Session, id: &str) -> Result<String> {
    let decision = session
        .catalog
        .decision(id)
        .ok_or_else(|| format!("unknown decision `{id}`"))?;
    Ok(decision_markdown(decision, ENTRY))
}

/// The text that explains a decision or rule: context, requirement, examples and
/// status, then the rule's analyzers and capabilities.
pub fn explain(catalog: &Catalog, registry: &Registry, id: &str) -> Result<String> {
    let decision = catalog.decision(id);
    let rule = registry.rule(id);
    let mut out = String::new();
    match (decision, rule) {
        (Some(decision), _) => {
            out.push_str(&decision_markdown(decision, ENTRY));
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
    test_catalog(session, &session.catalog, ids, language)
}

/// Like [`test_decisions`], over `catalog` instead of the project's own.
pub(crate) fn test_catalog(
    session: &Session,
    catalog: &Catalog,
    ids: &[String],
    language: Option<&str>,
) -> Result<DecisionTest> {
    let decisions = selected(catalog, ids)?;
    let (probe, registered) = session.registry()?;
    if let Some(gap) = registered.incomplete.first() {
        return Err(format!("a language plugin did not start: {}", gap.reason).into());
    }
    let languages: BTreeSet<String> = match language {
        Some(language) => BTreeSet::from([language.to_owned()]),
        None => probe
            .languages()
            .iter()
            .map(|l| l.provider.manifest().id.clone())
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
        let tester = RuleTester::new(fresh, catalog)
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

fn status(decision: &Decision, registry: &Registry) -> Implementation {
    if decision.check.is_some() && !decision.status.enforced() {
        Implementation::NotEnforced
    } else if registry.rule(decision.id()).is_some()
        && decision.check.is_some() == decision.automated()
    {
        // A model check has a rule too, but a model judges it: it stays `judged`.
        Implementation::Implemented
    } else if decision.check.is_none() {
        Implementation::Doc
    } else if !decision.automated() {
        Implementation::Judged
    } else {
        Implementation::Unimplemented
    }
}

fn selected<'c>(catalog: &'c Catalog, ids: &[String]) -> Result<Vec<&'c Decision>> {
    if ids.is_empty() {
        return Ok(catalog.decisions().filter(|d| d.automated()).collect());
    }
    let ids = Selector::new(ids)?.expand(catalog.decisions().map(Decision::id))?;
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
