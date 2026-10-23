//! The functions of CEL expressions that the model needs besides the standard
//! library and the official extensions (see `lighthouse_spec`'s `ext`):
//! `list.misplaced()` and `list.anchors()`. What the code model says is a field of a
//! node, built only for the expressions that read it; see [`Needs`].

use std::sync::Arc;

use cel::{
    Context, Value,
    extractors::{IntoFunction, This},
};

mod functions;
mod needs;
mod variables;
pub(crate) use functions::check_calls;
pub(crate) use needs::Needs;
pub(crate) use variables::{check_scope, declare as declare_variables, reached};

/// A context of the shared CEL environment with the library installed.
pub(crate) fn context() -> Result<Context<'static, 'static>, String> {
    let mut context = lighthouse_spec::context()?;
    declare(&mut context, "misplaced", misplaced)?;
    declare(&mut context, "anchors", anchors)?;
    Ok(context)
}

/// Declares `function` as `name`, or says why the environment refuses it.
fn declare<T: 'static, F>(
    context: &mut Context<'static, 'static>,
    name: &str,
    function: F,
) -> Result<(), String>
where
    F: IntoFunction<T> + 'static + Send + Sync,
{
    context
        .add_function(name, function)
        .map_err(|e| format!("the CEL function `{name}`: {e}"))
}

/// `keys.misplaced()`: the indexes of the elements of a list of ints that
/// must move for the list to be in order, with the fewest moves: the ones a
/// longest run already in order leaves out. The run kept is the first of the
/// longest ones (the one that ends at the lowest index, and at each step the
/// earliest predecessor), so the answer is the same from run to run. It is
/// empty exactly when `keys.isSorted()`.
///
/// ```text
/// [3, 0, 1, 2].misplaced() == [0]
/// [10, 10, 0].misplaced() == [2]
/// ```
///
/// The nearest official precedent is `sort` and `sortBy` (and the Kubernetes
/// `isSorted()`, which answers only whether the list is in order): they order
/// a list or say it is ordered, but not which elements are out of place. It is
/// a function because no CEL form computes it in linear time; it runs in
/// O(n log n).
fn misplaced(This(keys): This<Arc<Vec<Value>>>) -> Result<Arc<Vec<Value>>, cel::ExecutionError> {
    let keys = ints(&keys, "misplaced")?;
    let mut stays = vec![false; keys.len()];
    for i in kept_run(&keys) {
        stays[i] = true;
    }
    Ok(Arc::new(
        (0..keys.len())
            .filter(|&i| !stays[i])
            .map(|i| Value::Int(i64::try_from(i).unwrap_or(i64::MAX)))
            .collect(),
    ))
}

/// `keys.anchors()`: for each element, the index of the first later element
/// that sorts before it, -1 when there is none: the element it must come
/// after to be in order.
///
/// ```text
/// [3, 1, 2, 0].anchors() == [1, 3, 3, -1]
/// ```
///
/// It has no official precedent either; it answers in one pass what a scan
/// per element would, which an expression cannot do in linear time.
fn anchors(This(keys): This<Arc<Vec<Value>>>) -> Result<Arc<Vec<Value>>, cel::ExecutionError> {
    let keys = ints(&keys, "anchors")?;
    let mut found = vec![-1_i64; keys.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (j, key) in keys.iter().enumerate() {
        while let Some(&i) = stack.last() {
            if keys[i] <= *key {
                break;
            }
            found[i] = i64::try_from(j).unwrap_or(i64::MAX);
            stack.pop();
        }
        stack.push(j);
    }
    Ok(Arc::new(found.into_iter().map(Value::Int).collect()))
}

/// The ints of a list value; anything else in it is an error of `function`.
fn ints(values: &[Value], function: &str) -> Result<Vec<i64>, cel::ExecutionError> {
    values
        .iter()
        .map(|value| match value {
            Value::Int(i) => Ok(*i),
            other => Err(cel::ExecutionError::function_error(
                function,
                format!("the list holds ints, not {other:?}"),
            )),
        })
        .collect()
}

/// The indexes of the first longest run of `ranks` that is already in order.
///
/// Elements are grouped by the length of the longest run that ends at them
/// (their level). Within a level the values never increase with the index,
/// and the last value of each level never decreases with the level, so the
/// level of an element, and its earliest predecessor in the level below, are
/// found by binary search.
fn kept_run(ranks: &[i64]) -> Vec<usize> {
    let mut levels: Vec<Vec<usize>> = Vec::new();
    let mut from = vec![usize::MAX; ranks.len()];
    for (i, &rank) in ranks.iter().enumerate() {
        let level = levels.partition_point(|l| l.last().is_some_and(|&j| ranks[j] <= rank));
        if let Some(below) = level.checked_sub(1).map(|l| &levels[l]) {
            from[i] = below[below.partition_point(|&j| ranks[j] > rank)];
        }
        match levels.get_mut(level) {
            Some(same) => same.push(i),
            None => levels.push(vec![i]),
        }
    }
    let mut kept = Vec::new();
    let mut at = levels.last().and_then(|l| l.first().copied());
    while let Some(i) = at {
        kept.push(i);
        at = Some(from[i]).filter(|&j| j != usize::MAX);
    }
    kept.reverse();
    kept
}
