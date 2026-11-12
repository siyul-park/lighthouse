//! The solver of the levels against every assignment of small problems.

use lighthouse_checks::level::solve::{Problem, Term};

/// A xorshift generator: the cases are the same on every run.
struct Seed(u64);

impl Seed {
    fn next(&mut self, below: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % below
    }
}

const TOP: i64 = 9;

/// What every assignment in `1..=TOP` (a frozen variable at its start) costs
/// least, the componentwise least of the optimal ones, and how many are
/// optimal.
fn brute(problem: &Problem, start: &[i64]) -> Option<(i128, Vec<i64>, usize)> {
    let n = problem.slope.len();
    let mut p = vec![1i64; n];
    let mut best: Option<(i128, Vec<i64>, usize)> = None;
    loop {
        let frozen_ok = (0..n).all(|v| !problem.frozen[v] || p[v] == start[v]);
        if let Some(cost) = problem.cost(&p).filter(|_| frozen_ok) {
            match &mut best {
                Some((c, _, _)) if *c < cost => {}
                Some((c, least, optimal)) if *c == cost => {
                    *optimal += 1;
                    for (l, &q) in least.iter_mut().zip(&p) {
                        *l = (*l).min(q);
                    }
                }
                _ => best = Some((cost, p.clone(), 1)),
            }
        }
        let mut at = 0;
        while at < n {
            if p[at] < TOP {
                p[at] += 1;
                break;
            }
            p[at] = 1;
            at += 1;
        }
        if at == n {
            return best;
        }
    }
}

fn random(seed: &mut Seed) -> Problem {
    let n = 2 + seed.next(4) as usize;
    let mut problem = Problem::new(n);
    for v in 0..n {
        if seed.next(3) == 0 {
            problem.slope[v] = 1 + seed.next(4) as i64;
        }
        problem.frozen[v] = seed.next(8) == 0;
    }
    for _ in 0..seed.next(8) + 1 {
        let (a, b) = (seed.next(n as u64) as usize, seed.next(n as u64) as usize);
        if a == b {
            continue;
        }
        let weight = 1 + seed.next(5) as i64;
        let term = match seed.next(4) {
            0 => Term::Hard {
                floor: seed.next(2) as i64,
            },
            1 => Term::Linear {
                floor: seed.next(2) as i64,
                weight,
            },
            _ => Term::Distance { target: 1, weight },
        };
        problem.arcs.push((a, b, term));
    }
    problem
}

#[test]
fn the_cost_of_the_solution_is_the_least_over_all_levels() {
    let mut seed = Seed(0x9E37_79B9_7F4A_7C15);
    let (mut checked, mut moved, mut ties) = (0, 0, 0);
    for _ in 0..4000 {
        let problem = random(&mut seed);
        let Ok(start) = problem.start() else {
            continue;
        };
        if start.iter().any(|&p| p > TOP) || problem.cost(&start).is_none() {
            continue;
        }
        let Some((cost, least, optimal)) = brute(&problem, &start) else {
            continue;
        };
        let solved = problem.solve(start).expect("a feasible problem solves");
        assert_eq!(
            solved.cost, cost,
            "{problem:?} solved to {:?}",
            solved.levels
        );
        assert_eq!(solved.levels, least, "the least optimum of {problem:?}");
        assert!(solved.cost <= solved.start_cost);
        checked += 1;
        moved += usize::from(solved.levels != solved.start);
        ties += usize::from(optimal > 1);
    }
    assert!(checked >= 2000, "{checked} cases checked");
    assert!(moved >= 200, "{moved} cases moved off the longest paths");
    assert!(ties >= 100, "{ties} cases have several optimal levels");
}

#[test]
fn a_chain_stays_at_its_longest_path() {
    let mut problem = Problem::new(3);
    problem.arcs.push((
        1,
        0,
        Term::Linear {
            floor: 1,
            weight: 4,
        },
    ));
    problem.arcs.push((
        2,
        1,
        Term::Linear {
            floor: 1,
            weight: 4,
        },
    ));
    let solved = problem.solve(problem.start().unwrap()).unwrap();
    assert_eq!(solved.levels, [1, 2, 3]);
    assert_eq!(solved.rounds, 0);
}

#[test]
fn a_positive_cycle_of_floors_has_no_levels() {
    let mut problem = Problem::new(2);
    problem.arcs.push((0, 1, Term::Hard { floor: 1 }));
    problem.arcs.push((1, 0, Term::Hard { floor: 1 }));
    assert!(problem.start().is_err());
}
