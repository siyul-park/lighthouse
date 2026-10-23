//! The variables of a check that have parameters: functions of CEL written in
//! the decision or its pack, such as `group(n)`.

use std::sync::Arc;

use cel::{Context, ExecutionError, FunctionContext, Program, Value, extractors::Arguments};
use lighthouse_spec::Variable;

use lighthouse_spec::Cel;

use crate::{Error, eval::compile_shared};

/// What [`declare`] made of the variables of a check.
pub(crate) struct Declared<'a> {
    /// The variables that are values.
    pub(crate) values: Vec<&'a Variable>,
    /// The variables that are functions, and their compiled bodies.
    pub(crate) functions: Vec<&'a Variable>,
    pub(crate) bodies: Vec<Arc<Program>>,
}

/// Declares the variables that have parameters as functions of `context`, and
/// gives the others, with the compiled bodies of the functions. A function sees the `options` and the `file` of its
/// caller, besides its arguments.
pub(crate) fn declare<'a>(
    cel: &Cel,
    id: &str,
    context: &mut Context<'static, 'static>,
    variables: &'a [Variable],
) -> Result<Declared<'a>, Error> {
    let mut values = Vec::new();
    let mut functions = Vec::new();
    let mut bodies = Vec::new();
    for variable in variables {
        if variable.params.is_empty() {
            values.push(variable);
            continue;
        }
        let body = compile_shared(
            cel,
            id,
            &format!("variable `{}`", variable.name),
            &variable.expression,
        )?;
        context
            .add_function(&variable.name, call(variable.params.clone(), body.clone()))
            .map_err(|e| Error::invalid(id, format!("variable `{}`: {e}", variable.name)))?;
        functions.push(variable);
        bodies.push(body);
    }
    Ok(Declared {
        values,
        functions,
        bodies,
    })
}

/// Refuses a function variable whose body reads anything but its parameters,
/// `options`, `file`, or a variable of `shared` (the file-level ones): a
/// function does not see the variable of whoever calls it.
pub(crate) fn check_scope(
    cel: &Cel,
    id: &str,
    functions: &[&Variable],
    bodies: &[Arc<Program>],
    shared: &dyn Fn(&str) -> bool,
) -> Result<(), Error> {
    for (variable, body) in functions.iter().zip(bodies) {
        if let Some(name) = lighthouse_spec::free_variables(cel, body)
            .into_iter()
            .find(|name| {
                !variable.params.contains(name)
                    && name != "options"
                    && name != "file"
                    && !shared(name)
            })
        {
            return Err(Error::invalid(
                id,
                format!(
                    "variable `{}`: `{name}` is not a parameter, `options`, `file` or a file-level variable; a function sees only those",
                    variable.name
                ),
            ));
        }
    }
    Ok(())
}

/// Per value variable, whether `roots` read it: by name, through another
/// variable it is read by, or through a function variable they call. A
/// variable that nothing reads is not evaluated.
pub(crate) fn reached(
    values: &[(String, Arc<Program>)],
    functions: &[&Variable],
    bodies: &[Arc<Program>],
    roots: &[&Program],
) -> Vec<bool> {
    let mut used = vec![false; values.len()];
    let mut called = vec![false; functions.len()];
    let mut pending: Vec<&Program> = roots.to_vec();
    while let Some(program) = pending.pop() {
        for name in lighthouse_spec::program_identifiers(program) {
            if let Some(at) = values.iter().position(|(n, _)| *n == name)
                && !std::mem::replace(&mut used[at], true)
            {
                pending.push(values[at].1.as_ref());
            }
            if let Some(at) = functions.iter().position(|f| f.name == name)
                && !std::mem::replace(&mut called[at], true)
            {
                pending.push(bodies[at].as_ref());
            }
        }
    }
    used
}

fn call(
    params: Vec<String>,
    body: Arc<Program>,
) -> impl Fn(&FunctionContext, Arguments) -> Result<Value, ExecutionError> + Send + Sync + 'static {
    move |ftx: &FunctionContext, Arguments(args): Arguments| {
        if args.len() != params.len() {
            return Err(ExecutionError::invalid_argument_count(
                params.len(),
                args.len(),
            ));
        }
        let mut scope = ftx.ptx.new_inner_scope();
        for (param, arg) in params.iter().zip(args.iter()) {
            scope.add_variable_from_value(param.clone(), arg.clone());
        }
        body.execute(&scope)
    }
}
