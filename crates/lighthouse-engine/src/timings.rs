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
    /// Where a provider says its own time went, by language id: the phases in
    /// the order it ran them. Only providers that say are listed.
    pub index_phases: Vec<(String, Vec<(String, Duration)>)>,
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
        for (language, spent) in &self.index {
            out.push(line(&format!("index {language}"), *spent));
            if let Some(phases) = self.phases_of(language) {
                out.push(format!("timings: index {language}: {phases}"));
            }
        }
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

    /// The phases a provider reported, as `load 12.0ms, analyse 3.0ms`, summed
    /// over its calls; `None` when it reported none.
    fn phases_of(&self, language: &str) -> Option<String> {
        let mut totals: Vec<(&str, Duration)> = Vec::new();
        for (phase, spent) in self
            .index_phases
            .iter()
            .filter(|(id, _)| id == language)
            .flat_map(|(_, phases)| phases)
        {
            match totals.iter_mut().find(|(name, _)| name == phase) {
                Some((_, total)) => *total += *spent,
                None => totals.push((phase, *spent)),
            }
        }
        (!totals.is_empty()).then(|| {
            totals
                .iter()
                .map(|(phase, spent)| format!("{phase} {:.1}ms", spent.as_secs_f64() * 1000.0))
                .collect::<Vec<_>>()
                .join(", ")
        })
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

/// Where the time of a fix run went. `apply` and `settle` contain the checks
/// they make; `checks` lists every check on its own, by round.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FixTimings {
    /// One entry per check of the project over the texts of the run: the round
    /// it belonged to (0 before the first) and how long it took.
    pub checks: Vec<(usize, Duration)>,
    /// Asking the fixers for proposals, lowering their edits included.
    pub propose: Duration,
    /// Applying the proposals of rounds (collision, candidate texts, check,
    /// settling and judging), the checks included.
    pub apply: Duration,
    /// The safe follow-up fixes of what a round introduced, the checks included.
    pub settle: Duration,
    /// Undoing the newest link of a chain that a `[fix] verify` program refused.
    pub rollback: Duration,
    /// Running the programs of command fixes, formatters among them.
    pub command: Duration,
    /// Verifying the hashes and writing the files.
    pub write: Duration,
}

impl FixTimings {
    /// The lines of the breakdown, after those of the check.
    pub fn lines(&self, rounds: usize) -> Vec<String> {
        let mut out = vec![format!("timings: fix rounds {rounds}")];
        let last = self.checks.iter().map(|(at, _)| *at).max().unwrap_or(0);
        for round in 0..=last {
            let (count, spent) = self.round(round);
            if count > 0 {
                out.push(format!(
                    "{} ({count} check(s))",
                    line(&format!("fix round {round} checks"), spent)
                ));
            }
        }
        let (count, spent) = (0..=last).fold((0, Duration::ZERO), |(n, sum), r| {
            let (c, s) = self.round(r);
            (n + c, sum + s)
        });
        out.push(format!("{} ({count} check(s))", line("fix checks", spent)));
        out.push(line("fix propose", self.propose));
        out.push(line("fix apply (with its checks)", self.apply));
        out.push(line("fix settle (with its checks)", self.settle));
        out.push(line("fix rollback", self.rollback));
        out.push(line("fix command", self.command));
        out.push(line("fix write", self.write));
        out
    }

    /// The checks of `round`: how many and how long together.
    fn round(&self, round: usize) -> (usize, Duration) {
        self.checks
            .iter()
            .filter(|(at, _)| *at == round)
            .fold((0, Duration::ZERO), |(n, sum), (_, spent)| {
                (n + 1, sum + *spent)
            })
    }
}

fn line(phase: &str, spent: Duration) -> String {
    format!("timings: {phase} {:.1}ms", spent.as_secs_f64() * 1000.0)
}
