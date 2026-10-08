use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Catalog, Check, Decision, Enforcement, Error, Example, ExampleKind,
    sources::{Source, has_keyword},
};

/// Everything a single layer must satisfy on its own.
pub(crate) fn layer(catalog: &Catalog) -> Result<(), Error> {
    decisions(catalog)?;
    sources(catalog)
}

/// Decision-level rules; also run on a merged overlay.
pub(crate) fn decisions(catalog: &Catalog) -> Result<(), Error> {
    for pack in &catalog.packs {
        kebab(&pack.id)?;
        for section in &pack.sections {
            kebab(&section.id)?;
        }
    }
    let mut ids = BTreeSet::new();
    for decision in catalog.decisions() {
        if !ids.insert(decision.id()) {
            return Err(Error::invalid(decision.id(), "defined twice"));
        }
        self::decision(decision)?;
    }
    Ok(())
}

pub(crate) fn decision(decision: &Decision) -> Result<(), Error> {
    let id = decision.id();
    let (pack, name) = id
        .split_once('/')
        .ok_or_else(|| Error::invalid(id, "an id is `<pack>/<name>`"))?;
    kebab(pack)?;
    kebab(name)?;
    if decision.title.trim().is_empty() || decision.intent.trim().is_empty() {
        return Err(Error::invalid(id, "title and intent must not be empty"));
    }
    if !has_keyword(&decision.requirement) {
        return Err(Error::invalid(
            id,
            "the requirement needs MUST, SHOULD or MAY",
        ));
    }
    let doc = decision.enforcement == Enforcement::Doc;
    if doc && decision.severity.is_some() {
        return Err(Error::invalid(id, "a `doc` decision has no severity"));
    }
    if doc && decision.check.is_some() {
        return Err(Error::invalid(id, "a `doc` decision has no check"));
    }
    let checkable = matches!(
        decision.enforcement,
        Enforcement::Mechanical | Enforcement::Heuristic
    );
    if checkable && decision.evidence.is_empty() {
        return Err(Error::invalid(
            id,
            "a checkable decision lists its evidence fields",
        ));
    }
    options(decision)?;
    decision
        .examples
        .iter()
        .try_for_each(|e| example(decision, e))?;
    canonical(decision)?;
    check(decision, checkable)?;
    fix(decision)
}

pub(crate) fn relative(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && path.split('/').all(|part| part != "..")
}

fn fix(decision: &Decision) -> Result<(), Error> {
    let id = decision.id();
    if let Some(fix) = &decision.fix {
        crate::fix::validate(
            id,
            fix,
            decision.enforcement == Enforcement::Mechanical,
            decision.check.is_some(),
        )?;
        let fixed = decision
            .examples
            .iter()
            .any(|e| e.kind == ExampleKind::Invalid && !e.fixed.is_empty());
        if !fixed {
            return Err(Error::invalid(
                id,
                "a fixable decision needs an invalid example with `fixed`",
            ));
        }
    }
    for example in decision.examples.iter().filter(|e| !e.fixed.is_empty()) {
        let fail =
            |reason: &str| Error::invalid(id, format!("example `{}`: {reason}", example.name));
        if decision.fix.is_none() {
            return Err(fail("`fixed` needs a `fix` on the decision"));
        }
        if example.kind != ExampleKind::Invalid {
            return Err(fail("only an invalid example has `fixed`"));
        }
        let mut seen = BTreeSet::new();
        for file in &example.fixed {
            if !example.files.iter().any(|f| f.path == file.path) || !seen.insert(&file.path) {
                return Err(fail(&format!(
                    "`fixed` file `{}` is not a file of the example, or is repeated",
                    file.path
                )));
            }
        }
    }
    Ok(())
}

