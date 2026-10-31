//! The rules a catalog holds its decisions to, beyond what each kind says of
//! itself: ids, uids, supersession, options, examples, sources.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Catalog, CheckKind, Decision, DecisionStatus, Env, Error, Example, ExampleKind, FixKind,
    OptionType, Point,
    options::KINDS,
    sources::{SourceLine, has_keyword},
};

/// Everything a single layer must satisfy on its own.
pub(crate) fn layer(catalog: &Catalog, cel: &Env) -> Result<(), Error> {
    for pack in catalog.packs() {
        kebab(&pack.id)?;
        for section in &pack.sections {
            kebab(&section.id)?;
        }
    }
    unique(catalog)?;
    catalog.decisions().try_for_each(|d| decision(d, cel))?;
    option_sources(catalog)?;
    lifecycle(catalog, catalog.decisions())?;
    sources(catalog)
}

/// What the layers of `merged` must satisfy together, each having satisfied
/// the rest on its own: ids and uids unique across them, and the
/// supersession the decisions of `local` declare.
pub(crate) fn overlay(merged: &Catalog, local: &Catalog) -> Result<(), Error> {
    unique(merged)?;
    lifecycle(merged, local.decisions())
}

/// The projects of a catalog: every project they extend exists, and none
/// extends itself, however far round.
pub(crate) fn projects(catalog: &Catalog) -> Result<(), Error> {
    let all = catalog.projects();
    for name in all.names() {
        if let Some(config) = all.get(name) {
            config.entries(all)?;
        }
    }
    Ok(())
}

pub(crate) fn decision(decision: &Decision, cel: &Env) -> Result<(), Error> {
    let id = decision.id();
    let (pack, name) = id
        .split_once('/')
        .ok_or_else(|| Error::invalid(id, "an id is `<pack>/<name>`"))?;
    kebab(pack)?;
    kebab(name)?;
    if let Some(uid) = decision.uid()
        && !lighthouse_model::is_uid(uid)
    {
        return Err(Error::invalid(
            id,
            format!("uid `{uid}` is not a lowercase UUID v4"),
        ));
    }
    if decision.title.trim().is_empty() || decision.context.trim().is_empty() {
        return Err(Error::invalid(id, "title and context must not be empty"));
    }
    if !has_keyword(&decision.requirement) {
        return Err(Error::invalid(
            id,
            "the requirement needs MUST, SHOULD or MAY",
        ));
    }
    match (&decision.check, decision.severity) {
        (None, Some(_)) => {
            return Err(Error::invalid(
                id,
                "a decision without a `check` is documentation and has no `severity`",
            ));
        }
        (Some(_), None) => {
            return Err(Error::invalid(
                id,
                "a decision with a `check` needs a `severity`: error, warn or info",
            ));
        }
        _ => {}
    }
    if decision.supersedes.iter().any(|r| r.trim().is_empty()) {
        return Err(Error::invalid(id, "`supersedes` entries are not empty"));
    }
    options(decision)?;
    decision
        .examples
        .iter()
        .try_for_each(|e| example(decision, e))?;
    canonical(decision)?;
    check(decision, cel)?;
    fix(decision, cel)?;
    programs(decision)
}

pub(crate) fn relative(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && path.split('/').all(|part| part != "..")
}

/// No two decisions share an id or a uid.
fn unique(catalog: &Catalog) -> Result<(), Error> {
    let mut ids = BTreeSet::new();
    let mut uids = BTreeMap::new();
    for decision in catalog.decisions() {
        if !ids.insert(decision.id()) {
            return Err(Error::invalid(decision.id(), "defined twice"));
        }
        if let Some(uid) = decision.uid()
            && let Some(other) = uids.insert(uid, decision.id())
        {
            return Err(Error::invalid(
                decision.id(),
                format!(
                    "uid `{uid}` is also the uid of `{other}`; a uid is assigned once and never copied"
                ),
            ));
        }
    }
    Ok(())
}

