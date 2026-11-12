//! Looking at decisions and running their examples.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    fs,
    num::NonZeroUsize,
    thread,
    time::{Duration, Instant},
};

use lighthouse_logic::Classified;
use lighthouse_plugin::Registry;
use lighthouse_report::docs::{Density, decision_markdown};
use lighthouse_rpc::{StartOptions, Started};
use lighthouse_spec::{Catalog, Decision, authored_severity};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{Result, Selector, Session, config_file, examples};

/// A decision shown on its own: titled at heading 1, examples named without links.
const ENTRY: Density<'static> = Density::Entry {
    level: 1,
    sources: None,
};

/// The language examples without a provider run in.
const NEUTRAL: &str = "text";

/// How many of the slowest decisions the timings name.
const SLOWEST: usize = 10;

/// Most starts of the plugins a run makes, one for each worker up to this
/// many: a started plugin answers one request at a time.
const MAX_STARTS: usize = 4;

/// One line of a decision listing.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DecisionRow {
    pub id: String,
    pub status: Implementation,
    pub severity: String,
    pub title: String,
    /// How far the decision can be reasoned about, and why; absent for a rule
    /// with no decision in the catalog.
    #[serde(flatten)]
    pub tier: Option<Classified>,
}

/// How far a decision is carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Implementation {
    /// Its status is not accepted: written down, tested, not enforced.
    NotEnforced,
    /// A program checks it.
    Implemented,
    /// Agent review checks it.
    Judged,
    /// A check is named but nothing registers it.
    Unimplemented,
    /// It has no check.
    Doc,
}

lighthouse_model::vocabulary!(Implementation, "implementation" {
    NotEnforced => "not-enforced",
    Implemented => "implemented",
    Judged => "judged",
    Unimplemented => "unimplemented",
    Doc => "doc",
});

/// The outcome of running examples.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DecisionTest {
    pub decisions: usize,
    pub languages: usize,
    pub runs: usize,
    /// One line per failing example; empty when every example holds.
    pub failures: Vec<String>,
    /// One line per example that was not run because the program its command
    /// check names is not installed or not trusted, and one per plugin that
    /// examples need and is not available, with how many examples that keeps
    /// from running; none of them is a failure.
    pub skipped: Vec<String>,
    /// One line per conflict with another decision that an example declares
    /// and still has: a candidate for a contradiction, not a failure.
    pub conflicts: Vec<String>,
    /// Where the time went; reported on request.
    #[serde(skip)]
    pub timings: TestTimings,
}

/// Where the time of a decision test went, in seconds. The phases are summed
/// over all examples (and, when examples run side by side, over all workers).
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestTimings {
    /// Starting the plugins.
    pub plugin_start: f64,
    /// Building each example's engine and checking it.
    pub check: f64,
    /// Applying the fixes and judging them.
    pub fix: f64,
    /// Checking the examples against the other decisions.
    pub crossing: f64,
    /// The decisions that took longest, with the language they ran in.
    pub slowest: Vec<SlowDecision>,
}

/// A decision and the time its examples took in one language.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SlowDecision {
    pub decision: String,
    pub language: String,
    pub seconds: f64,
}

impl TestTimings {
    /// The lines of the breakdown.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![
            format!("timings: plugin start {:.1}s", self.plugin_start),
            format!("timings: check {:.1}s (summed over examples)", self.check),
            format!("timings: fix {:.1}s (summed over examples)", self.fix),
            format!(
                "timings: crossing {:.1}s (summed over examples)",
                self.crossing
            ),
        ];
        out.extend(self.slowest.iter().map(|slow| {
            format!(
                "timings: slowest {} [{}] {:.1}s",
                slow.decision, slow.language, slow.seconds
            )
        }));
        out
    }
}

/// The decisions the registry has a rule for, with their catalog titles; with
/// `all`, every catalog decision too, implemented or not.
pub fn decision_rows(session: &Session, registry: &Registry, all: bool) -> Vec<DecisionRow> {
    let catalog = &session.catalog;
    let tier = |decision: &Decision| Some(lighthouse_logic::tier(session.comp.cel(), decision));
    let mut rows = Vec::new();
    if all {
        for decision in catalog.decisions() {
            rows.push(DecisionRow {
                id: decision.id().to_owned(),
                status: status(decision, registry),
                severity: decision
                    .severity
                    .map_or_else(|| "-".to_owned(), |s| s.to_string()),
                title: decision.title.clone(),
                tier: tier(decision),
            });
        }
    }
    for rule in registry.rules() {
        let meta = rule.manifest();
        if all && catalog.decision(&meta.id).is_some() {
            continue;
        }
        rows.push(DecisionRow {
            id: meta.id.clone(),
            status: Implementation::Implemented,
            severity: meta.severity.to_string(),
            title: meta.description.clone(),
            tier: catalog.decision(&meta.id).and_then(tier),
        });
    }
    rows
}

