//! The functions a check calls exist, with a form they take: a typo in a
//! branch no example takes is a load error, not an error on the first file
//! that reaches it.

use std::collections::{BTreeMap, BTreeSet};

use cel::{Context, ExecutionError, Program};
use lighthouse_spec::{Call, Variable};

use crate::{Error, compose};

/// Refuses a call of `programs` to a function that is neither of the `cel`
/// crate, nor of a library of the catalog, nor one of `functions` (the
/// variables of the check that have parameters), and a call of a library
/// function or of one of `functions` with a number of arguments it has no
/// overload for. A function of the `cel` crate is found by calling it with no
/// arguments: the runtime tells one that does not exist from one that is
/// given the wrong ones (the crate cannot list its functions, so the
/// arguments of its own are not counted).
pub(crate) fn check_calls<'a>(
    id: &str,
    library: &Context<'static, 'static>,
    functions: &[&Variable],
    programs: impl IntoIterator<Item = &'a Program>,
) -> Result<(), Error> {
    let catalog = compose::catalog().map_err(|e| Error::invalid(id, e))?;
    let calls: BTreeSet<Call> = programs
        .into_iter()
        .flat_map(lighthouse_spec::program_calls)
        .collect();
    let scope = library.new_inner_scope();
    for call in calls {
        if let Some(own) = functions
            .iter()
            .find(|f| f.name == call.name && !call.member)
        {
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
            continue;
        }
        // An operator (`_+_`) or an internal function (`@in`) has no source
        // form; it is always there.
        let named = call.name.starts_with(|c: char| c.is_ascii_alphabetic())
            && call
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !named {
            continue;
        }
        let full = full_name(&call);
        let known: Vec<&compose::Entry> = catalog
            .iter()
            .filter(|e| e.function.name == full && e.function.receiver.is_some() == call.member)
            .collect();
        if known.iter().any(|e| e.function.accepts(call.args)) {
            continue;
        }
        let source = match (&call.namespace, call.member) {
            (Some(namespace), _) => format!("{namespace}.{}()", call.name),
            (None, true) => format!("null.{}()", call.name),
            (None, false) => format!("{}()", call.name),
        };
        if !known.is_empty() {
            // A library function with other arguments than it takes; the
            // `cel` crate may still have a function of the same name that does.
            if provided_by_cel(&source)? {
                continue;
            }
            return Err(Error::invalid(
                id,
                arity(&full, call.member, &known, call.args),
            ));
        }
        let Ok(probe) = lighthouse_spec::compile(&source) else {
            continue;
        };
        if let Err(ExecutionError::UndeclaredReference(_)) = probe.execute(&scope) {
            return Err(Error::invalid(
                id,
                format!(
                    "`{full}` is not a function of CEL, of a library of the catalog or of the variables of the check"
                ),
            ));
        }
    }
    Ok(())
}

/// The libraries (id and version) whose functions the programs call: a call
/// counts when the catalog has a function of that name, as a member or not,
/// that takes that many arguments. A name the `cel` crate shares with a
/// library (`indexOf`) counts too, so the revision of a check may change
/// without need, never the other way round.
pub(crate) fn libraries_called<'a>(
    programs: impl IntoIterator<Item = &'a Program>,
) -> Result<BTreeMap<String, String>, String> {
    let catalog = compose::catalog()?;
    let mut used = BTreeMap::new();
    for call in programs
        .into_iter()
        .flat_map(lighthouse_spec::program_calls)
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
    Ok(used)
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
