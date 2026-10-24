//! The functions a check calls exist, with a form they take: a typo in a
//! branch no example takes is a load error, not an error on the first file
//! that reaches it.

use std::collections::{BTreeMap, BTreeSet};

use cel::{Context, ExecutionError, Program};
use lighthouse_spec::{Call, Variable};

use crate::{Composition, Error, compose};

/// Refuses a call of `programs` to a function that is neither of the `cel`
/// crate, nor of a library of the catalog, nor one of `functions` (the
/// variables of the check that have parameters), and a call of a library
/// function or of one of `functions` with a number of arguments it has no
/// overload for. A function of the `cel` crate is found by calling it with no
/// arguments: the runtime tells one that does not exist from one that is
/// given the wrong ones (the crate cannot list its functions, so the
/// arguments of its own are not counted).
pub(crate) fn check_calls<'a>(
    comp: &Composition,
    id: &str,
    library: &Context<'static, 'static>,
    functions: &[&Variable],
    programs: impl IntoIterator<Item = &'a Program>,
) -> Result<(), Error> {
    let catalog = comp.functions();
    let calls: BTreeSet<Call> = programs
        .into_iter()
        .flat_map(|program| lighthouse_spec::program_calls(comp.cel(), program))
        .collect();
    let scope = library.new_inner_scope();
    for call in calls {
        if check_own(id, functions, &call)? || !named(&call) {
            continue;
        }
        let full = full_name(&call);
        let candidates: Vec<&compose::Entry> = catalog
            .iter()
            .filter(|entry| {
                entry.function.name == full && entry.function.receiver.is_some() == call.member
            })
            .collect();
        let known = known_functions(id, &call, candidates)?;
        if known.iter().any(|entry| entry.function.accepts(call.args)) {
            continue;
        }
        check_other_call(id, comp, &scope, &call, &full, &known)?;
    }
    Ok(())
}

/// The libraries (id and version) whose functions the programs call: a call
/// counts when the catalog has a function of that name, as a member or not,
/// that takes that many arguments. A name the `cel` crate shares with a
/// library (`indexOf`) counts too, so the revision of a check may change
/// without need, never the other way round.
pub(crate) fn libraries_called<'a>(
    comp: &Composition,
    programs: impl IntoIterator<Item = &'a Program>,
) -> BTreeMap<String, String> {
    let catalog = comp.functions();
    let mut used = BTreeMap::new();
    for call in programs
        .into_iter()
        .flat_map(|program| lighthouse_spec::program_calls(comp.cel(), program))
    {
        let full = full_name(&call);
        for entry in catalog.iter().filter(|e| {
            e.function.name == full
                && e.function.receiver.is_some() == call.member
                && e.function.accepts(call.args)
        }) {
            used.insert(entry.library.clone(), entry.version.clone());
        }
    }
    used
}

fn check_own(id: &str, functions: &[&Variable], call: &Call) -> Result<bool, Error> {
    let Some(own) = functions
        .iter()
        .find(|function| function.name == call.name && !call.member && call.namespace.is_none())
    else {
        return Ok(false);
    };
    if own.params.len() != call.args {
        return Err(Error::invalid(
            id,
            format!(
                "`{}` takes {} argument(s), not {}",
                call.name,
                own.params.len(),
                call.args
            ),
        ));
    }
    Ok(true)
}

fn named(call: &Call) -> bool {
    call.name.starts_with(|c: char| c.is_ascii_alphabetic())
        && call
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn known_functions<'a>(
    id: &str,
    call: &Call,
    candidates: Vec<&'a compose::Entry>,
) -> Result<Vec<&'a compose::Entry>, Error> {
    let Some(receiver) = call.receiver_type.as_deref() else {
        return Ok(candidates);
    };
    let matching: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|entry| entry.function.receiver.as_deref() == Some(receiver))
        .collect();
    if matching.is_empty() && !candidates.is_empty() {
        let expected: BTreeSet<&str> = candidates
            .iter()
            .filter_map(|entry| entry.function.receiver.as_deref())
            .collect();
        return Err(Error::invalid(
            id,
            format!(
                "`{}` expects a `{}` receiver, got `{receiver}`",
                full_name(call),
                expected.into_iter().collect::<Vec<_>>().join("` or `")
            ),
        ));
    }
    Ok(matching)
}

fn check_other_call(
    id: &str,
    comp: &Composition,
    scope: &Context<'_, '_>,
    call: &Call,
    full: &str,
    known: &[&compose::Entry],
) -> Result<(), Error> {
    let source = match (&call.namespace, call.member) {
        (Some(namespace), _) => format!("{namespace}.{}()", call.name),
        (None, true) => format!("null.{}()", call.name),
        (None, false) => format!("{}()", call.name),
    };
    if !known.is_empty() {
        return if provided_by_cel(&source)? {
            Ok(())
        } else {
            Err(Error::invalid(
                id,
                arity(full, call.member, known, call.args),
            ))
        };
    }
    let Ok(probe) = comp.cel().compile(&source) else {
        return Ok(());
    };
    if matches!(
        probe.execute(scope),
        Err(ExecutionError::UndeclaredReference(_))
    ) {
        return Err(Error::invalid(
            id,
            format!(
                "`{full}` is not a function of CEL, of a library of the catalog or of the variables of the check"
            ),
        ));
    }
    Ok(())
}
fn full_name(call: &Call) -> String {
    match &call.namespace {
        Some(namespace) => format!("{namespace}.{}", call.name),
        None => call.name.clone(),
    }
}

/// Whether the `cel` crate alone has the function that `source` calls with no
/// arguments: the environment before any library is registered.
fn provided_by_cel(source: &str) -> Result<bool, Error> {
    let env = std::sync::Arc::new(lighthouse_spec::base().map_err(|e| Error::invalid("cel", e))?);
    let Ok(probe) = env.compile(source) else {
        return Ok(true);
    };
    let context = Context::with_env(env);
    Ok(!matches!(
        probe.execute(&context),
        Err(ExecutionError::UndeclaredReference(_))
    ))
}

fn arity(full: &str, member: bool, known: &[&compose::Entry], args: usize) -> String {
    let takes: BTreeSet<String> = known
        .iter()
        .flat_map(|e| {
            e.function
                .overloads
                .iter()
                .map(|o| o.params.len().to_string())
        })
        .collect();
    let on = if member { " (called on a value)" } else { "" };
    format!(
        "`{full}`{on} takes {} argument(s), not {args}",
        takes.into_iter().collect::<Vec<_>>().join(" or ")
    )
}
