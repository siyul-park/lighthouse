//! Running the examples of decisions side by side: one unit of work is the
//! examples of one decision in one language, workers take units in turn, and
//! the results are put back in the order the units were listed in, so that a
//! run reports the same whatever the number of workers.

use std::{collections::BTreeSet, sync::Mutex, time::Duration};

use lighthouse_engine::{Plant, Planted, RuleTester, Spent, Tested};
use lighthouse_plugin::Registry;
use lighthouse_rpc::Started;
use lighthouse_spec::{Catalog, Decision};
use rayon::prelude::*;

use crate::{Result, Session};

/// Stack of a worker: the engine under an example recurses deeply.
const WORKER_STACK: usize = 8 << 20;

/// What running the examples found, in the order of the units.
#[derive(Default)]
pub(crate) struct Found {
    pub(crate) failures: Vec<String>,
    pub(crate) skipped: Vec<String>,
    /// The plugins examples need and the registry lacks, with how many
    /// examples each keeps from running.
    pub(crate) lacking: Vec<(String, usize)>,
    pub(crate) conflicts: Vec<String>,
    pub(crate) spent: Spent,
    /// The time of each unit: decision, language and total.
    pub(crate) per_unit: Vec<(String, String, Duration)>,
}

/// The examples of one decision in one language, run.
struct Unit {
    language: String,
    decision: String,
    skipped: Vec<String>,
    lacking: Vec<(String, usize)>,
    tested: Tested,
}

/// Runs the examples of `decisions` in each of `languages` on `jobs` workers
/// (one means in turn, on the calling thread). Each example gets a registry
/// of its own over the processes `plugins` started; a worker uses the
/// processes of the start its number picks, so that `plugins` holds as many
/// starts as workers should not wait for one another. An example whose
/// registry could not be built runs on the in-process rules, and the run is
/// then an error.
pub(crate) fn run(
    session: &Session,
    catalog: &Catalog,
    plugins: &[Started],
    languages: &BTreeSet<String>,
    decisions: &[&Decision],
    jobs: usize,
) -> Result<Found> {
    let failed = Mutex::new(None);
    let bundled = session.comp.bundled().map_err(|e| e.to_string())?;
    let fresh = || registry(session, plugins, &failed, true);
    let testers: Vec<(&String, RuleTester<'_>)> = languages
        .iter()
        .map(|language| {
            let tester = RuleTester::new(fresh, catalog, &session.base)
                .bundled(bundled)
                .planting(
                    plant(session, catalog, (plugins, &failed)),
                    session.orders(),
                );
            (
                language,
                tester.language(language).trusted(session.trusted()),
            )
        })
        .collect();
    let units: Vec<(&String, &RuleTester<'_>, &Decision)> = testers
        .iter()
        .flat_map(|(language, tester)| {
            decisions
                .iter()
                .copied()
                .filter(|decision| tester.applies(decision))
                .map(move |decision| (*language, tester, decision))
        })
        .collect();
    let done: Vec<Unit> = if jobs <= 1 {
        units.iter().map(|(l, t, d)| unit(l, t, d)).collect()
    } else {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .stack_size(WORKER_STACK)
            .thread_name(|n| format!("lighthouse-example-{n}"))
            .build()
            .map_err(|e| format!("cannot start the workers: {e}"))?;
        pool.install(|| units.par_iter().map(|(l, t, d)| unit(l, t, d)).collect())
    };
    if let Some(e) = failed.lock().map_err(|e| e.to_string())?.take() {
        return Err(format!("plugins could not be started for every example: {e}").into());
    }
    Ok(merge(done))
}

/// The examples of `decision` in `language`.
fn unit(language: &str, tester: &RuleTester<'_>, decision: &Decision) -> Unit {
    Unit {
        language: language.to_owned(),
        decision: decision.id().to_owned(),
        skipped: tester.skipped(decision),
        lacking: tester.lacking_plugins(decision).into_iter().collect(),
        tested: tester.tested(decision),
    }
}

/// The units in their order, their lines prefixed with their language.
fn merge(units: Vec<Unit>) -> Found {
    let mut found = Found::default();
    for unit in units {
        let language = &unit.language;
        let prefixed = |lines: Vec<String>| -> Vec<String> {
            lines
                .into_iter()
                .map(|line| format!("[{language}] {line}"))
                .collect()
        };
        found.skipped.extend(prefixed(unit.skipped));
        found.lacking.extend(unit.lacking);
        found.spent += unit.tested.spent;
        found.per_unit.push((
            unit.decision.clone(),
            unit.language.clone(),
            unit.tested.spent.total(),
        ));
        found.failures.extend(prefixed(unit.tested.failures));
        found.conflicts.extend(prefixed(unit.tested.conflicts));
    }
    found
}

/// A registry over the processes of the start the calling worker uses, with
/// the project's local decisions when `own`; when it cannot be built, the
/// in-process rules alone, and `failed` says why.
fn registry(
    session: &Session,
    plugins: &[Started],
    failed: &Mutex<Option<String>>,
    own: bool,
) -> Registry {
    let lane = rayon::current_thread_index().unwrap_or(0) % plugins.len().max(1);
    let note = |e: String| {
        if let Ok(mut failed) = failed.lock() {
            failed.get_or_insert(e);
        }
    };
    let alone = || {
        if own {
            session.in_process_registry()
        } else {
            session.bare_registry()
        }
    };
    let over = alone().and_then(|mut registry| {
        if let Some(started) = plugins.get(lane) {
            started.register(&mut registry)?;
        }
        Ok(registry)
    });
    over.unwrap_or_else(|e| {
        note(e.to_string());
        alone().unwrap_or_else(|e| {
            note(e.to_string());
            Registry::default()
        })
    })
}

/// The planting of examples: the local decisions an example writes are loaded
/// over `catalog` and their rules registered, in a registry over the processes
/// of the start the calling worker uses and without the project's own local
/// decisions, so that a decision about decisions is shown on decisions of its
/// own.
fn plant<'a>(
    session: &'a Session,
    catalog: &'a Catalog,
    (plugins, failed): (&'a [Started], &'a Mutex<Option<String>>),
) -> Plant<'a> {
    Box::new(move |files| {
        let registry = registry(session, plugins, failed, false);
        let (registry, catalog) =
            lighthouse_checks::planted(&session.comp, registry, catalog, files)?;
        Ok(Planted { registry, catalog })
    })
}