fn options(decision: &Decision) -> Result<(), Error> {
    let id = decision.id();
    let empty = crate::OptionsSchema::default();
    let schema = decision.options.as_ref().unwrap_or(&empty);
    if schema.additional_properties {
        return Err(Error::invalid(
            id,
            "`options.additionalProperties` must be false: undeclared options are refused",
        ));
    }
    for (key, property) in &schema.properties {
        if !property.kind.accepts(&property.default) {
            return Err(Error::invalid(
                id,
                format!("option `{key}` has a default that is not {}", property.kind),
            ));
        }
        if property.description.trim().is_empty() {
            return Err(Error::invalid(
                id,
                format!("option `{key}` needs a description"),
            ));
        }
    }
    for (language, spec) in &decision.languages {
        for (key, value) in &spec.options {
            let property = schema.properties.get(key).ok_or_else(|| {
                Error::invalid(
                    id,
                    format!("`languages.{language}` sets unknown option `{key}`"),
                )
            })?;
            if !property.kind.accepts(value) {
                return Err(Error::invalid(
                    id,
                    format!(
                        "`languages.{language}` sets option `{key}` to a value that is not {}",
                        property.kind
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn example(decision: &Decision, example: &Example) -> Result<(), Error> {
    let id = decision.id();
    let fail = |reason: String| Error::invalid(id, format!("example `{}`: {reason}", example.name));
    if example.name.trim().is_empty() || example.language.is_empty() {
        return Err(Error::invalid(id, "an example needs a name and a language"));
    }
    if example.files.is_empty() {
        return Err(fail("has no files".to_owned()));
    }
    let mut paths = BTreeSet::new();
    for file in &example.files {
        if !relative(&file.path) || !paths.insert(&file.path) {
            return Err(fail(format!(
                "file path `{}` is absolute, escaping or repeated",
                file.path
            )));
        }
    }
    if example.kind == ExampleKind::Valid && !example.expect.is_empty() {
        return Err(fail("a valid example expects no diagnostics".to_owned()));
    }
    decision
        .resolve_options(&example.options, Some(&example.language))
        .map(drop)
}

/// A language has at most one canonical example per kind.
fn canonical(decision: &Decision) -> Result<(), Error> {
    let mut seen = BTreeSet::new();
    for example in decision.examples.iter().filter(|e| e.canonical) {
        if !seen.insert((example.language.as_str(), example.kind.to_string())) {
            return Err(Error::invalid(
                decision.id(),
                format!(
                    "more than one canonical {} example for language `{}`",
                    example.kind, example.language
                ),
            ));
        }
    }
    Ok(())
}

fn check(decision: &Decision, checkable: bool) -> Result<(), Error> {
    let id = decision.id();
    let Some(check) = &decision.check else {
        return Ok(());
    };
    match check {
        Check::Builtin(builtin) if builtin.id.is_empty() => {
            return Err(Error::invalid(id, "a builtin check needs a rule id"));
        }
        Check::Builtin(_) => {}
        Check::Cel(cel) => {
            if let Some(problem) = cel.problem() {
                return Err(Error::invalid(id, problem));
            }
            let subject = decision.scope.subject;
            if cel.select.scope() != subject.rule_scope() {
                return Err(Error::invalid(
                    id,
                    format!(
                        "selects `{}`, which a `{subject}` decision cannot run over",
                        cel.select.name()
                    ),
                ));
            }
        }
    }
    let has = |kind| decision.examples.iter().any(|e| e.kind == kind);
    if checkable && !(has(ExampleKind::Valid) && has(ExampleKind::Invalid)) {
        return Err(Error::invalid(
            id,
            "a checked decision needs a valid and an invalid example",
        ));
    }
    Ok(())
}

fn kebab(name: &str) -> Result<(), Error> {
    let ok = !name.is_empty()
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        });
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(name, "not a kebab-case name"))
    }
}

fn sources(catalog: &Catalog) -> Result<(), Error> {
    let mut seen: BTreeMap<&str, &Source> = BTreeMap::new();
    for source in &catalog.sources {
        let fail = |reason: &str| Error::invalid(&source.reference, reason);
        if seen.insert(&source.reference, source).is_some() {
            return Err(fail("listed twice"));
        }
        match (source.decisions.is_empty(), &source.omitted) {
            (true, None) => return Err(fail("maps to no decision and has no `omitted` reason")),
            (false, Some(_)) => return Err(fail("has decisions and an `omitted` reason")),
            (true, Some(reason)) if reason.trim().is_empty() => {
                return Err(fail("`omitted` reason is empty"));
            }
            _ => {}
        }
        if let Some(id) = source
            .decisions
            .iter()
            .find(|id| catalog.decision(id).is_none())
        {
            return Err(Error::invalid(
                &source.reference,
                format!("maps to unknown decision `{id}`"),
            ));
        }
    }
    Ok(())
}