/// The rules the project's configuration enables for at least one file: the
/// projects it extends and the entries it sets, resolved over the catalog.
pub fn active_decisions(session: &Session) -> Result<BTreeSet<String>> {
    session.require_extended_packs()?;
    Ok(lighthouse_engine::active(
        &session.config,
        session.catalog.projects(),
    )?)
}

/// The effective configuration of the project: its root, the plugins and
/// projects it names, the decisions in force and the text of its file.
pub fn config_summary(session: &Session) -> Result<Value> {
    let plugins: Vec<&str> = session
        .config
        .plugins()
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    let file = config_file(&session.root).and_then(|path| fs::read_to_string(path).ok());
    Ok(json!({
        "root": session.root.display().to_string(),
        "plugins": plugins,
        "extends": session.config.extends(),
        "activeRules": active_decisions(session)?,
        "file": file,
    }))
}

/// The index of the project's catalog, one decision per line: id, authored
/// severity, status and title, tab-separated.
pub fn catalog_index(session: &Session) -> Result<String> {
    let mut out = String::new();
    for decision in session.catalog.decisions() {
        let severity = decision.severity.map_or_else(
            || "doc".to_owned(),
            |s| authored_severity(s, Some(decision)).to_string(),
        );
        let _ = writeln!(
            out,
            "{}\t{severity}\t{}\t{}",
            decision.id(),
            decision.status,
            decision.title
        );
    }
    Ok(out)
}

/// One decision of the project's catalog rendered as Markdown.
pub fn decision_text(session: &Session, id: &str) -> Result<String> {
    let decision = session
        .catalog
        .decision(id)
        .ok_or_else(|| format!("unknown decision `{id}`"))?;
    Ok(decision_markdown(decision, ENTRY))
}

/// The text that explains a decision or rule: context, requirement, examples and
/// status, then the rule's analyzers and capabilities.
pub fn explain(catalog: &Catalog, registry: &Registry, id: &str) -> Result<String> {
    let decision = catalog.decision(id);
    let rule = registry.rule(id);
    let mut out = String::new();
    match (decision, rule) {
        (Some(decision), _) => {
            out.push_str(&decision_markdown(decision, ENTRY));
            let _ = writeln!(out, "Status: {}", status(decision, registry));
        }
        (None, Some(rule)) => {
            let meta = rule.manifest();
            let _ = writeln!(
                out,
                "{}  (default: {})\n\n{}\n\n{}",
                meta.id, meta.severity, meta.description, meta.docs
            );
            let _ = writeln!(out, "Scope: {:?}", meta.scope);
        }
        (None, None) => return Err(format!("unknown decision or rule `{id}`").into()),
    }
    if let Some(rule) = rule {
        let meta = rule.manifest();
        if !meta.analyzers.is_empty() {
            let _ = writeln!(out, "Analyzers: {}", meta.analyzers.join(", "));
        }
        if !meta.capabilities.is_empty() {
            let caps: Vec<_> = meta.capabilities.iter().map(ToString::to_string).collect();
            let _ = writeln!(out, "Capabilities: {}", caps.join(", "));
        }
    }
    Ok(out)
}

/// Runs the examples of the checked decisions named by `ids` (all when
/// empty) in every language the project's plugins provide, or in `language`,
/// on `jobs` workers (the available parallelism when `None`, one meaning in
/// turn). What is reported does not depend on `jobs`.
pub fn test_decisions(
    session: &Session,
    ids: &[String],
    language: Option<&str>,
    jobs: Option<usize>,
) -> Result<DecisionTest> {
    test_catalog(session, &session.catalog, ids, language, jobs)
}

/// Like [`test_decisions`], over `catalog` instead of the project's own.
pub(crate) fn test_catalog(
    session: &Session,
    catalog: &Catalog,
    ids: &[String],
    language: Option<&str>,
    jobs: Option<usize>,
) -> Result<DecisionTest> {
    let decisions = selected(catalog, ids)?;
    let jobs = jobs.unwrap_or_else(available).max(1);
    let started = Instant::now();
    let env = hermetic_go();
    let options = StartOptions {
        cache: None,
        env: &env,
    };
    let plugins = (0..jobs.min(MAX_STARTS))
        .map(|_| session.start_plugins(&options))
        .collect::<Result<Vec<_>>>()?;
    if let Some(gap) = plugins.iter().find_map(|p| p.registered.incomplete.first()) {
        return Err(format!("a language plugin did not start: {}", gap.reason).into());
    }
    let registered = registered_languages(session, &plugins)?;
    let start = started.elapsed();
    let languages = match language {
        Some(language) => BTreeSet::from([language.to_owned()]),
        None => registered.clone(),
    };
    let languages = if languages.is_empty() {
        BTreeSet::from([NEUTRAL.to_owned()])
    } else {
        languages
    };
    let mut found = examples::run(session, catalog, &plugins, &languages, &decisions, jobs)?;
    let mut lacking: BTreeMap<String, usize> = BTreeMap::new();
    let unserved = unserved(&decisions, language, &registered);
    for (plugin, examples) in std::mem::take(&mut found.lacking)
        .into_iter()
        .chain(unserved)
    {
        *lacking.entry(plugin).or_default() += examples;
    }
    let mut skipped = std::mem::take(&mut found.skipped);
    skipped.extend(lacking.iter().map(|(plugin, examples)| {
        format!("plugin `{plugin}` is not available: {examples} example(s) not run")
    }));
    Ok(DecisionTest {
        decisions: decisions.len(),
        languages: languages.len(),
        runs: found.per_unit.len(),
        failures: found.failures,
        skipped,
        conflicts: found.conflicts,
        timings: TestTimings {
            plugin_start: start.as_secs_f64(),
            check: found.spent.check.as_secs_f64(),
            fix: found.spent.fix.as_secs_f64(),
            crossing: found.spent.crossing.as_secs_f64(),
            slowest: slowest(found.per_unit),
        },
    })
}

