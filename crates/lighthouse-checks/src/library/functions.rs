//! The functions a check calls exist: a typo in a branch no example takes is
//! a load error, not an error on the first file that reaches it.

use std::collections::BTreeSet;

use cel::{Context, ExecutionError, Program};
use lighthouse_spec::{Call, Variable};

use crate::Error;

/// Refuses a call of `programs` to a function that is neither of the
/// environment, nor of the library, nor one of `functions` (the variables of
/// the check that have parameters), and a call of one of `functions` with
/// other arguments than it has parameters. A function is found by calling it
/// with no arguments: the runtime tells one that does not exist from one
/// that is given the wrong ones.
pub(crate) fn check_calls<'a>(
    id: &str,
    library: &Context<'static, 'static>,
    functions: &[&Variable],
    programs: impl IntoIterator<Item = &'a Program>,
) -> Result<(), Error> {
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
        let source = match (&call.namespace, call.member) {
            (Some(namespace), _) => format!("{namespace}.{}()", call.name),
            (None, true) => format!("null.{}()", call.name),
            (None, false) => format!("{}()", call.name),
        };
        let Ok(probe) = lighthouse_spec::compile(&source) else {
            continue;
        };
        if let Err(ExecutionError::UndeclaredReference(_)) = probe.execute(&scope) {
            return Err(Error::invalid(
                id,
                format!(
                    "`{}` is not a function of CEL, of the checks library or of the variables of the check",
                    match &call.namespace {
                        Some(namespace) => format!("{namespace}.{}", call.name),
                        None => call.name.clone(),
                    }
                ),
            ));
        }
    }
    Ok(())
}
