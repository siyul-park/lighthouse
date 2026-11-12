//! The decisions of a project read as logic. A cel check is a violation
//! predicate over one selected subject; the fragment of CEL that is a union of
//! conjunctive queries with comparisons is translated into [`Ucq`], which the
//! evaluator runs over the same JSON facts the checks read.

mod eval;
mod ir;
mod tier;
mod translate;

pub use ir::{Atom, Cmp, Const, Cq, Op, Operand, Path, Root, Ucq};
pub use tier::{Classified, Tier, tier};
pub use translate::{Reject, translate};