/// What the Go tool is told in every plugin process of a decision test, so
/// that an example is judged the same on any machine: modules are read as the
/// example's `go.mod` says, and nothing is fetched. The build cache is the
/// user's own, which is shared by every example and keyed by content.
fn hermetic_go() -> Vec<(String, String)> {
    vec![
        ("GOFLAGS".to_owned(), "-mod=mod".to_owned()),
        ("GOPROXY".to_owned(), "off".to_owned()),
    ]
}

/// The languages the registry of the started plugins provides.
fn registered_languages(session: &Session, plugins: &[Started]) -> Result<BTreeSet<String>> {
    let Some(first) = plugins.first() else {
        return Ok(BTreeSet::new());
    };
    let probe = session.registry_from(first)?;
    Ok(probe
        .languages()
        .iter()
        .map(|l| l.provider.manifest().id.clone())
        .filter(|l| l != NEUTRAL)
        .collect())
}

/// The workers of a run unless it says: as many as the machine runs at once.
fn available() -> usize {
    thread::available_parallelism().map_or(1, NonZeroUsize::get)
}

/// The slowest of `spent`, longest first, ties by decision and language.
fn slowest(mut spent: Vec<(String, String, Duration)>) -> Vec<SlowDecision> {
    spent.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| (&a.0, &a.1).cmp(&(&b.0, &b.1))));
    spent.truncate(SLOWEST);
    spent
        .into_iter()
        .map(|(decision, language, spent)| SlowDecision {
            decision,
            language,
            seconds: spent.as_secs_f64(),
        })
        .collect()
}

/// How many examples are written in a language no registered provider
/// serves, by the plugin that would: the language `go` is served by `lang-go`.
/// Only the language asked for counts, else every language; the
/// language-neutral `text` needs none.
fn unserved(
    decisions: &[&Decision],
    asked: Option<&str>,
    registered: &BTreeSet<String>,
) -> BTreeMap<String, usize> {
    let mut lacking = BTreeMap::new();
    let examples = decisions.iter().flat_map(|d| &d.examples);
    for example in examples {
        let language = &example.language;
        if language != NEUTRAL
            && asked.is_none_or(|asked| asked == language)
            && !registered.contains(language)
        {
            *lacking.entry(format!("lang-{language}")).or_default() += 1;
        }
    }
    lacking
}

fn status(decision: &Decision, registry: &Registry) -> Implementation {
    if decision.check.is_some() && !decision.status.enforced() {
        Implementation::NotEnforced
    } else if registry.rule(decision.id()).is_some()
        && decision.check.is_some() == decision.automated()
    {
        // A model check has a rule too, but a model judges it: it stays `judged`.
        Implementation::Implemented
    } else if decision.check.is_none() {
        Implementation::Doc
    } else if !decision.automated() {
        Implementation::Judged
    } else {
        Implementation::Unimplemented
    }
}

fn selected<'c>(catalog: &'c Catalog, ids: &[String]) -> Result<Vec<&'c Decision>> {
    if ids.is_empty() {
        return Ok(catalog.decisions().filter(|d| d.automated()).collect());
    }
    let named = ids;
    let ids = Selector::new(ids)?.expand(catalog.decisions().map(Decision::id))?;
    // A pattern (`design/*`) selects the matching decisions that have a check.
    ids.iter()
        .map(|id| {
            let decision = catalog
                .decision(id)
                .ok_or_else(|| format!("unknown decision `{id}`"))?;
            if !decision.automated() && named.contains(id) {
                return Err(format!("`{id}` has no automated check to test").into());
            }
            Ok(decision)
        })
        .filter(|found| found.as_ref().map_or(true, |d| d.automated()))
        .collect()
}
