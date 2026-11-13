//! The decisions of a project read as logic. A cel check is a violation
//! predicate over one selected subject; the fragment of CEL that is a union of
//! conjunctive queries with comparisons is translated into [`Ucq`], which the
//! evaluator runs over the same JSON facts the checks read.

mod eval;
mod ir;
mod nogood;
mod tier;
mod translate;

pub use ir::{Atom, Cmp, Const, Cq, Op, Operand, Path, Root, Ucq};
pub use nogood::{
    Assumption, Env, Label, MAX_DECISIONS, Nogoods, Unsolved, digest, hitting_sets, is_hitting,
    is_minimal_hitting, is_resolved,
};
pub use tier::{Classified, Tier, tier};
pub use translate::{Construct, Reject, translate};
