//! The formulas between the expression and the query: negation is pushed to
//! the comparisons, and the disjunctive normal form is the union of
//! conjunctions.

use crate::ir::{Atom, Cmp, Cq, Path};

use super::{R, Reject};

/// The most conjunctions a decision may expand to.
const MAX_CQS: usize = 4096;

/// The most literals one conjunction may hold.
const MAX_LITERALS: usize = 256;

/// A condition on the subject, before it is a query.
#[derive(Debug, Clone)]
pub(super) enum F {
    Lit(bool),
    Cmp(Cmp),
    /// Some member of `list` satisfies `cond`.
    Has {
        binder: usize,
        list: Path,
        cond: Box<F>,
    },
    /// The container (a list, a map or a text) has no element.
    Empty(Path),
    /// The container has some element.
    Filled(Path),
    And(Vec<F>),
    Or(Vec<F>),
}

impl F {
    /// The union of conjunctions this is.
    pub(super) fn dnf(&self) -> R<Vec<Cq>> {
        match self {
            Self::Lit(true) => Ok(vec![Cq::default()]),
            Self::Lit(false) => Ok(Vec::new()),
            Self::Cmp(cmp) => Ok(vec![Cq {
                cmps: vec![cmp.clone()],
                ..Cq::default()
            }]),
            Self::Empty(path) => Ok(vec![Cq {
                empties: vec![path.clone()],
                ..Cq::default()
            }]),
            Self::Filled(path) => Ok(vec![Cq {
                filled: vec![path.clone()],
                ..Cq::default()
            }]),
            Self::Has { binder, list, cond } => Ok(cond
                .dnf()?
                .into_iter()
                .map(|mut cq| {
                    cq.atoms.insert(
                        0,
                        Atom {
                            binder: *binder,
                            list: list.clone(),
                        },
                    );
                    cq
                })
                .collect()),
            Self::Or(parts) => {
                let mut all = Vec::new();
                for part in parts {
                    all.extend(part.dnf()?);
                    too_many(all.len())?;
                }
                Ok(all)
            }
            Self::And(parts) => {
                let mut all = vec![Cq::default()];
                for part in parts {
                    let next = part.dnf()?;
                    too_many(all.len().saturating_mul(next.len()))?;
                    all = all
                        .iter()
                        .flat_map(|a| next.iter().map(move |b| merged(a, b)))
                        .map(|cq| {
                            let literals =
                                cq.atoms.len() + cq.cmps.len() + cq.empties.len() + cq.filled.len();
                            if literals > MAX_LITERALS {
                                return Err(Reject::new("a conjunction of too many conditions"));
                            }
                            Ok(cq)
                        })
                        .collect::<R<_>>()?;
                }
                Ok(all)
            }
        }
    }
}

fn too_many(size: usize) -> R<()> {
    if size > MAX_CQS {
        return Err(Reject::new("a condition that expands to too many cases"));
    }
    Ok(())
}

fn merged(a: &Cq, b: &Cq) -> Cq {
    Cq {
        atoms: [a.atoms.clone(), b.atoms.clone()].concat(),
        cmps: [a.cmps.clone(), b.cmps.clone()].concat(),
        empties: [a.empties.clone(), b.empties.clone()].concat(),
        filled: [a.filled.clone(), b.filled.clone()].concat(),
    }
}