/// A decision that reads the options of another (`options.from`) names one
/// that exists, is not itself, and reads none in its turn.
fn option_sources(catalog: &Catalog) -> Result<(), Error> {
    for decision in catalog.decisions() {
        let Some(from) = decision.options_from() else {
            continue;
        };
        let source = catalog.decision(from).filter(|s| s.id() != decision.id());
        match source {
            None => {
                return Err(Error::invalid(
                    decision.id(),
                    format!("`options.from` names `{from}`, which is no other decision"),
                ));
            }
            Some(source) if source.options_from().is_some() => {
                return Err(Error::invalid(
                    decision.id(),
                    format!("`options.from` names `{from}`, which reads the options of another"),
                ));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// A decision that supersedes another names one that exists, is not itself,
/// and has been marked superseded; no two supersede each other.
fn lifecycle<'c>(
    catalog: &Catalog,
    decisions: impl Iterator<Item = &'c Decision>,
) -> Result<(), Error> {
    for decision in decisions.filter(|d| !d.supersedes.is_empty()) {
        let id = decision.id();
        for old in &decision.supersedes {
            let fail = |reason: String| Error::invalid(id, format!("supersedes `{old}`: {reason}"));
            let Some(target) = catalog.decision(old) else {
                return Err(fail("no such decision".to_owned()));
            };
            if old == id {
                return Err(fail("a decision cannot supersede itself".to_owned()));
            }
            if target.status != DecisionStatus::Superseded {
                return Err(fail(format!(
                    "it is `{}`; set its status to `superseded`",
                    target.status
                )));
            }
        }
        if let Some(cycle) = supersession_cycle(catalog, id) {
            return Err(Error::invalid(
                id,
                format!(
                    "supersession goes round in a circle: {}",
                    cycle.join(" -> ")
                ),
            ));
        }
    }
    Ok(())
}

/// The decisions on a path of `supersedes` that leads from `start` back to it.
fn supersession_cycle<'c>(catalog: &'c Catalog, start: &'c str) -> Option<Vec<&'c str>> {
    let mut path = vec![start];
    let mut seen = BTreeSet::new();
    let mut stack: Vec<(&str, usize)> = vec![(start, 0)];
    while let Some((at, next)) = stack.pop() {
        let successors = catalog.decision(at).map_or(&[][..], |d| &d.supersedes[..]);
        let Some(to) = successors.get(next) else {
            path.pop();
            continue;
        };
        stack.push((at, next + 1));
        if to == start {
            path.push(start);
            return Some(path);
        }
        if seen.insert(to.as_str()) {
            path.push(to);
            stack.push((to, 0));
        }
    }
    None
}

fn fix(decision: &Decision, cel: &Env) -> Result<(), Error> {
    let id = decision.id();
    if let Some(fix) = &decision.fix {
        if let Some(reason) = fix.problem(cel, decision) {
            return Err(Error::invalid(id, format!("fix: {reason}")));
        }
        let fixed = decision
            .examples
            .iter()
            .any(|e| e.kind == ExampleKind::Invalid && !e.fixed.is_empty());
        if fix.deterministic() && !fixed {
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
        if property.reference.is_some() && property.target().is_none() {
            return Err(Error::invalid(
                id,
                format!("option `{key}` refers to a shape the spec does not define"),
            ));
        }
        if property.check(&property.default, "default").is_err() {
            return Err(Error::invalid(
                id,
                format!(
                    "option `{key}` has a default that is not {}",
                    property.describe()
                ),
            ));
        }
        if property.description.trim().is_empty() {
            return Err(Error::invalid(
                id,
                format!("option `{key}` needs a description"),
            ));
        }
        if property.strict_default
            && !matches!(
                property.kind,
                Some(crate::OptionType::Boolean | crate::OptionType::Array)
            )
        {
            return Err(Error::invalid(
                id,
                format!("option `{key}` uses `strictDefault` but is neither a boolean nor a list"),
            ));
        }
        if property.strict_default
            && property.kind == Some(crate::OptionType::Array)
            && property
                .items
                .as_ref()
                .is_none_or(|items| items.values.is_empty())
        {
            return Err(Error::invalid(
                id,
                format!("option `{key}` uses `strictDefault` but has no enumerated items"),
            ));
        }
    }
    if let Some(kinds) = schema.properties.get(KINDS) {
        kinds_shape(id, kinds)?;
    }
    for (language, spec) in &decision.languages {
        for (key, value) in &spec.options {
            let property = schema.properties.get(key).ok_or_else(|| {
                Error::invalid(
                    id,
                    format!("`languages.{language}` sets unknown option `{key}`"),
                )
            })?;
            if let Err(problem) = property.check(value, &format!("option `{key}`")) {
                return Err(Error::invalid(
                    id,
                    format!(
                        "`languages.{language}` sets option `{key}` to a value that is not {} ({problem})",
                        property.describe()
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// The `kinds` option lists kinds: `items.enum` names each one once, and the
/// default (checked against the items with the other defaults) is some of them.
fn kinds_shape(id: &str, kinds: &crate::OptionSchema) -> Result<(), Error> {
    let fail = |reason: &str| Error::invalid(id, format!("option `{KINDS}` {reason}"));
    let Some(items) = kinds
        .items
        .as_deref()
        .filter(|_| kinds.kind == Some(OptionType::Array))
    else {
        return Err(fail("must be a list with `items`"));
    };
    let declared: BTreeSet<&str> = items
        .values
        .iter()
        .filter_map(serde_json::Value::as_str)
        .filter(|kind| !kind.is_empty())
        .collect();
    if declared.is_empty() || declared.len() != items.values.len() {
        return Err(fail("needs `items.enum`: unique, non-empty strings"));
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
    let mut conflicts = BTreeSet::new();
    for other in &example.conflicts {
        if other == id
            || !other
                .split_once('/')
                .is_some_and(|(p, n)| kebab(p).is_ok() && kebab(n).is_ok())
            || !conflicts.insert(other)
        {
            return Err(fail(format!(
                "conflict `{other}` is not the id of another decision, or is repeated"
            )));
        }
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

fn check(decision: &Decision, cel: &Env) -> Result<(), Error> {
    let id = decision.id();
    let Some(check) = &decision.check else {
        return Ok(());
    };
    if let Some(reason) = check.kind.problem(cel, decision.scope.subject) {
        return Err(Error::invalid(id, reason));
    }
    let has = |kind| decision.examples.iter().any(|e| e.kind == kind);
    if decision.automated() && !(has(ExampleKind::Valid) && has(ExampleKind::Invalid)) {
        return Err(Error::invalid(
            id,
            "a checked decision needs a valid and an invalid example",
        ));
    }
    Ok(())
}

/// A `command` check or fix has a program for the languages it covers: its
/// own `command`, or one in each `languages.<id>` that has none of its own to
/// replace, and the ones the languages give run at the point they are bound.
fn programs(decision: &Decision) -> Result<(), Error> {
    let id = decision.id();
    let check = match decision.check.as_ref().map(|c| &c.kind) {
        Some(CheckKind::Command(command)) => Some(&command.program),
        _ => None,
    };
    let fix = match decision.fix.as_ref().map(|f| &f.kind) {
        Some(FixKind::Command(command)) => Some(&command.program),
        _ => None,
    };
    let owned = [("check", check, Point::Check), ("fix", fix, Point::Fix)];
    for (what, program, point) in owned {
        let given = decision.languages.values().any(|l| match point {
            Point::Check => l.check.is_some(),
            _ => l.fix.is_some(),
        });
        match program {
            Some(own) if own.command.is_empty() && !given => {
                return Err(Error::invalid(
                    id,
                    format!("{what}: `command` needs a program, or one in `languages.<id>.{what}`"),
                ));
            }
            None if given => {
                return Err(Error::invalid(
                    id,
                    format!(
                        "`languages.<id>.{what}` needs a decision whose `{what}` is a `command`"
                    ),
                ));
            }
            _ => {}
        }
    }
    for (language, spec) in &decision.languages {
        let given = [
            ("check", &spec.check, Point::Check),
            ("fix", &spec.fix, Point::Fix),
        ];
        for (what, program, point) in given {
            if let Some(why) = program.as_ref().and_then(|p| p.problem(point)) {
                return Err(Error::invalid(
                    id,
                    format!("`languages.{language}.{what}`: {why}"),
                ));
            }
        }
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
    let mut seen: BTreeMap<&str, &SourceLine> = BTreeMap::new();
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
