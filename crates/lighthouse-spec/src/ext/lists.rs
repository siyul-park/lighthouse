//! The `indexOf` and `lastIndexOf` of a list, as the Kubernetes CEL library
//! defines them: `list.indexOf(value)` and `list.lastIndexOf(value)` give the
//! index of the first or the last element equal to `value`, `-1` when there is
//! none. Equality is CEL's.

use cel::{
    DeclarationError, Env, ExecutionError,
    common::{
        types::{CelInt, DYN_TYPE, LIST_TYPE},
        value::CowVal,
    },
};

use super::take_args;

/// Registers the overloads on `env`.
pub(crate) fn install(env: &mut Env) -> Result<(), DeclarationError> {
    env.add_member_overload(
        "indexOf",
        "list.indexOf(dyn)",
        LIST_TYPE,
        vec![DYN_TYPE],
        index_of,
    )?;
    env.add_member_overload(
        "lastIndexOf",
        "list.lastIndexOf(dyn)",
        LIST_TYPE,
        vec![DYN_TYPE],
        last_index_of,
    )
}

fn index_of<'b, 'v>(args: Vec<CowVal<'b, 'v>>) -> Result<CowVal<'b, 'v>, ExecutionError> {
    find(args, false)
}

fn last_index_of<'b, 'v>(args: Vec<CowVal<'b, 'v>>) -> Result<CowVal<'b, 'v>, ExecutionError> {
    find(args, true)
}

fn find<'b, 'v>(args: Vec<CowVal<'b, 'v>>, last: bool) -> Result<CowVal<'b, 'v>, ExecutionError> {
    let [list, value] = take_args(args)?;
    let list = list
        .as_iterable()
        .ok_or_else(|| ExecutionError::UnexpectedType {
            got: list.get_type().name().to_owned(),
            want: "iterable".to_owned(),
        })?;
    let mut items = list.iter();
    let mut found: Option<i64> = None;
    let mut at = 0_i64;
    while let Some(item) = items.next() {
        if item.equals(value.as_ref()) {
            found = Some(at);
            if !last {
                break;
            }
        }
        at += 1;
    }
    Ok(CowVal::owned(CelInt::from(found.unwrap_or(-1))))
}
