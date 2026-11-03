//! The order a file loads in: in a language that evaluates declarations as
//! they are written, a declaration is never put above a binding that its code
//! needs as the file loads, and a binding is never put below code that needs
//! it (a decorator, a default argument, an initializer, a class base, the
//! callback of a test suite, top-level code, and what they call). Needs that
//! the file already breaks are left as they are.

use std::{collections::BTreeMap, ops::Range, path::Path};

use lighthouse_model::{
    LineIndex, Position, Span,
    ucm::{LoadNeed, LoadPoint, Symbol, SymbolId},
};

use super::Lowerer;

/// A block of whole lines put at `at`, among those put there in the order of
/// `seq`; the lines it fills and the module it is of.
#[derive(Debug, Clone)]
pub(super) struct Placed {
    pub lines: Range<usize>,
    pub at: usize,
    pub seq: usize,
    pub module: String,
}

impl Lowerer<'_> {
    /// Whether the `placed` blocks of `file` break no need that the file keeps.
    pub(super) fn keeps_load_order(
        &self,
        file: &Path,
        text: &str,
        placed: &[Placed],
    ) -> Result<(), String> {
        if placed.is_empty() {
            return Ok(());
        }
        let index = LineIndex::new(text);
        let offset = |at: Position| index.range(Span { start: at, end: at }).map(|r| r.start);
        for need in self.project.load_needs(file) {
            let (Some(user), Some(binding)) = (
                self.project.load_place(&need.user).and_then(offset),
                self.project
                    .symbol(&need.binding)
                    .and_then(|s| offset(start_of(s))),
            ) else {
                continue;
            };
            let kept = binding < user;
            let still = Standing::of(binding, placed) < Standing::of(user, placed);
            if kept && !still {
                return Err(self.breaks(&need, text, user));
            }
        }
        Ok(())
    }

    fn breaks(&self, need: &LoadNeed, text: &str, user: usize) -> String {
        let name = |id: &SymbolId| {
            self.project
                .symbol(id)
                .map_or_else(|| id.as_str().to_owned(), |s| s.name.clone())
        };
        let who = match &need.user {
            LoadPoint::Declaration(id) => format!("`{}`", name(id)),
            LoadPoint::Site(_) => format!(
                "the code of the file on line {}",
                text[..user].matches('\n').count() + 1
            ),
        };
        format!(
            "the move would break the load order of the file: {who} needs `{}` bound above it as the file loads",
            name(&need.binding)
        )
    }

    /// The order, as indices into `wanted`, that moves the declarations the
    /// file loads in an order just enough that each follows the bindings it
    /// needs among them, the rest keeping the order given. Declined when the
    /// needs allow no order.
    pub(super) fn respecting(
        &self,
        file: &Path,
        wanted: &[&SymbolId],
    ) -> Result<Vec<usize>, String> {
        let place = |id: &SymbolId| self.project.symbol(id).map(start_of);
        let mut after: BTreeMap<&SymbolId, Vec<&SymbolId>> = BTreeMap::new();
        let needs = self.project.load_needs(file);
        for need in &needs {
            let LoadPoint::Declaration(user) = &need.user else {
                continue;
            };
            let (Some(u), Some(b)) = (place(user), place(&need.binding)) else {
                continue;
            };
            if b < u && wanted.contains(&user) && wanted.contains(&&need.binding) {
                after.entry(user).or_default().push(&need.binding);
            }
        }
        let mut left: Vec<usize> = (0..wanted.len()).collect();
        let mut out = Vec::with_capacity(left.len());
        while !left.is_empty() {
            let ready = left
                .iter()
                .position(|&n| is_ready(wanted[n], &after, wanted, &left));
            let Some(next) = ready else {
                return Err("the order wanted puts a declaration above one that it needs as the file loads, and no order keeps both".to_owned());
            };
            out.push(left.remove(next));
        }
        Ok(out)
    }
}

/// Where code stands once the placements are made: moved blocks take the
/// place they are put at, before what already stands there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Standing {
    at: usize,
    stays: bool,
    seq: usize,
    within: usize,
}

impl Standing {
    fn of(offset: usize, placed: &[Placed]) -> Self {
        match placed.iter().find(|p| p.lines.contains(&offset)) {
            Some(p) => Self {
                at: p.at,
                stays: false,
                seq: p.seq,
                within: offset - p.lines.start,
            },
            None => Self {
                at: offset,
                stays: true,
                seq: 0,
                within: 0,
            },
        }
    }
}

/// Whether none of the bindings `id` needs is still waiting in `left`.
fn is_ready(
    id: &SymbolId,
    after: &BTreeMap<&SymbolId, Vec<&SymbolId>>,
    wanted: &[&SymbolId],
    left: &[usize],
) -> bool {
    after.get(id).is_none_or(|needed| {
        needed
            .iter()
            .all(|n| !left.iter().any(|&l| wanted[l] == *n))
    })
}

fn start_of(symbol: &Symbol) -> Position {
    symbol.extent.unwrap_or(symbol.span).start
}
