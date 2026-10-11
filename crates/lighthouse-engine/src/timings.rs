//! Where the time of a run went, phase by phase.

use std::{collections::BTreeMap, time::Duration};

use rayon::ThreadPool;

/// How long the phases of a run took. Phases that run in parallel are timed on
/// the wall clock; `rules` is the time each rule spent, summed over the files
/// it ran on, so it adds up to more than the wall clock of the rules phase.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Timings {
    /// Loading the configuration and starting the plugins, before the engine
    /// runs; set by the session that ran it.
    pub setup: Duration,
    /// Walking the root and reading the files a language claims.
    pub read: Duration,
    /// Indexing by each language provider, by language id; the providers run
    /// side by side.
    pub index: Vec<(String, Duration)>,
    /// Merging what the providers indexed into one project.
    pub merge: Duration,
    /// Each analyzer, over all files, in the order they ran.
    pub analyzers: Vec<(String, Duration)>,
    /// Hashing the merged project for the keys of the result cache; zero
    /// without one.
    pub hashing: Duration,
    /// Rule runs whose findings the result cache had, and those it did not.
    pub cache_hits: usize,
    pub cache_misses: usize,
    /// All the rules together, on the wall clock.
    pub rules_wall: Duration,
    /// Each rule, summed over its runs, longest first.
    pub rules: Vec<(String, Duration)>,
    /// Applying source annotations, ordering the findings, giving them their
    /// identity and gathering what is known about each.
    pub identity: Duration,
    /// Recording the run in the store and applying judgments; set by the
    /// session that ran it.
    pub store: Duration,
}

impl Timings {
    /// Folds `spent` into the total of each rule.
    pub(crate) fn add_rules(&mut self, spent: BTreeMap<String, Duration>) {
        let mut totals: BTreeMap<String, Duration> =
            std::mem::take(&mut self.rules).into_iter().collect();
        for (rule, time) in spent {
            *totals.entry(rule).or_default() += time;
        }
        self.rules = totals.into_iter().collect();
        self.rules
            .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }
}

/// Runs `op` on the worker threads the independent steps of a run share. A
/// worker gets the stack the main thread has, as the expressions of a check
/// recurse deeply. If such workers cannot start, workers with the default
/// stack do, and failing that `op` runs on the global pool.
pub(crate) fn in_pool<R: Send>(op: impl FnOnce() -> R + Send) -> R {
    use std::sync::OnceLock;
    const STACK: usize = 8 << 20;
    static POOL: OnceLock<Option<ThreadPool>> = OnceLock::new();
    let pool = POOL.get_or_init(|| {
        let named =
            || rayon::ThreadPoolBuilder::new().thread_name(|n| format!("lighthouse-worker-{n}"));
        named()
            .stack_size(STACK)
            .build()
            .or_else(|_| named().build())
            .ok()
    });
    match pool {
        Some(pool) => pool.install(op),
        None => op(),
    }
}
