//! CEL extension libraries the `cel` crate lacks, each implemented as its
//! upstream library defines it, under its official names: one module per
//! library, installed in the shared environment.

use cel::{ExecutionError, common::value::CowVal};

pub(crate) mod bindings;
pub(crate) mod glob;
pub(crate) mod lists;
pub(crate) mod regex;

/// `args` as the `N` arguments of an overload.
fn take_args<'b, 'v, const N: usize>(
    args: Vec<CowVal<'b, 'v>>,
) -> Result<[CowVal<'b, 'v>; N], ExecutionError> {
    args.try_into()
        .map_err(|args: Vec<_>| ExecutionError::invalid_argument_count(N, args.len()))
}
