use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Catalog, Enforcement, Error, Example, Implementation, Kind, Pattern,
    sources::{Source, has_keyword},
};

/// Everything a single layer must satisfy on its own.
pub(crate) fn layer(catalog: &Catalog) -> Result<(), Error> {
    patterns(catalog)?;
    sources(catalog)
}

/// Pattern-level rules; also run on a merged overlay.
pub(crate) fn patterns(catalog: &Catalog) -> Result<(), Error> {
    for pack in &catalog.packs {
        kebab(&pack.id)?;
        for section in &pack.sections {
            kebab(&section.id)?;
        }
    }
    let mut ids = BTreeSet::new();
    for pattern in catalog.patterns() {
        if !ids.insert(pattern.id.as_str()) {
            return Err(Error::invalid(&pattern.id, "defined twice"));
        }
        self::pattern(pattern)?;
    }
    Ok(())
}

pub(crate) fn pattern(pattern: &Pattern) -> Result<(), Error> {
    let id = pattern.id.as_str();
    let (pack, name) = id
        .split_once('/')
        .ok_or_else(|| Error::invalid(id, "an id is `<pack>/<name>`"))?;
    kebab(pack)?;
    kebab(name)?;
    if pattern.title.trim().is_empty() || pattern.intent.trim().is_empty() {
        return Err(Error::invalid(id, "title and intent must not be empty"));
    }
    if !has_keyword(&pattern.requirement) {
        return Err(Error::invalid(
            id,
            "the requirement needs MUST, SHOULD or MAY",
        ));
    }
    let doc = pattern.enforcement == Enforcement::Doc;
    if doc && pattern.severity_override.is_some() {
        return Err(Error::invalid(id, "a `doc` pattern has no severity"));
    }
    if doc && pattern.implementation.is_some() {
        return Err(Error::invalid(id, "a `doc` pattern has no implementation"));
    }
    let checkable = matches!(
        pattern.enforcement,
        Enforcement::Mechanical | Enforcement::Heuristic
    );
    if checkable && pattern.evidence.is_empty() {
        return Err(Error::invalid(
            id,
            "a checkable pattern lists its evidence fields",
        ));
    }
    options(pattern)?;
    pattern
        .examples
        .iter()
        .try_for_each(|e| example(pattern, e))?;
    implementation(pattern, checkable)
}

pub(crate) fn relative(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && path.split('/').all(|part| part != "..")
}

fn options(pattern: &Pattern) -> Result<(), Error> {
    for (key, spec) in &pattern.options {
        let values = std::iter::once(&spec.default).chain(spec.per_language.values());
        if values.into_iter().any(|v| !spec.kind.accepts(v)) {
            return Err(Error::invalid(
                &pattern.id,
                format!("option `{key}` has a value that is not {}", spec.kind),
            ));
        }
        if spec.description.trim().is_empty() {
            return Err(Error::invalid(
                &pattern.id,
                format!("option `{key}` needs a description"),
            ));
        }
    }
    Ok(())
}

fn example(pattern: &Pattern, example: &Example) -> Result<(), Error> {
    let fail = |reason: String| {
        Error::invalid(&pattern.id, format!("example `{}`: {reason}", example.name))
    };
    if example.name.trim().is_empty() || example.language.is_empty() {
        return Err(Error::invalid(
            &pattern.id,
            "an example needs a name and a language",
        ));
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
    if example.kind == Kind::Valid && !example.expect.is_empty() {
        return Err(fail("a valid example expects no diagnostics".to_owned()));
    }
    pattern
        .resolve_options(&example.options, Some(&example.language))
        .map(drop)
}

fn implementation(pattern: &Pattern, checkable: bool) -> Result<(), Error> {
    let id = pattern.id.as_str();
    let Some(implementation) = &pattern.implementation else {
        return Ok(());
    };
    match implementation {
        Implementation::Builtin(rule) if rule.is_empty() => {
            return Err(Error::invalid(id, "builtin implementation needs a rule id"));
        }
        Implementation::Declarative(path) if !relative(path) || !path.ends_with(".yaml") => {
            return Err(Error::invalid(
                id,
                "declarative implementation needs a relative .yaml path",
            ));
        }
        _ => {}
    }
    let has = |kind| pattern.examples.iter().any(|e| e.kind == kind);
    if checkable && !(has(Kind::Valid) && has(Kind::Invalid)) {
        return Err(Error::invalid(
            id,
            "an implemented pattern needs a valid and an invalid example",
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
        match (source.patterns.is_empty(), &source.omitted) {
            (true, None) => return Err(fail("maps to no pattern and has no `omitted` reason")),
            (false, Some(_)) => return Err(fail("has patterns and an `omitted` reason")),
            (true, Some(reason)) if reason.trim().is_empty() => {
                return Err(fail("`omitted` reason is empty"));
            }
            _ => {}
        }
        if let Some(id) = source
            .patterns
            .iter()
            .find(|id| catalog.pattern(id).is_none())
        {
            return Err(Error::invalid(
                &source.reference,
                format!("maps to unknown pattern `{id}`"),
            ));
        }
    }
    Ok(())
}
