use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, Enforcement, Pattern};

use crate::Result;

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
