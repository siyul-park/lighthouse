//! `list.misplaced()`, `list.anchors()` and `list.usersFirst()`, members of a
//! list, for the decisions that order siblings.
//!
//! Nothing official has them: the nearest precedents are `sort` and `sortBy`
//! (and the Kubernetes `isSorted()`), which order a list or say whether it is
//! ordered, but not which elements are out of place or where the next smaller
//! one is. They are functions because no CEL expression computes them in
//! linear time (a comprehension over the elements before each one is
//! quadratic). One word, lowerCamelCase, no algorithm term.

use std::{cmp::Reverse, collections::BinaryHeap};

use cel::{
    DeclarationError, Env, ExecutionError,
    common::{
        types::{CelInt, CelList, LIST_TYPE},
        value::{CowVal, Val},
    },
};
use lighthouse_plugin::{Overload, Precedence};

use crate::{Library, function, take_args};

/// The library of the three functions.
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
            order(
                "usersFirst",
                "For a list that holds, per declaration in file order, the indexes of the declarations it uses, the rank of each declaration in the order that keeps file order except that a declaration follows every one of its users. Declarations that use each other, and one that uses itself, keep file order.",
                "[[], [0]].usersFirst() == [1, 0]",
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
    env.add_member_overload("anchors", "list.anchors()", LIST_TYPE, vec![], anchors)?;
    env.add_member_overload(
        "usersFirst",
        "list.usersFirst()",
        LIST_TYPE,
        vec![],
        users_first,
    )
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

/// `[[], [0]].usersFirst() == [1, 0]`; O((n + edges) log n).
fn users_first<'b, 'v>(args: Vec<CowVal<'b, 'v>>) -> Result<CowVal<'b, 'v>, ExecutionError> {
    let [list] = take_args(args)?;
    let callers = int_lists(list.as_ref(), "usersFirst")?;
    Ok(indexes(user_ranks(&callers)))
}

/// The rank of each declaration when, of those whose users are all placed,
/// the one earliest in the file goes next. Uses that lead back to the user are
/// left out, so that declarations that use each other keep file order.
fn user_ranks(uses: &[Vec<usize>]) -> Vec<i64> {
    let n = uses.len();
    let all: Vec<Vec<usize>> = uses
        .iter()
        .map(|found| found.iter().copied().filter(|&used| used < n).collect())
        .collect();
    let mut calls = all.clone();
    let mut waiting = vec![0_usize; n];
    for (caller, callees) in calls.iter_mut().enumerate() {
        callees.sort_unstable();
        callees.dedup();
        callees.retain(|&callee| !reaches(&all, callee, caller));
        for &callee in callees.iter() {
            waiting[callee] += 1;
        }
    }
    let mut ready: BinaryHeap<Reverse<usize>> =
        (0..n).filter(|&i| waiting[i] == 0).map(Reverse).collect();
    let mut ranks = vec![0_i64; n];
    let mut next = 0_i64;
    while let Some(Reverse(at)) = ready.pop() {
        ranks[at] = next;
        next += 1;
        for &callee in &calls[at] {
            waiting[callee] -= 1;
            if waiting[callee] == 0 {
                ready.push(Reverse(callee));
            }
        }
    }
    ranks
}

/// Whether `to` can be reached from `from` along `edges`.
fn reaches(edges: &[Vec<usize>], from: usize, to: usize) -> bool {
    let mut seen = vec![false; edges.len()];
    let mut stack = vec![from];
    while let Some(node) = stack.pop() {
        if node == to {
            return true;
        }
        if !std::mem::replace(&mut seen[node], true) {
            stack.extend(&edges[node]);
        }
    }
    false
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

/// The lists of non-negative ints a list value holds; anything else is an error of `function`.
fn int_lists(list: &dyn Val, function: &str) -> Result<Vec<Vec<usize>>, ExecutionError> {
    let outer = list
        .as_iterable()
        .ok_or_else(|| ExecutionError::UnexpectedType {
            got: list.get_type().name().to_owned(),
            want: "list".to_owned(),
        })?;
    let mut items = outer.iter();
    let mut lists = Vec::new();
    while let Some(item) = items.next() {
        let row: Vec<usize> = ints(item, function)?
            .into_iter()
            .filter_map(|i| usize::try_from(i).ok())
            .collect();
        lists.push(row);
    }
    Ok(lists)
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
