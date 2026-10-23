//! The variables a pack shares with its decisions: a decision that names one
//! gets it among its own `variables`.

use std::collections::BTreeSet;

use crate::{
    Cel, CheckKind, Decision, Error, FixKind, TemplatePart, Variable, identifiers, parse_template,
};

/// What is wrong with the shared variables of pack `pack`.
pub(super) fn validate(cel: &Cel, pack: &str, shared: &[Variable]) -> Result<(), Error> {
    let mut seen = BTreeSet::new();
    for variable in shared {
        if let Some(problem) = variable.problem(cel) {
            return Err(Error::invalid(pack, problem));
        }
        if !seen.insert(variable.name.as_str()) {
            return Err(Error::invalid(
                pack,
                format!("pack variable `{}` is defined twice", variable.name),
            ));
        }
    }
    Ok(())
}

/// `decision` with the shared variables it names, directly or through another
/// shared variable, put before its own. A decision that defines a variable of
/// the same name is refused: there are no overrides.
pub(super) fn import(
    cel: &Cel,
    decision: Decision,
    shared: &[Variable],
) -> Result<Decision, Error> {
    if shared.is_empty() {
        return Ok(decision);
    }
    let Some(CheckKind::Cel(check)) = decision.check.as_ref().map(|c| &c.kind) else {
        return Ok(decision);
    };
    let mut sources: Vec<&str> = vec![&check.condition];
    sources.extend(check.variables.iter().map(|v| v.expression.as_str()));
    sources.extend(check.evidence.values().map(String::as_str));
    if let Some(at) = &check.at {
        sources.extend(
            [&at.symbol, &at.line, &at.end_line]
                .into_iter()
                .flatten()
                .map(String::as_str),
        );
    }
    if let Some(identity) = &check.identity {
        sources.push(&identity.subject);
        sources.extend(identity.snippet.as_deref());
    }
    sources.extend(check.abstain.as_deref());
    if let Some(FixKind::Cel { edits, .. }) = decision.fix.as_ref().map(|f| &f.kind) {
        sources.push(edits);
    }
    let holes: Vec<String> = parse_template(&check.message)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|part| match part {
            TemplatePart::Hole(source) => Some(source),
            TemplatePart::Text(_) => None,
        })
        .collect();
    // Most decisions name none: spare them the parse.
    let text = sources
        .iter()
        .copied()
        .chain(holes.iter().map(String::as_str));
    if !text
        .clone()
        .any(|source| shared.iter().any(|v| source.contains(v.name.as_str())))
    {
        return Ok(decision);
    }
    let mut names: BTreeSet<String> = sources
        .into_iter()
        .chain(holes.iter().map(String::as_str))
        .flat_map(|source| identifiers(cel, source))
        .collect();
    // A shared variable may name the ones before it.
    let mut used = Vec::new();
    for variable in shared.iter().rev() {
        if names.contains(&variable.name) {
            names.extend(identifiers(cel, &variable.expression));
            used.push(variable);
        }
    }
    used.reverse();
    if used.is_empty() {
        return Ok(decision);
    }
    if let Some(own) = check
        .variables
        .iter()
        .find(|own| used.iter().any(|shared| shared.name == own.name))
    {
        return Err(Error::invalid(
            decision.id(),
            format!(
                "variable `{}` is a variable of its pack: a decision does not redefine one",
                own.name
            ),
        ));
    }
    let imported: Vec<Variable> = used.into_iter().cloned().collect();
    Ok(decision.map_spec(|mut spec| {
        if let Some(CheckKind::Cel(check)) = spec.check.as_mut().map(|c| &mut c.kind) {
            check.variables.splice(0..0, imported);
        }
        spec
    }))
}
