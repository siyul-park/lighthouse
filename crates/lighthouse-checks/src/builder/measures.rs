//! What the metric analyzers measured for the functions of one file.

use std::collections::BTreeMap;

use lighthouse_model::SymbolId;
use lighthouse_plugin::{Ctx, Error};

use crate::metrics::{COGNITIVE, CYCLOMATIC, FAN, Fan, NESTING, SIZE, Size, read};

/// What was measured for the functions of one file.
#[derive(Default)]
pub(super) struct Measures {
    pub(super) sizes: BTreeMap<SymbolId, Size>,
    pub(super) cyclomatic: BTreeMap<SymbolId, u32>,
    pub(super) cognitive: BTreeMap<SymbolId, u32>,
    pub(super) nesting: BTreeMap<SymbolId, u32>,
    pub(super) fans: BTreeMap<SymbolId, Fan>,
}

pub(super) fn measures(ctx: &Ctx) -> Result<Measures, Error> {
    Ok(Measures {
        sizes: read(ctx, SIZE)?,
        cyclomatic: read(ctx, CYCLOMATIC)?,
        cognitive: read(ctx, COGNITIVE)?,
        nesting: read(ctx, NESTING)?,
        fans: read(ctx, FAN)?,
    })
}
