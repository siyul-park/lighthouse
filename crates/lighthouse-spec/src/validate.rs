use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::{EdgeKind, RunScope, Severity};

use crate::{
    Catalog, CheckKind, Decision, DecisionStatus, Error, Example, ExampleKind,
    check::{BuiltinCheck, BuiltinOp},
    sources::{SourceLine, has_keyword},
};

/// Everything a single layer must satisfy on its own.
pub(crate) fn layer(catalog: &Catalog) -> Result<(), Error> {
    decisions(catalog)?;
    sources(catalog)?;
    catalog.projects()?;
    Ok(())
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
    lifecycle(catalog)
}

/// The projects of a catalog: names are unique, every project they extend
/// exists, and none extends itself, however far round.
pub(crate) fn projects(catalog: &Catalog) -> Result<(), Error> {
    let all = catalog.projects()?;
    for name in all.names() {
        if let Some(layer) = all.get(name) {
            layer.entries(&all)?;
        }
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
    let automated = decision
        .check
        .as_ref()
        .is_some_and(crate::Check::is_automated);
    for reference in decision.supersedes.iter() {
        if reference.trim().is_empty() {
            return Err(Error::invalid(id, "`supersedes` entries are not empty"));
        }
    }
    options(decision)?;
    decision
        .examples
        .iter()
        .try_for_each(|e| example(decision, e))?;
    canonical(decision)?;
    check(decision, automated)?;
    fix(decision)
}

pub(crate) fn relative(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && path.split('/').all(|part| part != "..")
}

/// A decision that supersedes another names one that exists, is not itself,
/// and has been marked superseded; no two supersede each other.
fn lifecycle(catalog: &Catalog) -> Result<(), Error> {
    for decision in catalog.decisions() {
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

fn fix(decision: &Decision) -> Result<(), Error> {
    let id = decision.id();
    if let Some(fix) = &decision.fix {
        crate::fix::validate(
            id,
            fix,
            decision.severity == Some(Severity::Error),
            decision
                .check
                .as_ref()
                .is_some_and(crate::Check::is_automated),
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

fn check(decision: &Decision, automated: bool) -> Result<(), Error> {
    let id = decision.id();
    let Some(check) = &decision.check else {
        return Ok(());
    };
    if check.timeout_duration().is_none_or(|t| t.is_zero()) {
        return Err(Error::invalid(
            id,
            format!(
                "check `timeout` is `{}`, expected a duration such as `30s` or `2m`",
                check.timeout.as_deref().unwrap_or_default()
            ),
        ));
    }
    kind_problem(decision, &check.kind).map_or(Ok(()), |reason| Err(Error::invalid(id, reason)))?;
    let has = |kind| decision.examples.iter().any(|e| e.kind == kind);
    if automated && !(has(ExampleKind::Valid) && has(ExampleKind::Invalid)) {
        return Err(Error::invalid(
            id,
            "a checked decision needs a valid and an invalid example",
        ));
    }
    Ok(())
}

/// What is wrong with the provider a check names, for this decision.
fn kind_problem(decision: &Decision, kind: &CheckKind) -> Option<String> {
    let subject = decision.scope.subject;
    match kind {
        CheckKind::Builtin(BuiltinCheck::Named(named)) if named.id.is_empty() => {
            Some("a builtin check needs a rule id".to_owned())
        }
        CheckKind::Builtin(BuiltinCheck::Named(_)) => None,
        CheckKind::Builtin(BuiltinCheck::Op(op)) => op_problem(op).or_else(|| scope_problem(op, subject)),
        CheckKind::Cel(cel) => cel.problem().or_else(|| match cel.selects(subject) {
            None => Some(format!("a `{subject}` decision needs a `select`")),
            Some(select) => (select.scope() != subject.run_scope()).then(|| {
                format!(
                    "selects `{}`, which a `{subject}` decision cannot run over",
                    select.name()
                )
            }),
        }),
        CheckKind::Command(command) => command.problem().or_else(|| {
            let per_file = subject.run_scope() == RunScope::File;
            (command.batch == crate::check::Batch::All && per_file).then(|| {
                "check `batch: all` needs a module or project decision: a file-scope decision is checked one file at a time"
                    .to_owned()
            })
        }),
        CheckKind::Model(model) => model.problem().or_else(|| {
            (decision.severity == Some(Severity::Error)).then(|| {
                "a `model` check is capped at `warn`: only a deterministic check can be an `error`"
                    .to_owned()
            })
        }),
        CheckKind::Rpc(_) => Some("an `rpc` check is not supported until plugin protocol 0.2".to_owned()),
    }
}

/// A standard operation judges the shape of one file or of the whole project.
fn scope_problem(op: &BuiltinOp, subject: crate::Subject) -> Option<String> {
    let per_file = subject.run_scope() == RunScope::File;
    match (op, per_file) {
        (BuiltinOp::Order { .. } | BuiltinOp::Proximity { .. }, false) => Some(format!(
            "an `order` or `proximity` check judges one file at a time, which a `{subject}` decision does not"
        )),
        (BuiltinOp::Cycle { .. }, true) => Some(format!(
            "a `cycle` check judges the whole project, which a `{subject}` decision does not"
        )),
        _ => None,
    }
}

fn op_problem(op: &BuiltinOp) -> Option<String> {
    match op {
        BuiltinOp::Order { clauses } => {
            if clauses.is_empty() {
                return Some("check: `order` needs at least one clause".to_owned());
            }
            for clause in clauses {
                let mut seen = BTreeSet::new();
                if clause.by.is_empty() {
                    return Some(
                        "check: an `order` clause needs at least one key in `by`".to_owned(),
                    );
                }
                let stray = clause
                    .by
                    .iter()
                    .find(|key| !key.contains('/') || !seen.insert(*key));
                if let Some(key) = stray {
                    return Some(format!(
                        "check: order key `{key}` must be a qualified `plugin/name` and listed once"
                    ));
                }
                if let Some(problem) =
                    expressions(clause.when.as_ref(), &clause.message, &clause.evidence)
                {
                    return Some(problem);
                }
            }
            None
        }
        BuiltinOp::Proximity {
            when,
            group,
            separator,
            max_distance,
            contiguous,
            message,
            evidence,
        } => {
            if *contiguous && max_distance.is_some_and(|d| d > 0) {
                return Some(
                    "check: `contiguous` and a positive `maxDistance` disagree".to_owned(),
                );
            }
            [("group", Some(group)), ("separator", separator.as_ref())]
                .into_iter()
                .filter_map(|(what, source)| Some((what, source?)))
                .find_map(|(what, source)| {
                    cel::Program::compile(source)
                        .err()
                        .map(|e| format!("check {what}: {e}"))
                })
                .or_else(|| expressions(when.as_ref(), message, evidence))
        }
        BuiltinOp::Cycle { edge, .. } => edge
            .parse::<EdgeKind>()
            .is_err()
            .then(|| format!("check: `{edge}` is not an edge kind")),
    }
}

/// The first of a `when` guard, a message template and evidence expressions
/// that does not compile.
fn expressions(
    when: Option<&String>,
    message: &str,
    evidence: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    let compile = |what: &str, source: &str| {
        cel::Program::compile(source)
            .err()
            .map(|e| format!("check {what}: {e}"))
    };
    when.and_then(|source| compile("when", source))
        .or_else(|| {
            evidence
                .iter()
                .find_map(|(name, source)| compile(&format!("evidence `{name}`"), source))
        })
        .or_else(|| crate::check::template_problem(message).map(|p| format!("check message: {p}")))
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
