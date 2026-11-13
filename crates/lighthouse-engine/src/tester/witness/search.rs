//! The reading of the flags: for every shape, which arrangements which
//! decisions flag, the nogoods the unsatisfiable shapes teach, and what shows
//! each of them.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_logic::{Assumption, Env, Nogoods, hitting_sets, is_minimal_hitting};

use super::{
    Alphabet, Arrangement, Contradiction, Flagged, Forced, Language, Shape, Witness, text,
};

/// The rows of every shape of a search, smallest first.
pub(super) struct Table {
    pub(super) rows: Vec<Row>,
}

/// A shape, its arrangements and the decisions that flag each.
pub(super) struct Row {
    pub(super) shape: Shape,
    arrangements: Vec<Arrangement>,
    flagged: Vec<Env>,
}

/// What the shapes taught: the nogoods and what keeps the search from being
/// complete.
pub(super) struct Search {
    pub(super) nogoods: Nogoods,
    /// The first shape that showed each nogood: the smallest, as the shapes
    /// come smallest first.
    pub(super) shapes: BTreeMap<Env, usize>,
    pub(super) reasons: Vec<String>,
}

impl Table {
    /// The rows of `shapes`: the packages of the scratch project are numbered
    /// in the order of the shapes and of their arrangements, as it was
    /// rendered.
    pub(super) fn of(shapes: &[Shape], flags: &BTreeMap<Assumption, BTreeSet<usize>>) -> Self {
        let mut base = 0;
        let mut rows = Vec::new();
        for shape in shapes {
            let arrangements = shape.arrangements();
            let flagged = (0..arrangements.len())
                .map(|at| {
                    let unit = base + at;
                    Env::new(
                        flags
                            .iter()
                            .filter(|(_, units)| units.contains(&unit))
                            .map(|(decision, _)| decision.clone()),
                    )
                })
                .collect();
            base += arrangements.len();
            rows.push(Row {
                shape: shape.clone(),
                arrangements,
                flagged,
            });
        }
        Self { rows }
    }

    /// The contradiction of `env`, shown by its first shape.
    pub(super) fn contradiction(
        &self,
        env: &Env,
        at: usize,
        language: Language,
        alphabet: Alphabet,
    ) -> Contradiction {
        let row = &self.rows[at];
        let by = |flagged: &Env| -> Vec<String> {
            flagged
                .iter()
                .filter(|decision| env.has(decision))
                .map(|decision| decision.uid.clone())
                .collect()
        };
        let arrangements = row
            .arrangements
            .iter()
            .zip(&row.flagged)
            .map(|(arrangement, flagged)| Flagged {
                arrangement: arrangement.clone(),
                by: by(flagged),
                text: text::files(language, alphabet, &row.shape, arrangement, "p0")
                    .into_iter()
                    .map(|(_, body)| body)
                    .collect::<Vec<_>>()
                    .join("\n"),
            })
            .collect();
        Contradiction {
            decisions: env.iter().map(|decision| decision.uid.clone()).collect(),
            witness: Witness {
                shape: row.shape.clone(),
                arrangements,
                forced: forced(row, env),
            },
        }
    }
}

impl Search {
    /// What `table` teaches. A decision must flag some arrangements of a
    /// shape of up to `bound` declarations and not others, else the alphabet
    /// cannot show what it asks of the order and the search is incomplete.
    pub(super) fn over<'a>(
        table: &Table,
        decisions: impl Iterator<Item = &'a Assumption>,
        bound: usize,
    ) -> Self {
        let mut search = Self {
            nogoods: Nogoods::default(),
            shapes: BTreeMap::new(),
            reasons: Vec::new(),
        };
        let mut sensitive = BTreeSet::new();
        for (at, row) in table.rows.iter().enumerate() {
            sensitivity(row, bound, &mut sensitive);
            search.row(at, row);
        }
        for decision in decisions.filter(|decision| !sensitive.contains(decision)) {
            search.reasons.push(format!(
                "`{}` flags no arrangement differently on any shape of up to {bound} \
                 declarations: the alphabet cannot show the order it asks",
                decision.uid
            ));
        }
        search
    }

    /// Learns what an unsatisfiable shape says: its minimal hitting sets are
    /// nogoods, one decision alone included, which then refuses every set that
    /// holds it.
    fn row(&mut self, at: usize, row: &Row) {
        if row.flagged.iter().any(Env::is_empty) {
            return;
        }
        match hitting_sets(&row.flagged) {
            Err(unsolved) => self.reasons.push(format!("shape {at}: {unsolved}")),
            Ok(label) => {
                for env in label.iter() {
                    debug_assert!(is_minimal_hitting(env, &row.flagged));
                    self.learn(at, env);
                }
            }
        }
    }

    fn learn(&mut self, at: usize, env: &Env) {
        if self.nogoods.insert(env.clone()) {
            self.shapes
                .retain(|stored, _| self.nogoods.contains(stored));
            self.shapes.insert(env.clone(), at);
        }
    }
}

/// The decisions that flag some arrangements of a small shape and not
/// others: those whose order the alphabet can see.
fn sensitivity(row: &Row, bound: usize, sensitive: &mut BTreeSet<Assumption>) {
    if row.shape.size() > bound || row.flagged.len() < 2 {
        return;
    }
    let decisions: BTreeSet<&Assumption> = row.flagged.iter().flat_map(Env::iter).collect();
    for decision in decisions {
        let count = row
            .flagged
            .iter()
            .filter(|flagged| flagged.has(decision))
            .count();
        if count < row.flagged.len() {
            sensitive.insert(decision.clone());
        }
    }
}

/// The order each decision of `env` forces on two declarations of the row: the
/// decision flags every arrangement that puts them the other way round (and
/// at least one does).
fn forced(row: &Row, env: &Env) -> Vec<Forced> {
    let above = |arrangement: &Arrangement, first: usize, second: usize| {
        let at = |decl: usize| arrangement.source.iter().position(|&d| d == decl);
        at(first) < at(second)
    };
    let mut forced = Vec::new();
    for first in 0..row.shape.decls.len() {
        for second in 0..row.shape.decls.len() {
            let reversed: Vec<&Env> = row
                .arrangements
                .iter()
                .zip(&row.flagged)
                .filter(|(arrangement, _)| !above(arrangement, first, second))
                .map(|(_, flagged)| flagged)
                .collect();
            let by: Vec<String> = env
                .iter()
                .filter(|decision| reversed.iter().all(|flagged| flagged.has(decision)))
                .map(|decision| decision.uid.clone())
                .collect();
            if first != second && !reversed.is_empty() && !by.is_empty() {
                forced.push(Forced { first, second, by });
            }
        }
    }
    forced
}
