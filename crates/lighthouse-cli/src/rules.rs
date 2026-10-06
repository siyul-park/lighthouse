use std::{collections::BTreeSet, path::Path};

use lighthouse_engine::RuleTester;
use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, Enforcement, Pattern};

use crate::{Result, session::Session};

/// The language examples without a provider run in.
const NEUTRAL: &str = "text";

/// Runs the examples of the implemented patterns named by `ids` (all when
/// empty) in every language the project's plugins provide, or in `language`.
/// Returns 1 when an example fails.
pub fn test(ids: &[String], language: Option<&str>, config: Option<&Path>) -> Result<u8> {
    let session = Session::load_or_default(config)?;
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
            .map(|(_, l)| l.id().to_owned())
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
    for language in &languages {
        let fresh = || session.registry().expect("plugins started once already").0;
        let tester = RuleTester::new(fresh, &catalog).language(language);
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
    for failure in &failures {
        println!("FAIL {failure}");
    }
    println!(
        "rule test: {} pattern(s) in {} language(s), {runs} run(s), {} failure(s)",
        patterns.len(),
        languages.len(),
        failures.len()
    );
    Ok(u8::from(!failures.is_empty()))
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

pub fn list(all: bool) -> Result<u8> {
    let registry = lighthouse_builtin::registry();
    if all {
        list_all(&registry);
    } else {
        for rule in registry.rules() {
            let meta = rule.meta();
            println!("{}\t{}\t{}", meta.id, meta.severity, meta.description);
        }
    }
    Ok(0)
}

fn list_all(registry: &Registry) {
    let catalog = Catalog::bundled();
    for pattern in catalog.patterns() {
        let severity = pattern
            .severity()
            .map_or_else(|| "-".to_owned(), |s| s.to_string());
        println!(
            "{}\t{}\t{severity}\t{}",
            pattern.id,
            status(pattern, registry),
            pattern.title
        );
    }
    for rule in registry
        .rules()
        .filter(|r| catalog.pattern(&r.meta().id).is_none())
    {
        let meta = rule.meta();
        println!(
            "{}\timplemented\t{}\t{}",
            meta.id, meta.severity, meta.description
        );
    }
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

pub fn explain(id: &str) -> Result<u8> {
    let registry = lighthouse_builtin::registry();
    let pattern = Catalog::bundled().pattern(id);
    let rule = registry.rule(id);
    match (pattern, rule) {
        (Some(pattern), _) => {
            print!("{}", lighthouse_spec::pattern_markdown(pattern, 1));
            println!("Status: {}", status(pattern, &registry));
        }
        (None, Some(rule)) => {
            let meta = rule.meta();
            println!(
                "{}  (default: {})\n\n{}\n\n{}",
                meta.id, meta.severity, meta.description, meta.docs
            );
            println!("Scope: {:?}", meta.scope);
            if let Some(citation) = &meta.citation {
                println!("Method: {citation}");
            }
        }
        (None, None) => return Err(format!("unknown pattern or rule `{id}`").into()),
    }
    if let Some(rule) = rule {
        let meta = rule.meta();
        if !meta.analyzers.is_empty() {
            println!("Analyzers: {}", meta.analyzers.join(", "));
        }
        if !meta.capabilities.is_empty() {
            let caps: Vec<_> = meta.capabilities.iter().map(ToString::to_string).collect();
            println!("Capabilities: {}", caps.join(", "));
        }
    }
    Ok(0)
}
