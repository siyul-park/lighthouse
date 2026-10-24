//! `list.misplaced()` and `list.anchors()`, members of a list of ints, for the
//! decisions that order siblings.
//!
//! Nothing official has them: the nearest precedents are `sort` and `sortBy`
//! (and the Kubernetes `isSorted()`), which order a list or say whether it is
//! ordered, but not which elements are out of place or where the next smaller
//! one is. They are functions because no CEL expression computes them in
//! linear time (a comprehension over the elements before each one is
//! quadratic). One word, lowerCamelCase, no algorithm term.

use cel::{
    DeclarationError, Env, ExecutionError,
    common::{
        types::{CelInt, CelList, LIST_TYPE},
        value::{CowVal, Val},
    },
};
use lighthouse_plugin::{Overload, Precedence};

use crate::{Library, function, take_args};

/// The library of the two functions.
pub(crate) fn library() -> Library {
    let order = |name: &str, doc: &str, example: &str| {
        function(
            name,
            Some("list"),
            vec![Overload::new(&[], "list<int>")],
            doc,
            example,
            Precedence::Designed,
        )
    };
    Library::new(
        "order",
        vec![
            order(
                "misplaced",
                "The indexes of the elements of a list of ints that must move for the list to be in order, with the fewest moves: the ones the first longest run already in order leaves out. Empty exactly when the list is sorted.",
                "[3, 0, 1, 2].misplaced() == [0]",
            ),
            order(
                "anchors",
                "For each element of a list of ints, the index of the first later element that sorts before it, -1 when there is none: the element it must come after to be in order.",
                "[3, 1, 2, 0].anchors() == [1, 3, 3, -1]",
            ),
        ],
        install,
    )
}

fn install(env: &mut Env) -> Result<(), DeclarationError> {
    env.add_member_overload(
        "misplaced",
        "list.misplaced()",
        LIST_TYPE,
        vec![],
        misplaced,
    )?;
    env.add_member_overload("anchors", "list.anchors()", LIST_TYPE, vec![], anchors)
}

/// `[10, 10, 0].misplaced() == [2]`; O(n log n).
fn misplaced<'b, 'v>(args: Vec<CowVal<'b, 'v>>) -> Result<CowVal<'b, 'v>, ExecutionError> {
    let [list] = take_args(args)?;
    let keys = ints(list.as_ref(), "misplaced")?;
    let mut stays = vec![false; keys.len()];
    for i in kept_run(&keys) {
        stays[i] = true;
    }
    Ok(indexes(
        (0..keys.len())
            .filter(|&i| !stays[i])
            .map(|i| i64::try_from(i).unwrap_or(i64::MAX)),
    ))
}

/// `[3, 1, 2, 0].anchors() == [1, 3, 3, -1]`; O(n).
fn anchors<'b, 'v>(args: Vec<CowVal<'b, 'v>>) -> Result<CowVal<'b, 'v>, ExecutionError> {
    let [list] = take_args(args)?;
    let keys = ints(list.as_ref(), "anchors")?;
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
    Ok(indexes(found))
}

fn indexes<'b, 'v>(values: impl IntoIterator<Item = i64>) -> CowVal<'b, 'v> {
    let list: Vec<Box<dyn Val>> = values
        .into_iter()
        .map(|i| Box::new(CelInt::from(i)) as Box<dyn Val>)
        .collect();
    CowVal::owned(CelList::from(list))
}

/// The ints of a list value; anything else in it is an error of `function`.
fn ints(list: &dyn Val, function: &str) -> Result<Vec<i64>, ExecutionError> {
    let list = list
        .as_iterable()
        .ok_or_else(|| ExecutionError::UnexpectedType {
            got: list.get_type().name().to_owned(),
            want: "list".to_owned(),
        })?;
    let mut items = list.iter();
    let mut keys = Vec::new();
    while let Some(item) = items.next() {
        let Some(int) = item.downcast_ref::<CelInt>() else {
            return Err(ExecutionError::function_error(
                function,
                format!("the list holds ints, not {}", item.get_type().name()),
            ));
        };
        keys.push(*int.inner());
    }
    Ok(keys)
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
