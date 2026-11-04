//! Random edit sequences: after every step the graph equals a graph that
//! never ran, and only the nodes the edit can reach have run.

use std::collections::BTreeSet;

use crate::toy::*;

/// A seeded generator: the sequences are the same on every run.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

/// What an edit is allowed to run.
#[derive(Debug, Clone)]
pub enum Edit {
    Body(String),
    Api(String),
    Add,
    Remove,
    Rename,
    Revert,
    Config,
    Version,
    Evict,
    Noop,
}

pub struct Sim {
    pub project: Project,
    pub known: Vec<String>,
    history: Vec<(String, String)>,
    counter: u64,
}

impl Sim {
    pub fn new() -> Self {
        let mut sim = Self {
            project: Project::new(),
            known: Vec::new(),
            history: Vec::new(),
            counter: 0,
        };
        for name in ["a", "b", "c", "d", "e"] {
            sim.write(name, "");
        }
        sim
    }

    fn fresh(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    fn pick(&self, rng: &mut Rng) -> Option<String> {
        let files: Vec<&String> = self.project.files.keys().collect();
        (!files.is_empty()).then(|| files[rng.below(files.len())].clone())
    }

    fn write(&mut self, path: &str, uses: &str) {
        let n = self.fresh();
        let text = format!("api=A{n};body=B{n};uses={uses}");
        self.set(path, text);
    }

    fn set(&mut self, path: &str, text: String) {
        if !self.known.iter().any(|known| known == path) {
            self.known.push(path.to_owned());
        }
        self.history.push((path.to_owned(), text.clone()));
        self.project.files.insert(path.to_owned(), text);
    }

    fn part(text: &str, key: &str) -> String {
        text.split(';')
            .find_map(|p| p.strip_prefix(&format!("{key}=")).map(str::to_owned))
            .unwrap_or_default()
    }

    /// Picks and applies one edit to the project.
    pub fn step(&mut self, rng: &mut Rng) -> Edit {
        let Some(path) = self.pick(rng) else {
            self.write("z0", "");
            return Edit::Add;
        };
        let text = self.project.files[&path].clone();
        let n = self.fresh();
        match rng.below(11) {
            0..=2 => {
                let edited = format!(
                    "api={};body=B{n};uses={}",
                    Self::part(&text, "api"),
                    Self::part(&text, "uses")
                );
                self.set(&path, edited);
                Edit::Body(path)
            }
            3 | 4 => {
                let edited = format!(
                    "api=A{n};body={};uses={}",
                    Self::part(&text, "body"),
                    Self::part(&text, "uses")
                );
                self.set(&path, edited);
                Edit::Api(path)
            }
            5 => {
                let other = self.pick(rng).unwrap_or_default();
                let name = format!("n{n}");
                self.write(&name, &other);
                Edit::Add
            }
            6 => {
                self.project.files.remove(&path);
                Edit::Remove
            }
            7 => {
                let name = format!("r{n}");
                self.project.files.remove(&path);
                self.set(&name, text);
                Edit::Rename
            }
            8 => self.revert(rng),
            9 => self.global(rng),
            _ => Edit::Noop,
        }
    }

    fn revert(&mut self, rng: &mut Rng) -> Edit {
        let (path, text) = self.history[rng.below(self.history.len())].clone();
        if self.project.files.get(&path) == Some(&text) {
            return Edit::Noop;
        }
        self.project.files.insert(path.clone(), text);
        Edit::Revert
    }

    fn global(&mut self, rng: &mut Rng) -> Edit {
        match rng.below(3) {
            0 => {
                let config = format!("c{}", self.fresh());
                self.project.config = config;
                Edit::Config
            }
            1 => {
                let version = format!("f{}", self.fresh());
                self.project.versions.insert("facts".into(), version);
                Edit::Version
            }
            _ => Edit::Evict,
        }
    }

    /// The files that use `path` in the project now or before the edit.
    pub fn users(&self, path: &str, before: &Project) -> BTreeSet<String> {
        let uses = |project: &Project| -> Vec<String> {
            project
                .files
                .iter()
                .filter(|(_, text)| Self::part(text, "uses").split(',').any(|u| u == path))
                .map(|(name, _)| name.clone())
                .collect()
        };
        uses(&self.project)
            .into_iter()
            .chain(uses(before))
            .collect()
    }
}

/// The paths whose nodes an edit may run; `None` when any may run.
fn reach(sim: &Sim, before: &Project, edit: &Edit) -> Option<BTreeSet<String>> {
    let direct = |path: &String| -> BTreeSet<String> { [path.clone()].into() };
    match edit {
        Edit::Body(path) => Some(direct(path)),
        Edit::Api(path) => Some(
            direct(path)
                .into_iter()
                .chain(sim.users(path, before))
                .collect(),
        ),
        Edit::Config | Edit::Version => Some(BTreeSet::new()),
        Edit::Noop => Some(BTreeSet::new()),
        _ => None,
    }
}

/// `tainted` are the paths whose nodes were evicted: they run again whenever
/// something next pulls them.
fn check_executed(
    sim: &Sim,
    before: &Project,
    edit: &Edit,
    tainted: &BTreeSet<String>,
    ran: &[String],
) {
    let Some(mut allowed) = reach(sim, before, edit) else {
        return;
    };
    allowed.extend(tainted.iter().cloned());
    for label in ran {
        let kind = kind_of(label);
        let global = matches!(edit, Edit::Config | Edit::Version);
        let fine = match kind {
            "report" | "par-report" => true,
            "facts" if global => true,
            "facts" => allowed.contains(path_of(label)),
            _ => (!global && allowed.contains(path_of(label))) || tainted.contains(path_of(label)),
        };
        assert!(
            fine,
            "{edit:?} ran {label}; allowed {allowed:?}; all {ran:?}"
        );
    }
    if matches!(edit, Edit::Noop) && tainted.is_empty() {
        assert!(ran.is_empty(), "no edit ran {ran:?}");
    }
}

/// Runs one seeded sequence on a graph, checking each step. `evict` drops
/// the stored value of a random node on the `Evict` edit.
pub fn run(seed: u64, steps: usize) {
    let mut rng = Rng::new(seed);
    let mut sim = Sim::new();
    let mut db = open(None, &sim.project);
    db.set_verify(0);
    apply(&mut db, &sim.project, &sim.known);
    pull(&db);
    pull_par(&db);
    db.take_executed();
    let mut tainted = BTreeSet::new();
    for step in 0..steps {
        let before = sim.project.clone();
        let edit = sim.step(&mut rng);
        if matches!(edit, Edit::Evict) {
            evict_some(&mut db, &sim, &mut rng);
            tainted.extend(sim.project.files.keys().cloned());
        }
        apply(&mut db, &sim.project, &sim.known);
        let graph = pull(&db);
        let fanned = pull_par(&db);
        let ran = db.take_executed();
        assert_eq!(
            graph,
            from_scratch(&sim.project, &sim.known),
            "seed {seed} step {step} after {edit:?}"
        );
        assert_eq!(fanned, graph, "seed {seed} step {step}: fan-out report");
        check_executed(&sim, &before, &edit, &tainted, &ran);
    }
}

fn evict_some(db: &mut lighthouse_graph::Db, sim: &Sim, rng: &mut Rng) {
    for path in sim.project.files.keys() {
        match rng.below(4) {
            0 => db.evict(&PARSE, path).expect("evict"),
            1 => db.evict(&FACTS, path).expect("evict"),
            2 => db.evict(&DEPS, path).expect("evict"),
            _ => {}
        }
    }
}

#[test]
fn graph_equals_from_scratch_over_random_edits() {
    for seed in 1..=40 {
        run(seed, 60);
    }
}

#[test]
fn verify_mode_on_every_green_node_finds_no_impure_query() {
    let mut rng = Rng::new(7);
    let mut sim = Sim::new();
    let mut db = open(None, &sim.project);
    db.set_verify(1);
    apply(&mut db, &sim.project, &sim.known);
    pull(&db);
    for _ in 0..80 {
        sim.step(&mut rng);
        apply(&mut db, &sim.project, &sim.known);
        assert_eq!(pull(&db), from_scratch(&sim.project, &sim.known));
    }
}
