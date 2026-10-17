//! Where the time of a run went, phase by phase.

use std::{collections::BTreeMap, time::Duration};

/// How many rules the breakdown names, longest first.
const TOP_RULES: usize = 5;

/// How long the phases of a run took. Phases that run in parallel are timed on
/// the wall clock; `rule` is the time each rule spent, summed over the files
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
    pub rules: Duration,
    /// Each rule, summed over its runs, longest first.
    pub rule: Vec<(String, Duration)>,
    /// Applying source annotations, ordering the findings, giving them their
    /// fingerprints and gathering what is known about each.
    pub fingerprints: Duration,
    /// Recording the run in the store and applying judgments; set by the
    /// session that ran it.
    pub store: Duration,
}

impl Timings {
    /// The lines of the breakdown. `report` is the time spent printing the
    /// findings and `total` the time of the whole run.
    pub fn lines(&self, report: Duration, total: Duration) -> Vec<String> {
        let mut out = vec![line("setup", self.setup), line("read", self.read)];
        out.extend(
            self.index
                .iter()
                .map(|(language, spent)| line(&format!("index {language}"), *spent)),
        );
        out.push(line("merge", self.merge));
        if self.hashing > Duration::ZERO {
            out.push(format!(
                "{} ({} cache hit(s), {} miss(es))",
                line("hashing", self.hashing),
                self.cache_hits,
                self.cache_misses
            ));
        }
        out.extend(
            self.analyzers
                .iter()
                .map(|(analyzer, spent)| line(&format!("analyzer {analyzer}"), *spent)),
        );
        out.push(format!("{} (wall clock)", line("rules", self.rules)));
        out.extend(self.rule.iter().take(TOP_RULES).map(|(rule, spent)| {
            format!(
                "{} (summed over files)",
                line(&format!("rule {rule}"), *spent)
            )
        }));
        out.push(line("fingerprints", self.fingerprints));
        out.push(line("store", self.store));
        out.push(line("report", report));
        out.push(line("total", total));
        out
    }

    /// Folds `times` into the total of each rule.
    pub(crate) fn add_rules(&mut self, times: BTreeMap<String, Duration>) {
        let mut totals: BTreeMap<String, Duration> =
            std::mem::take(&mut self.rule).into_iter().collect();
        for (rule, time) in times {
            *totals.entry(rule).or_default() += time;
        }
        self.rule = totals.into_iter().collect();
        self.rule
            .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }
}

fn line(phase: &str, spent: Duration) -> String {
    format!("timings: {phase} {:.1}ms", spent.as_secs_f64() * 1000.0)
}
